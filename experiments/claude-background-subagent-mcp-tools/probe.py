"""Is a Claude subagent whose `tools` is `Read` offered a connected MCP server's
tools when it runs in the background, where the same agent run in the
foreground is offered none and a background subagent with no `tools` field
is offered all of them?

Three arms. `foreground` and `background` delegate to agents with
`tools: Read` and differ only in the delegation mode the prompt asks for;
`background_inherit` delegates in the background to an agent with no `tools`
field, so the run shows a background subagent can be offered `fx` at all.
Every arm spends a billed model call, which is why
this package declares `driver: billed` and `just probe-run` withholds it
without `--billed`.
"""

from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeOtelProbe,
    DelegationBackground,
    McpConfig,
    McpConnected,
    Model,
)

# Each would change what is measured: whether tool search is on, whether the MCP server
# has connected when the subagent spawns, or which model the subagent runs.
# `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` changes how long `-p` waits for the background
# subagent before exiting.
_UNSET = (
    "ENABLE_TOOL_SEARCH",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
    "CLAUDE_CODE_MCP_STARTUP_WAIT_MS",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
    "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS",
)

_ARGS = ("--allowedTools", "Agent", "--effort", "low")
_FX_TOOLS = ("mcp__fx__alpha", "mcp__fx__beta")

PROBE = ClaudeOtelProbe(
    model="claude-sonnet-5-5",
    budget_usd="0.50",
    marker="AGENTSPEC-PROBE-MARKER-CCBG6",
    arms=(
        ClaudeArm(
            "foreground",
            "foreground",
            "Use the Agent tool with run_in_background set to false to delegate to the "
            "probe-bg-foreground subagent. Do not answer yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "background",
            "background",
            "Use the Agent tool with run_in_background set to true to delegate to the "
            "probe-bg-background subagent. Do not answer yourself.",
            _ARGS,
            McpConfig(),
        ),
        ClaudeArm(
            "background_inherit",
            "background_inherit",
            "Use the Agent tool with run_in_background set to true to delegate to the "
            "probe-bg-inherit subagent. Do not answer yourself.",
            _ARGS,
            McpConfig(),
        ),
    ),
    gates=(
        Model(),
        McpConnected(_FX_TOOLS),
        DelegationBackground("foreground", False),
        DelegationBackground("background", True),
        DelegationBackground("background_inherit", True),
    ),
    unset_env=_UNSET,
    variants=("assertion", "discriminator"),
)
