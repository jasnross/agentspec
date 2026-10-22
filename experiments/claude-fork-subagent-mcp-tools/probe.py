"""Under fork mode (`CLAUDE_CODE_FORK_SUBAGENT=1`, `-p`), is a Claude subagent
whose `tools` is `Read` offered a connected MCP server's tools, where a
subagent with no `tools` field is offered all of them?

Two arms. `fork_restricted` delegates to an agent with `tools: Read`;
`fork_inherit` delegates to an agent with no `tools` field, so the run shows a
subagent under fork mode can be offered `fx` at all. Every arm spends a billed
model call, which is why this package declares `driver: billed` and
`just probe-run` withholds it without `--billed`.
"""

from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeOtelProbe,
    DelegationFork,
    McpConfig,
    McpConnected,
    Model,
)

# Each would change what is measured: whether tool search is on, whether the MCP server
# has connected when the subagent spawns, or which model the subagent runs.
# `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` changes how long `-p` waits for the background
# subagent before exiting, and `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` runs every subagent
# in the foreground even under fork mode.
_UNSET = (
    "ENABLE_TOOL_SEARCH",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
    "CLAUDE_CODE_MCP_STARTUP_WAIT_MS",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
    "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS",
    "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS",
)

_ARGS = ("--allowedTools", "Agent", "--effort", "low")
_FX_TOOLS = ("mcp__fx__alpha", "mcp__fx__beta")

PROBE = ClaudeOtelProbe(
    model="claude-sonnet-5-5",
    budget_usd="0.50",
    marker="AGENTSPEC-PROBE-MARKER-CCFORK8",
    arms=(
        ClaudeArm(
            "fork_restricted",
            "fork_restricted",
            "Use the Agent tool to delegate to the probe-fork-restricted subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "fork_inherit",
            "fork_inherit",
            "Use the Agent tool to delegate to the probe-fork-inherit subagent. Do not answer "
            "yourself.",
            _ARGS,
            McpConfig(),
        ),
    ),
    gates=(
        Model(),
        McpConnected(_FX_TOOLS),
        DelegationFork("fork_restricted"),
        DelegationFork("fork_inherit"),
    ),
    unset_env=_UNSET,
    set_env=(("CLAUDE_CODE_FORK_SUBAGENT", "1"),),
    variants=("assertion", "discriminator"),
)
