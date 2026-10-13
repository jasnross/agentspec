"""What every family shares: definition building blocks and the five-step pipeline.

Each run is Preflight → Arrange → Act → Judge → Record. Judge reads only the workspace, so
`probe replay` can run it on a workspace some earlier run collected.
"""

import os
import shutil
import sys
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path

from probe_harness import jq_cli, record
from probe_harness.contract import Manifest
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
    "record_and_retain",
    "record_refused",
    "refused",
    "run_custom_checks",
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
    # Return True to keep the workspace after a real record.
    after_record: Callable[[Path], bool] | None = None


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


def kept_for_inspection(ws: Path) -> None:
    print(f"probe: the workspace has been kept for inspection: {ws}", file=sys.stderr)


def refused(failure: GateFailure, ws: Path) -> int:
    """Print a gate's lines and the kept-workspace line; a gate failure never records."""
    for line in failure.lines:
        print(line, file=sys.stderr)
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
        sys.stderr.write(e.stderr)
        kept_for_inspection(ws)
        raise SystemExit(1) from None
    except BaseException:
        kept_for_inspection(ws)
        raise


def record_and_retain(
    manifest: Manifest,
    ws: Path,
    result: JudgeResult,
    options: RunOptions,
    functions: ProbeFunctions,
    *,
    capture: Path | None = None,
    delete_after_record: bool = True,
) -> int:
    """Record the judged result, then keep or delete the workspace.

    A dry run and any failure keep it. A real record deletes it unless the family keeps
    every workspace or the definition's `after_record` asks to keep this one.
    """
    try:
        computed = record.compute(
            manifest, view=result.view, selection=result.selection, capture=capture
        )
        written = None if options.dry_run else record.write(computed)
    except record.RecordRefused as e:
        record_refused(e)
        kept_for_inspection(ws)
        return 1
    record.report(computed, written)
    if options.dry_run:
        return 0
    keep = functions.after_record(ws) if functions.after_record is not None else False
    if delete_after_record and not keep:
        shutil.rmtree(ws, ignore_errors=True)
    return 0


def record_refused(refusal: record.RecordRefused) -> None:
    """Print a recorder refusal: its own lines prefixed, then what it passed through."""
    for line in refusal.lines:
        print(f"record: {line}", file=sys.stderr)
    sys.stderr.writelines(f"{line}\n" for line in refusal.detail)
