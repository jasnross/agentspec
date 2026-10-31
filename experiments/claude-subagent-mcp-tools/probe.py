"""For each spelling of an MCP grant in a Claude subagent's `tools` field, which
MCP tools does the subagent's model request carry — directly or as deferred —
and does `ToolSearch` come with them?

Six arms, one per spelling, each delegating to an agent that differs from its
siblings only in `tools`. Every arm spends a billed model call, which is why
this package declares `driver: billed` and `just probe-run` withholds it
without `--billed`.
"""

from probe_harness.assertions import NO_GOVERNED_REQUEST, unique
from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeArms,
    ClaudeOtelProbe,
    DeferredMcp,
    McpConfig,
    McpConnected,
    Model,
)
from probe_harness.gates.bodies import deferred_tools, governed_requests
from probe_harness.json_values import JsonValue
from probe_harness.models.claude_request import ClaudeRequest

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
_MARKER = "AGENTSPEC-PROBE-MARKER-CCMCP4T"


def _shape(governed: list[ClaudeRequest]) -> JsonValue:
    """Which MCP tools the requests offer directly and which deferred, and whether any offers
    `ToolSearch`."""
    return {
        "direct": unique(
            [t.name for r in governed for t in r.tool_list if t.name.startswith("mcp__")]
        ),
        "deferred": unique(
            [line for r in governed for line in deferred_tools(r) if line.startswith("mcp__")]
        ),
        "tool_search": any(t.name == "ToolSearch" for r in governed for t in r.tool_list),
    }


def _assertion(arms: ClaudeArms) -> JsonValue:
    """Each arm's MCP tool shape, from its governed requests."""
    value: dict[str, JsonValue] = {}
    for arm, capture in arms.items():
        governed = governed_requests(capture.requests, _MARKER)
        value[arm] = _shape(governed) if governed else NO_GOVERNED_REQUEST
    return value


PROBE = ClaudeOtelProbe(
    model="claude-sonnet-5-5",
    budget_usd="0.50",
    marker=_MARKER,
    arms=(
        ClaudeArm(
            "inherit",
            "inherit",
            "Use the Agent tool to delegate to the probe-mcp-inherit subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "read",
            "read",
            "Use the Agent tool to delegate to the probe-mcp-read subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "exact",
            "exact",
            "Use the Agent tool to delegate to the probe-mcp-exact subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "exact_search",
            "exact_search",
            "Use the Agent tool to delegate to the probe-mcp-exact-search subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "server",
            "server",
            "Use the Agent tool to delegate to the probe-mcp-server subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "server_glob",
            "server_glob",
            "Use the Agent tool to delegate to the probe-mcp-server-glob subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
    ),
    gates=(Model(), DeferredMcp("inherit", _FX_TOOLS), McpConnected(_FX_TOOLS)),
    unset_env=_UNSET,
    assertion=_assertion,
    variants=("assertion", "discriminator"),
)
