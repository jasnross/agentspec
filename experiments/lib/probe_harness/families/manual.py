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

from probe_harness import console, jq_cli, json_values, views
from probe_harness.contract import Manifest, OptionsAssertion
from probe_harness.families.base import (
    GateFailure,
    JudgeResult,
    ProbeFunctions,
    RunOptions,
    UsageError,
    kept_on_failure,
    record_result,
    record_tool_version,
    refused,
)
from probe_harness.source import write_source
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
    ws = arrange_human_workspace(manifest.path.parent, name, options.variant)
    with kept_on_failure(ws):
        write_source(ws, options.source)
        try:
            record_tool_version(manifest, ws)
        except GateFailure as e:
            return refused(e, ws)
        print(f"\nWorkspace ready: {ws / 'project'}", file=sys.stderr)
        print(
            f"(capture lives outside it, at {ws / 'capture'})\n{probe.instructions}",
            file=sys.stderr,
        )

        if not wait_for_capture(ws, wait_for, options.timeout):
            # The workspace is never deleted: discarding one an operator spent a live session
            # on is the single unrecoverable mistake a runner could make, and reading the
            # capture is how they tell a hook that never fired from a procedure that went
            # off the rails.
            return refused(
                GateFailure(
                    f"timed out waiting for the capture after {options.timeout}s",
                    "this run is over; re-run the probe to try again.",
                    f"if the capture arrived after the deadline, record it with: "
                    f"experiments/probe record {name} {ws}",
                ),
                ws,
            )
        try:
            result = judge(manifest, ws, probe, None, interactive=True)
        except GateFailure as e:
            return refused(e, ws)
        return record_result(manifest, ws, result, options)


def judge(
    manifest: Manifest,
    ws: Path,
    probe: ManualProbe,
    selection: str | None,
    *,
    interactive: bool,
) -> JudgeResult:
    """Judge a captured workspace, for a live run and for `probe record` alike.

    Refuses unless the capture exists, is non-empty, and satisfies the manifest's
    `wait_for`, so a workspace whose hook never fired, or fired for the wrong event, is
    never recorded. The assertion's shape, not the driver, then decides the answer: an
    option set is a person choosing from a list, and a projection is jq over the capture.
    """
    if manifest.wait_for is None:
        raise GateFailure("manifest declares no wait_for filter")
    capture = ws / "capture"
    if not capture.is_dir():
        raise GateFailure(f"{ws} is not a probe workspace (no capture/ directory)")
    payloads = capture / "payloads.jsonl"
    if not payloads.is_file() or payloads.stat().st_size == 0:
        raise GateFailure(
            f"no captured payloads at {payloads}",
            "the hook did not fire. Check that the provider was fully quit and reopened.",
        )
    # Written before `wait_for` runs, so a capture that is not JSON is refused as such
    # rather than as jq's parse error.
    view = ws / "view.json"
    views.write_capture_view(payloads, view)
    if not jq_cli.test(manifest.wait_for, payloads, slurp=True):
        raise GateFailure(
            f"the capture does not satisfy the manifest's wait_for: {manifest.wait_for}",
            f"the provider did not produce the payload this probe waits for; see {payloads}.",
        )
    if probe.functions.judge is not None:
        probe.functions.judge(ws)

    if isinstance(manifest.assertion, OptionsAssertion):
        if selection is None:
            if not interactive:
                raise UsageError(
                    f"{manifest.path.parent.name} declares options; pass --selection <id>"
                )
            # The machine has already proved the apparatus worked, so the person answers
            # only the question no machine can.
            selection = prompt_selection(manifest.assertion.options, manifest.question)
        return JudgeResult(view=None, selection=selection)
    return JudgeResult(view=view, selection=None)


def arrange_human_workspace(package: Path, name: str, variant: str) -> Path:
    """Materialize the workspace and return its root, the path `probe record` takes."""
    ws = create_workspace(name, variant)
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
            console.say(f"generated {dst} is not valid JSON — check the workspace path")
            raise SystemExit(1)


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
