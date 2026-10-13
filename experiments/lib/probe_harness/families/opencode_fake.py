"""The OpenCode fake-provider family: OpenCode talks to a stub model endpoint on loopback.

Fully script-driven: no human step, no credentials, and no model quota. The stub writes
every request body to disk, and that dump is the oracle, so these probes measure at
`outbound-request` depth. Arms run one at a time, each with its own isolated OpenCode
environment and its own stub, so no arm's requests can land in another's sink.

Workspace layout, per arm: `<ws>/<arm>/{project, sink, home, xdg/…, fake.port,
fake.stderr, stdout, stderr, <key>.stamp}`, plus `<ws>/view.json`.
"""

import shutil
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

from probe_harness import views
from probe_harness.contract import Manifest
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
from probe_harness.gates import opencode as gates
from probe_harness.workspace import create_workspace, template_file

FAMILY_TOOLS = ("jq", "opencode")

STUBS_DIR = Path(__file__).resolve().parents[2] / "stubs"
FAKE_PROVIDER = STUBS_DIR / "fake_provider.py"
FX_SERVER = STUBS_DIR / "fx_server.py"

PORT_POLL_SECONDS = 0.1
PORT_POLLS = 50


@dataclass(frozen=True)
class ScriptedCall:
    """The one tool call the stub streams to a marked request."""

    tool: str
    # The exact JSON text sent as the call's `arguments`.
    arguments: str


@dataclass(frozen=True)
class OpenCodeArm:
    name: str
    agent: str
    call: ScriptedCall | None = None


# (package, arm directory, arm) -> a replacement call, or None to keep the arm's own.
type ArmArrange = Callable[[Path, Path, OpenCodeArm], ScriptedCall | None]


@dataclass(frozen=True)
class OpenCodeFakeProbe:
    marker: str
    arms: tuple[OpenCodeArm, ...]
    # `opencode.json` mcp keys. Each gets `<arm>/<key>.stamp`, the template key
    # `<KEY>_STAMP`, and a stamp gate.
    fx_servers: tuple[str, ...] = ()
    tool_result_gate: bool = False
    custom_checks: tuple[CustomCheck, ...] = ()
    variants: tuple[str, ...] = ("assertion",)
    arrange_arm: ArmArrange | None = None
    functions: ProbeFunctions = field(default_factory=ProbeFunctions)


def run(manifest: Manifest, name: str, options: RunOptions, probe: OpenCodeFakeProbe) -> int:
    package = manifest.path.parent
    ws = create_workspace(name)
    with kept_on_failure(ws):
        try:
            for arm in probe.arms:
                _run_arm(package, ws, arm, probe)
            result = judge(ws, probe)
        except GateFailure as e:
            return refused(e, ws)
        return record_and_retain(manifest, ws, result, options, probe.functions)


def judge(ws: Path, probe: OpenCodeFakeProbe) -> JudgeResult:
    """Judge a workspace from the sinks and stamps its arms left."""
    arms = [arm.name for arm in probe.arms]
    for arm in arms:
        if not any((ws / arm / "sink").glob("*.request.json")):
            raise GateFailure(f"probe: arm {arm}: the fake provider received no request")
    view = ws / "view.json"
    views.write_arm_view(view, {arm: views.read_sink(ws / arm / "sink").requests for arm in arms})
    for arm in arms:
        gates.governed(view, arm, probe.marker, ws / arm / "stderr")
    for arm in arms:
        for key in probe.fx_servers:
            gates.stamp(ws, arm, key)
    if probe.tool_result_gate:
        for arm in arms:
            gates.tool_result(view, arm, probe.marker, ws / arm / "stderr")
    run_custom_checks(probe.custom_checks, view, probe.marker)
    if probe.functions.judge is not None:
        probe.functions.judge(ws)
    # Printed before recording: a dry or failed record keeps the workspace, and this view is
    # what a candidate projection is iterated against.
    print(f"probe: the assembled view is at {view}", file=sys.stderr)
    return JudgeResult(view=view, selection=None)


def _run_arm(package: Path, ws: Path, arm: OpenCodeArm, probe: OpenCodeFakeProbe) -> None:
    isolation = OpenCodeIsolation()
    arm_dir = ws / arm.name
    layout = isolation.layout(arm_dir)
    (arm_dir / "sink").mkdir()
    shutil.copytree(
        package / "fixtures" / "project", layout.project, symlinks=True, dirs_exist_ok=True
    )

    call = arm.call
    if probe.arrange_arm is not None:
        call = probe.arrange_arm(package, arm_dir, arm) or call

    argv = [sys.executable, str(FAKE_PROVIDER), str(arm_dir / "sink")]
    if call is not None:
        argv += ["--marker", probe.marker, "--tool", call.tool, "--args", call.arguments]
    port_file = arm_dir / "fake.port"
    with port_file.open("wb") as port_out, (arm_dir / "fake.stderr").open("wb") as fake_err:
        fake = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=port_out, stderr=fake_err)
    # The stub is stopped whatever happens to the arm, so an aborted run never leaves a
    # server bound to loopback.
    try:
        port = _wait_for_port(port_file)
        if not port:
            raise GateFailure(
                f"probe: arm {arm.name}: the fake provider printed no port within 5 s; its "
                "stderr follows:",
                *file_lines(arm_dir / "fake.stderr"),
            )
        stamps = {f"{key.upper()}_STAMP": str(arm_dir / f"{key}.stamp") for key in probe.fx_servers}
        template_file(
            package / "fixtures" / "project" / "opencode.json",
            layout.project / "opencode.json",
            PORT=port,
            PYTHON=sys.executable,
            FX_SERVER=str(FX_SERVER),
            **stamps,
        )
        with (arm_dir / "stdout").open("wb") as out, (arm_dir / "stderr").open("wb") as err:
            completed = subprocess.run(
                ["opencode", "run", "--agent", arm.agent, "-m", "fake/m", "hi"],
                cwd=layout.project,
                env=provider_env(layout.project, isolation.env(layout)),
                stdin=subprocess.DEVNULL,
                stdout=out,
                stderr=err,
                check=False,
            )
        if completed.returncode != 0:
            raise GateFailure(
                f'probe: arm {arm.name}: "opencode run --agent {arm.agent}" exited nonzero; '
                "its stderr follows:",
                *file_lines(arm_dir / "stderr"),
            )
    finally:
        fake.terminate()
        fake.wait()


def _wait_for_port(port_file: Path) -> str:
    for _ in range(PORT_POLLS):
        lines = port_file.read_text(encoding="utf-8").splitlines()
        if lines and lines[0]:
            return lines[0]
        time.sleep(PORT_POLL_SECONDS)
    return ""
