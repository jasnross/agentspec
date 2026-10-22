"""When a connected MCP server offers resources, which of `ListMcpResourcesTool`,
`ReadMcpResourceTool`, and `ReadMcpResourceDirTool` is a Claude subagent
offered with no `tools` field, with `tools: Read`, with `Read` beside an
exact or whole-server MCP grant, and with the three named explicitly?

Five arms, each delegating to an agent that differs from its siblings only in
`tools`. Every arm spends a billed model call, which is why
this package declares `driver: billed` and `just probe-run` withholds it
without `--billed`.
"""

from pathlib import Path

from probe_harness.families.base import GateFailure, ProbeFunctions
from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeOtelProbe,
    DeferredMcp,
    McpConfig,
    McpConnected,
    Model,
)
from probe_harness.gates import bodies, claude_otel

# Each would change what is measured: whether tool search is on, whether the MCP server
# has connected when the subagent spawns, or which model the subagent runs.
_UNSET = (
    "ENABLE_TOOL_SEARCH",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
    "CLAUDE_CODE_MCP_STARTUP_WAIT_MS",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
)

_ARGS = ("--allowedTools", "Agent", "--effort", "low")
_FX_TOOLS = ("mcp__fx__alpha", "mcp__fx__beta")
_MARKER = "AGENTSPEC-PROBE-MARKER-CCRES5"
_RESOURCE_TOOLS = ("ListMcpResourcesTool", "ReadMcpResourceTool", "ReadMcpResourceDirTool")


def _judge(ws: Path) -> None:
    """The unrestricted main thread lists all three resource tools.

    Only then can an arm's empty value mean "not offered" rather than "renamed", or "not yet
    offered because the server's resources had not been seen".
    """
    try:
        claude_otel.mcp_connected(bodies.read_claude_arms(ws, _ARM_NAMES), _MARKER, _RESOURCE_TOOLS)
    except GateFailure as e:
        raise GateFailure(
            *e.lines,
            "for these three names the likelier cause is a renamed resource tool, or a "
            "Claude Code that no longer offers them for this server.",
            detail=e.detail,
        ) from None


PROBE = ClaudeOtelProbe(
    model="claude-sonnet-5-5",
    budget_usd="0.50",
    marker=_MARKER,
    arms=(
        ClaudeArm(
            "inherit",
            "inherit",
            "Use the Agent tool to delegate to the probe-res-inherit subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "read",
            "read",
            "Use the Agent tool to delegate to the probe-res-read subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "exact",
            "exact",
            "Use the Agent tool to delegate to the probe-res-exact subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "server_glob",
            "server_glob",
            "Use the Agent tool to delegate to the probe-res-server-glob subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "explicit",
            "explicit",
            "Use the Agent tool to delegate to the probe-res-explicit subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
    ),
    gates=(Model(), DeferredMcp("inherit", _FX_TOOLS), McpConnected(_FX_TOOLS)),
    unset_env=_UNSET,
    functions=ProbeFunctions(judge=_judge),
    variants=("assertion", "discriminator"),
)

_ARM_NAMES = tuple(arm.name for arm in PROBE.arms)
