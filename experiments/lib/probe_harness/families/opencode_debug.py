"""The OpenCode debug family: one `opencode debug …` oracle whose output is the view.

Fully script-driven: no human step, no network, no credentials. The oracle's raw output
goes to `view.json` unprojected, because the recorder owns projection, so the expression a
probe author validates is the one that runs.

Workspace layout: the variant's fixture tree copied to `<ws>/` (or to `<ws>/project/`
under an isolated layout), plus `<ws>/view.json` and `<ws>/oracle.stderr`.
"""

import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

from probe_harness import jq_cli
from probe_harness.families.base import (
    CustomCheck,
    GateFailure,
    JudgeResult,
    ProbeFunctions,
    RunOptions,
    file_lines,
    kept_on_failure,
    provider_env,
    record_and_retain,
    refused,
    run_custom_checks,
)
from probe_harness.families.opencode_env import OpenCodeIsolation
from probe_harness.workspace import create_workspace

FAMILY_TOOLS = ("jq", "opencode")


@dataclass(frozen=True)
class OpenCodeDebugProbe:
    # After `opencode debug`, e.g. ("config", "--pure").
    argv: tuple[str, ...]
    variants: tuple[str, ...] = ("assertion",)
    isolation: OpenCodeIsolation | None = None
    custom_checks: tuple[CustomCheck, ...] = ()
    functions: ProbeFunctions = field(default_factory=ProbeFunctions)


def run(package: Path, name: str, options: RunOptions, probe: OpenCodeDebugProbe) -> int:
    ws = create_workspace(name)
    with kept_on_failure(ws):
        try:
            _arrange_and_act(package, ws, options.variant, probe)
            result = judge(ws, probe)
        except GateFailure as e:
            return refused(e, ws)
        return record_and_retain(package, ws, result, options, probe.functions)


def judge(ws: Path, probe: OpenCodeDebugProbe) -> JudgeResult:
    """Judge a workspace from the files the oracle left: its output and its stderr."""
    view = ws / "view.json"
    if not view.is_file() or view.stat().st_size == 0:
        raise GateFailure(
            "probe: the oracle produced no output; its stderr follows:",
            *file_lines(ws / "oracle.stderr"),
        )
    # Named here rather than left to the recorder, which would report "projection failed"
    # and send the reader after an expression that is correct.
    if not jq_cli.test(".", view):
        raise GateFailure(
            f"probe: the oracle wrote a document jq cannot parse — {view.stat().st_size} "
            f"bytes at {view}"
        )
    run_custom_checks(probe.custom_checks, view, None)
    if probe.functions.judge is not None:
        probe.functions.judge(ws)
    # Printed before recording: a dry or failed record keeps the workspace, and this view is
    # what a candidate projection is iterated against.
    print(f"probe: the assembled view is at {view}", file=sys.stderr)
    return JudgeResult(view=view, selection=None)


def _arrange_and_act(package: Path, ws: Path, variant: str, probe: OpenCodeDebugProbe) -> None:
    source = package / "fixtures" / variant
    env = None
    cwd = ws
    if probe.isolation is not None:
        layout = probe.isolation.layout(ws)
        cwd = layout.project
        env = probe.isolation.env(layout)
    shutil.copytree(source, cwd, symlinks=True, dirs_exist_ok=True)

    # Redirected to a file rather than piped: `opencode debug` truncates its stdout at
    # 65536 bytes when it is a pipe, which would hand the recorder a half-written document.
    command = " ".join(("opencode", "debug", *probe.argv))
    with (ws / "view.json").open("wb") as out, (ws / "oracle.stderr").open("wb") as err:
        completed = subprocess.run(
            ["opencode", "debug", *probe.argv],
            cwd=cwd,
            env=provider_env(cwd, env),
            stdout=out,
            stderr=err,
            check=False,
        )
    if completed.returncode != 0:
        raise GateFailure(f'probe: "{command}" exited nonzero:', *file_lines(ws / "oracle.stderr"))
