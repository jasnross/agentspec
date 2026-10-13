"""The manual family: a person drives the provider while the runner blocks.

Packages here differ only in their fixtures, the instructions they print, and whether the
answer is read from the capture or from a person.

The workspace separates two halves deliberately:

    <ws>/project/   provider config only; this is what the operator opens
    <ws>/capture/   the hook script and its payloads

The capture sits outside the opened project because a probe whose oracle is the agent's
answer can otherwise be defeated by the agent reading the apparatus: a marker string in a
capture script inside the project is findable with a filesystem search. Nothing is written
outside <ws>, which makes a collision with a real `agentspec sync` target impossible.
"""

import os
import shutil
import sys
from dataclasses import dataclass, field
from pathlib import Path

from probe_harness import json_values, record, views
from probe_harness.contract import Manifest, OptionsAssertion
from probe_harness.families.base import (
    GateFailure,
    ProbeFunctions,
    RunOptions,
    kept_for_inspection,
    kept_on_failure,
    record_refused,
    refused,
)
from probe_harness.workspace import (
    create_workspace,
    prompt_selection,
    template_file,
    wait_for_capture,
)

FAMILY_TOOLS = ("jq",)


@dataclass(frozen=True)
class ManualProbe:
    instructions: str
    variants: tuple[str, ...] = ("assertion",)
    functions: ProbeFunctions = field(default_factory=ProbeFunctions)


def run(
    manifest: Manifest, name: str, wait_for: str, options: RunOptions, probe: ManualProbe
) -> int:
    """The whole human-driven flow as one blocking invocation: arrange, print, poll, record.

    Blocking is what removes the stale-capture failure: the workspace was created by the
    invocation still running, so there is no earlier capture to mistake for this one.
    """
    ws = arrange_human_workspace(manifest.path.parent, name)

    with kept_on_failure(ws):
        print(f"\nWorkspace ready: {ws / 'project'}", file=sys.stderr)
        print(
            f"(capture lives outside it, at {ws / 'capture'})\n{probe.instructions}",
            file=sys.stderr,
        )

        if wait_for_capture(ws, wait_for, options.timeout):
            if probe.functions.judge is not None:
                try:
                    probe.functions.judge(ws)
                except GateFailure as e:
                    return refused(e, ws)
            code = record_capture(manifest, ws, dry_run=options.dry_run)
            if code != 0:
                kept_for_inspection(ws)
            elif not options.dry_run and probe.functions.after_record is not None:
                # Every manual workspace is kept, so its answer changes nothing here; the
                # function still runs for whatever it reports.
                probe.functions.after_record(ws)
            return code

    # The workspace is never deleted: discarding one an operator spent a live session on is
    # the single unrecoverable mistake a runner could make, and reading the capture is how
    # they tell a hook that never fired from a procedure that went off the rails.
    sys.stderr.write("\nprobe: timed out waiting for the capture.\n")
    sys.stderr.write("This run is over; re-run the probe to try again.\n")
    sys.stderr.write(f"The workspace has been kept for inspection: {ws}\n\n")
    return 1


def arrange_human_workspace(package: Path, name: str) -> Path:
    """Materialize the workspace and return its root, the path `--capture` takes."""
    ws = create_workspace(name)
    with kept_on_failure(ws):
        _materialize(package, ws)
    return ws


def _materialize(package: Path, ws: Path) -> None:
    project = ws / "project"
    project.mkdir()

    capture_script = ws / "capture" / "dump-hook.sh"
    template_file(
        package / "fixtures" / "capture" / "dump-hook.sh",
        capture_script,
        JQ=shutil.which("jq") or "jq",
        PAYLOADS=str(ws / "capture" / "payloads.jsonl"),
    )
    _make_executable(capture_script)

    fixtures = package / "fixtures"
    for src in sorted(fixtures.rglob("*")):
        # Regular files only: a fixture tree is repo-controlled, so a symlink would be a
        # mistake rather than a feature.
        if src.is_symlink() or not src.is_file():
            continue
        rel = src.relative_to(fixtures)
        if rel.parts[0] == "capture":
            continue
        dst = project / rel
        template_file(src, dst, CAPTURE_SCRIPT=str(capture_script))

        # Templating writes a fresh file, so carry the executable bit across, or a fixture
        # the provider is meant to run becomes a hook that silently never fires.
        if os.access(src, os.X_OK):
            _make_executable(dst)

        # A workspace path carrying a quote or backslash would yield config the provider
        # silently ignores. Fail here, where the cause is obvious.
        if rel.suffix == ".json" and not _parses(dst):
            print(
                f"probe: generated {dst} is not valid JSON — check the workspace path",
                file=sys.stderr,
            )
            raise SystemExit(1)


def record_capture(
    manifest: Manifest, ws: Path, *, dry_run: bool, selection: str | None = None
) -> int:
    """Record from a completed capture, projecting or prompting per the assertion's shape.

    Refusing to record on an empty capture is the correct outcome: the failure stays loud
    rather than becoming a record nobody can trust. Under replay, <selection> stands in for
    the operator's answer.
    """
    payloads = ws / "capture" / "payloads.jsonl"
    if not (ws / "capture").is_dir():
        print(f"probe: {ws} is not a probe workspace (no capture/ directory)", file=sys.stderr)
        return 1
    if not payloads.is_file() or payloads.stat().st_size == 0:
        print(f"probe: no captured payloads at {payloads}", file=sys.stderr)
        print(
            "probe: the hook did not fire. Check that the provider was fully quit and reopened.",
            file=sys.stderr,
        )
        return 1

    # The assertion shape decides this, not the driver: an `options` set is a person
    # choosing from a list and a `projection` is jq over a file.
    view: Path | None = None
    if isinstance(manifest.assertion, OptionsAssertion):
        if selection is None:
            # The machine has already proved the apparatus worked, so the person answers
            # only the question no machine can.
            selection = prompt_selection(manifest.assertion.options, manifest.question)
    else:
        view = ws / "view.json"
        try:
            views.write_capture_view(payloads, view)
        except GateFailure as e:
            for line in e.lines:
                print(line, file=sys.stderr)
            return 1

    try:
        computed = record.compute(manifest, view=view, selection=selection, capture=ws)
        written = None if dry_run else record.write(computed)
    except record.RecordRefused as e:
        record_refused(e)
        return 1
    record.report(computed, written)
    return 0


def _make_executable(path: Path) -> None:
    # As `chmod +x`: add every execute bit the umask allows.
    umask = os.umask(0)
    os.umask(umask)
    path.chmod(path.stat().st_mode | (0o111 & ~umask))


def _parses(path: Path) -> bool:
    try:
        json_values.loads(path.read_text(encoding="utf-8"))
    except ValueError:
        return False
    return True
