"""What every family shares: definition building blocks and the five-step pipeline.

Each run is Preflight → Arrange → Act → Judge → Record. Judge reads only the workspace, so
`probe record` can run it on a workspace some earlier run collected. The harness never
deletes a workspace: its owner does, once the record it produced has shipped.
"""

import os
import traceback
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path

from probe_harness import console, json_values, record
from probe_harness.arms import ArmCapture, Arms
from probe_harness.contract import CommandSource, Manifest
from probe_harness.gates import GateFailure
from probe_harness.json_values import JsonValue
from probe_harness.preflight import RunOptions
from probe_harness.record import JudgeResult, Observed, Selected
from probe_harness.source import SourceMapError, source_map

__all__ = [
    "ArmCapture",
    "Arms",
    "CustomCheck",
    "CustomProbe",
    "GateFailure",
    "JudgeResult",
    "Observed",
    "ProbeFunctions",
    "RunOptions",
    "Selected",
    "UsageError",
    "call",
    "decide",
    "file_lines",
    "holds",
    "kept_for_inspection",
    "kept_on_failure",
    "observe",
    "provider_env",
    "record_result",
    "record_tool_version",
    "refused",
    "run_checks",
    "run_wait_for",
    "source_changed",
    "workspace_kept",
]


class UsageError(Exception):
    """A usage error found while judging, such as an option set judged with no selection.

    `str(e)` is the message; the caller prints it and exits 2.
    """


@dataclass(frozen=True)
class CustomCheck[V]:
    """A predicate over the family's parsed input that must hold for the run to be read.

    <name> keys the check's declared result in a sample. <failure> is printed line by line
    when it does not hold.
    """

    name: str
    holds: Callable[[V], bool]
    failure: tuple[str, ...]


@dataclass(frozen=True)
class ProbeFunctions[V]:
    """A definition's own steps, for needs only one probe of a family has."""

    # Raise `GateFailure` to refuse before a workspace exists.
    preflight: Callable[[], None] | None = None
    # Receives the family's judged input; raise `GateFailure` to refuse.
    judge: Callable[[V], None] | None = None


@dataclass(frozen=True)
class CustomProbe[V]:
    """A probe fitting no family. Its definition supplies every step itself."""

    required_tools: tuple[str, ...]
    # Receives the workspace and the selected fixture variant.
    arrange: Callable[[Path, str], None]
    act: Callable[[Path], None]
    # Reads the workspace into what the assertion receives.
    judge: Callable[[Path], V]
    # Required by a manifest declaring `expected`, and refused by an option set.
    assertion: Callable[[V], JsonValue] | None = None
    variants: tuple[str, ...] = ("assertion",)
    functions: ProbeFunctions[V] = field(default_factory=ProbeFunctions)


def decide[V](assertion: Callable[[V], JsonValue] | None, value: V) -> JudgeResult:
    """The outcome of a family whose answer is computed: the definition's assertion over
    <value>."""
    # The agreement check guarantees an assertion for a manifest declaring `expected`.
    assert assertion is not None
    return JudgeResult(observe(assertion, value))


def observe[V](assertion: Callable[[V], JsonValue], value: V) -> Observed:
    """The assertion's value over <value>, refusing when it raises or yields no JSON value.

    Either refusal is a fault in the definition, not a statement about the provider, so it
    is a `GateFailure` and keeps the workspace.
    """
    observed = call("assertion", assertion, value)
    try:
        json_values.check(observed)
    except ValueError as e:
        raise GateFailure(f"the assertion did not yield a JSON value: {e}") from None
    except RecursionError:
        raise GateFailure(
            "the assertion did not yield a JSON value: it is cyclic or nested too deeply"
        ) from None
    return Observed(observed)


def run_checks[V](checks: Sequence[CustomCheck[V]], value: V) -> None:
    """Refuse at the first check that does not hold over <value>, or that raises."""
    for check in checks:
        if not holds(check, value):
            raise GateFailure(*check.failure)


def holds[V](check: CustomCheck[V], value: V) -> bool:
    """Whether <check> holds over <value>; one that raises or yields no bool refuses."""
    result = call(f"custom check {check.name}", check.holds, value)
    if type(result) is not bool:
        raise GateFailure(
            f"the custom check {check.name} yielded a {type(result).__name__}, not a bool"
        )
    return result


def run_wait_for[L](wait_for: Callable[[Sequence[L]], bool], lines: Sequence[L]) -> bool:
    """Whether <wait_for> holds over <lines>; one that raises or yields no bool refuses."""
    result = call("wait_for", wait_for, lines)
    if type(result) is not bool:
        raise GateFailure(f"the wait_for yielded a {type(result).__name__}, not a bool")
    return result


def source_changed(source: Mapping[str, str], package: Path) -> record.RecordRefused | None:
    """A refusal when <package>'s source map no longer equals <source>, the one it judged by."""
    try:
        now = source_map(package)
    except SourceMapError as e:
        return record.RecordRefused(*e.lines)
    if now == dict(source):
        return None
    paths = sorted(key for key in now.keys() | source.keys() if now.get(key) != source.get(key))
    return record.RecordRefused(
        "the probe's source changed after its map was computed, so the record could not "
        "name the code that judged it:",
        *(f"  {path}" for path in paths[:10]),
        *((f"  … and {len(paths) - 10} more",) if len(paths) > 10 else ()),
    )


def provider_env(cwd: Path, env: Mapping[str, str] | None = None) -> dict[str, str]:
    """<env> (or this process's environment) with `PWD` naming <cwd>.

    A shell's `cd` updates `PWD`, but a subprocess started in another directory inherits the
    caller's. OpenCode reads `PWD` to find its project, so a stale one points it at the
    wrong directory, where it finds no provider config and fails with an internal error.
    """
    child = dict(os.environ if env is None else env)
    child["PWD"] = str(cwd)
    return child


def file_lines(path: Path) -> tuple[str, ...]:
    """A file's text as printed lines, as `cat` printed it; nothing for an empty or missing file."""
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ()
    return tuple(text.rstrip("\n").split("\n")) if text else ()


def record_tool_version(manifest: Manifest, ws: Path) -> None:
    """Write a `command` version source's output to `<ws>/tool_version`, just before Act.

    Every later judge reads the version from there rather than running the command, so a
    provider updated after the run cannot change the version its record carries. A command
    that prints nothing refuses the run, still before Act: its record could never carry a
    version, and no later judge would run the command again.
    """
    if not isinstance(manifest.version_source, CommandSource):
        return
    timeout = record.VERSION_COMMAND_TIMEOUT_SECONDS
    version = record.run_version_command(manifest.version_source.argv, timeout=timeout)
    if not version:
        raise GateFailure(
            f"the version command printed nothing within {timeout}s: "
            f"{manifest.version_source.command}",
            "the record would carry no tool version; re-run once the command answers.",
        )
    (ws / "tool_version").write_text(version + "\n", encoding="utf-8")


def workspace_kept(ws: Path) -> None:
    console.say(f"the workspace is kept at {ws}")


def kept_for_inspection(ws: Path) -> None:
    console.say(f"the workspace has been kept for inspection: {ws}")


def refused(failure: console.Refusal, ws: Path) -> int:
    """Print a refusal's lines and the kept-workspace line; a refused judge never records."""
    console.say_refusal(failure)
    kept_for_inspection(ws)
    return 1


@contextmanager
def kept_on_failure(ws: Path) -> Iterator[None]:
    """Print the kept-workspace line when a run fails in any way after <ws> exists.

    A nonzero `SystemExit` and every other exception still propagate after the line.
    """
    try:
        yield
    except SystemExit as e:
        if e.code not in (None, 0):
            kept_for_inspection(ws)
        raise
    except BaseException:
        kept_for_inspection(ws)
        raise


def record_result(manifest: Manifest, ws: Path, result: JudgeResult, options: RunOptions) -> int:
    """Record the judged result; a dry run prints it instead. The workspace is kept.

    The record carries the source map preflight computed, which a real run computes again
    before writing: an edit made while the run was under way would otherwise be recorded
    as code that never ran.
    """
    source = options.source
    try:
        computed = record.compute(manifest, ws, result, dry_run=options.dry_run, source=source)
        if not options.dry_run:
            changed = source_changed(source, manifest.path.parent)
            if changed is not None:
                raise changed
        written = None if options.dry_run else record.write(computed)
    except record.RecordRefused as e:
        return refused(e, ws)
    record.report(computed, written)
    workspace_kept(ws)
    return 0


def call[V, R](name: str, function: Callable[[V], R], value: V) -> R:
    """<function> over <value>; an exception it raises becomes a `GateFailure` naming <name>."""
    try:
        return function(value)
    except GateFailure:
        # A definition refusing in its own words.
        raise
    except Exception as e:
        raise GateFailure(
            f"the {name} raised {type(e).__name__}: {e}", detail=_traceback_lines(e)
        ) from None


def _traceback_lines(e: BaseException) -> tuple[str, ...]:
    """<e>'s traceback as printed lines."""
    return tuple(
        line for chunk in traceback.format_exception(e) for line in chunk.rstrip("\n").split("\n")
    )
