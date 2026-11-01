"""Under `OpenCodeIsolation`, does OpenCode put an `AGENTS.md` or a `CLAUDE.md` that sits in
a directory above the project into the agent's system prompt?

Fully script-driven: no human step, no credentials, and no model quota. OpenCode talks to a
fake OpenAI-compatible provider on loopback, which writes every request body to disk. The
system messages of the requests the agent governs are the oracle, so the measurement is at
`outbound-request` depth.
"""

import subprocess
from pathlib import Path

from probe_harness.assertions import consistent, unique
from probe_harness.families.base import GateFailure, ProbeFunctions
from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeArms, OpenCodeFakeProbe
from probe_harness.gates.opencode import governed_requests
from probe_harness.json_values import JsonValue
from probe_harness.workspace import require_tools

_MARKER = "AGENTSPEC-PROBE-MARKER-OCANC1"

# Each arm's instruction file, written in the arm directory, the project's parent, with a
# token no other file carries.
_ANCESTORS = {
    "agents_md": ("AGENTS.md", "AGENTSPEC-ANCESTOR-AGENTS-OCANC1"),
    "claude_md": ("CLAUDE.md", "AGENTSPEC-ANCESTOR-CLAUDE-OCANC1"),
}


def _assertion(arms: OpenCodeArms) -> JsonValue:
    """Whether each arm's governed requests carry its ancestor file's token in a system
    message: one bool when they agree."""
    value: dict[str, JsonValue] = {}
    for arm, (_, token) in _ANCESTORS.items():
        carried: list[JsonValue] = [
            any(m.role == "system" and token in m.content for m in r.messages)
            for r in governed_requests(arms[arm].requests, _MARKER)
        ]
        value[arm] = consistent(unique(carried))
    return value


def _preflight() -> None:
    require_tools("git")


def _arrange_arm(fixtures: Path, arm_dir: Path, arm: OpenCodeArm) -> None:
    """Write the arm's instruction file into the arm directory, above the project.

    OpenCode bounds its upward search by the git worktree when the project has one; a non-git
    project's worktree is `/`, which is the case an operator's `~/AGENTS.md` would meet.
    """
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
            f"arm {arm.name}: the workspace {arm_dir} is inside a git repository.",
            "OpenCode would stop its search at that worktree instead; set XDG_STATE_HOME so "
            "the probe workspace root lies outside any repository.",
        )
    name, token = _ANCESTORS[arm.name]
    (arm_dir / name).write_text(f"Instruction canary: {token}\n", encoding="utf-8")


PROBE = OpenCodeFakeProbe(
    marker=_MARKER,
    arms=(
        OpenCodeArm("agents_md", "probe-ancestor"),
        OpenCodeArm("claude_md", "probe-ancestor"),
    ),
    arrange_arm=_arrange_arm,
    assertion=_assertion,
    functions=ProbeFunctions(preflight=_preflight),
    variants=("assertion", "discriminator"),
)
