"""Under an OpenCode agent `permission` map that leads with `"*": "deny"` and
allows `read`, does reading a file outside the project raise a permission
prompt when `external_directory: "ask"` follows the allow — and fail on the
rule when it does not?

Fully script-driven: no human step, no credentials, and no model quota.
OpenCode talks to a fake OpenAI-compatible provider on loopback, which scripts
one `read` tool call and writes every request body to disk. The tool result
OpenCode sends back in the follow-up request is the oracle, so the measurement
is at `outbound-request` depth.
"""

import json
import subprocess
from pathlib import Path

from probe_harness.families.base import GateFailure, ProbeFunctions
from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe, ScriptedCall
from probe_harness.workspace import require_tools

_OUTSIDE_CONTENT = "AGENTSPEC-OUTSIDE-CONTENT-OCEXTR6"


def _preflight() -> None:
    require_tools("git")


def _arrange_arm(package: Path, arm_dir: Path, arm: OpenCodeArm) -> ScriptedCall:
    """Place the read target beside the project, outside it, and script a read of it.

    OpenCode compares a path against the git worktree when the project has one; only a
    non-git project, whose worktree is `/`, falls back to the project directory alone, which
    is the comparison the README traces.
    """
    (arm_dir / "outside").mkdir()
    inside_git = subprocess.run(
        ["git", "rev-parse"],
        cwd=arm_dir,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if inside_git.returncode == 0:
        raise GateFailure(
            f"probe: arm {arm.name}: the workspace {arm_dir} is inside a git repository.",
            "probe: OpenCode would compare the read against that worktree instead; set TMPDIR "
            "outside any repository.",
        )
    target = arm_dir / "outside" / "target.txt"
    target.write_text(f"{_OUTSIDE_CONTENT}\n", encoding="utf-8")
    return ScriptedCall("read", json.dumps({"filePath": str(target)}))


PROBE = OpenCodeFakeProbe(
    marker="AGENTSPEC-PROBE-MARKER-OCEXTR6",
    arms=(
        OpenCodeArm("restated", "probe-restated"),
        OpenCodeArm("bare", "probe-bare"),
    ),
    tool_result_gate=True,
    arrange_arm=_arrange_arm,
    functions=ProbeFunctions(preflight=_preflight),
)
