"""What every family shares: definition building blocks and the five-step pipeline.

Each run is Preflight → Arrange → Act → Judge → Record. Judge reads only the workspace, so
`probe replay` can run it on a workspace some earlier run collected. The harness never
deletes a workspace: its owner does, once the record it produced has shipped.
"""

import os
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path

from probe_harness import console, jq_cli, record
from probe_harness.contract import CommandSource, Manifest
from probe_harness.gates import GateFailure
from probe_harness.preflight import RunOptions

__all__ = [
    "CustomCheck",
    "CustomProbe",
    "GateFailure",
    "JudgeResult",
    "ProbeFunctions",
    "RunOptions",
    "file_lines",
    "kept_for_inspection",
    "kept_on_failure",
    "provider_env",
    "record_result",
    "record_tool_version",
    "refused",
    "run_custom_checks",
    "workspace_kept",
]


@dataclass(frozen=True)
class CustomCheck:
    """A jq expression over the view that must yield true, run with `-e`.

    Evaluated with `--arg m <marker>` when the definition declares a marker. <failure> is
    printed line by line when it does not hold.
    """

    expression: str
    failure: tuple[str, ...]


@dataclass(frozen=True)
class ProbeFunctions:
    """A definition's own steps, for needs only one probe of a family has."""

    # Raise `GateFailure` to refuse before a workspace exists.
    preflight: Callable[[], None] | None = None
    # Receives the workspace path only; raise `GateFailure` to refuse.
    judge: Callable[[Path], None] | None = None


@dataclass(frozen=True)
class JudgeResult:
    """What Judge hands to Record: exactly one of a view or a selected option id."""

    view: Path | None
    selection: str | None


@dataclass(frozen=True)
class CustomProbe:
    """A probe fitting no family. Its definition supplies every step itself."""

    required_tools: tuple[str, ...]
    # Receives the workspace and the selected fixture variant.
    arrange: Callable[[Path, str], None]
    act: Callable[[Path], None]
    judge: Callable[[Path], JudgeResult]
    variants: tuple[str, ...] = ("assertion",)
    functions: ProbeFunctions = field(default_factory=ProbeFunctions)


def run_custom_checks(checks: Sequence[CustomCheck], view: Path, marker: str | None) -> None:
    args = {"m": marker} if marker is not None else {}
    for check in checks:
        if not jq_cli.test(check.expression, view, args=args):
            raise GateFailure(*check.failure)


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


def refused(failure: GateFailure, ws: Path) -> int:
    """Print a gate's lines and the kept-workspace line; a gate failure never records."""
    console.say_refusal(failure)
    kept_for_inspection(ws)
    return 1


@contextmanager
def kept_on_failure(ws: Path) -> Iterator[None]:
    """Print the kept-workspace line when a run fails in any way after <ws> exists.

    A nonzero `SystemExit` and every other exception still propagate after the line. An
    unexpected jq failure becomes jq's own diagnostic and exit 1 rather than a traceback.
    """
    try:
        yield
    except SystemExit as e:
        if e.code not in (None, 0):
            kept_for_inspection(ws)
        raise
    except jq_cli.JqError as e:
        console.verbatim(e.stderr.splitlines())
        kept_for_inspection(ws)
        raise SystemExit(1) from None
    except BaseException:
        kept_for_inspection(ws)
        raise


def record_result(
    manifest: Manifest,
    ws: Path,
    result: JudgeResult,
    options: RunOptions,
    *,
    capture: Path | None = None,
) -> int:
    """Record the judged result; a dry run prints it instead. The workspace is kept."""
    try:
        computed = record.compute(
            manifest,
            view=result.view,
            selection=result.selection,
            capture=capture,
            workspace=ws,
        )
        written = None if options.dry_run else record.write(computed)
    except record.RecordRefused as e:
        console.say_refusal(e)
        kept_for_inspection(ws)
        return 1
    record.report(computed, written)
    workspace_kept(ws)
    return 0
