"""Under `--permission-prompts none`, does a Claude skill whose `allowed-tools`
names an MCP tool exactly, by server, by server wildcard, or by plugin-server
wildcard let the skill's call to that tool run, where `allowed-tools: Read`
gets the same call refused for permission, for a user-configured and a
plugin-bundled server alike?

Six arms, each invoking a `probe-skill` that differs from its siblings only
in `allowed-tools` and in which `fx` tool its body names. Every arm spends a
billed model call, which is why this package declares `driver: billed` and
`just probe-run` withholds it without `--billed`.
"""

from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeOtelProbe,
    McpConfig,
    Model,
    PluginDir,
    ToolAnswered,
)

# Each would change what is measured: whether tool search is on, whether the MCP server
# has connected by the first turn, or which model a subagent would run; kept from the
# sibling packages so the environment matches theirs.
_UNSET = (
    "ENABLE_TOOL_SEARCH",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
    "CLAUDE_CODE_MCP_STARTUP_WAIT_MS",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
)

_MARKER = "AGENTSPEC-PROBE-MARKER-CCSKILL7"

_ARGS = (
    "--append-system-prompt",
    _MARKER,
    "--permission-prompts",
    "none",
    "--allowedTools",
    "ToolSearch",
    "--effort",
    "low",
)
_FX_ALPHA = "mcp__fx__alpha"
_PLUGIN_ALPHA = "mcp__plugin_fxp_fx__alpha"

PROBE = ClaudeOtelProbe(
    model="claude-sonnet-5-5",
    budget_usd="0.50",
    marker=_MARKER,
    arms=(
        ClaudeArm("none", "none", "/probe-skill", _ARGS, McpConfig()),
        ClaudeArm("exact", "exact", "/probe-skill", _ARGS, McpConfig()),
        ClaudeArm("server", "server", "/probe-skill", _ARGS, McpConfig()),
        ClaudeArm("server_glob", "server_glob", "/probe-skill", _ARGS, McpConfig()),
        ClaudeArm("plugin_none", "plugin_none", "/probe-skill", _ARGS, PluginDir()),
        ClaudeArm("plugin_glob", "plugin_glob", "/probe-skill", _ARGS, PluginDir()),
    ),
    gates=(
        Model(),
        ToolAnswered("none", _FX_ALPHA),
        ToolAnswered("exact", _FX_ALPHA),
        ToolAnswered("server", _FX_ALPHA),
        ToolAnswered("server_glob", _FX_ALPHA),
        ToolAnswered("plugin_none", _PLUGIN_ALPHA),
        ToolAnswered("plugin_glob", _PLUGIN_ALPHA),
    ),
    unset_env=_UNSET,
    # A claude.ai connector would add MCP tools the projection cannot tell from the fixture's.
    set_env=(("ENABLE_CLAUDEAI_MCP_SERVERS", "false"),),
    responses_in_view=True,
    variants=("assertion", "discriminator"),
)
