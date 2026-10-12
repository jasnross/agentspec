"""For each spelling of a grant for a plugin-bundled MCP server in a Claude
subagent's `tools`, with the agent inside the plugin or in the project, which
of that server's tools does the subagent's model request carry?

Six arms, each delegating to an agent that differs from its siblings only in
`tools` and in where it lives. Every arm spends a billed model call, which is
why this package declares `driver: billed` and `just probe-run` withholds it
without `--billed`.
"""

import shutil
from pathlib import Path

from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeOtelProbe,
    McpAbsent,
    McpConnected,
    Model,
    PluginDir,
)

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
_PLUGIN_TOOLS = ("mcp__plugin_fxp_fx__alpha", "mcp__plugin_fxp_fx__beta")

# The fixtures/ subdirectory the plugin arms' agents are copied from, so a reachability
# check counts it.
FIXTURE_PATHS = ("plugin-agents",)

# The arms whose agent lives in the plugin, and that agent's file stem.
_PLUGIN_AGENTS = {"plugin_exact": "probe-plug-exact", "plugin_glob": "probe-plug-glob"}


def _arrange_arm(package: Path, arm_dir: Path, arm: ClaudeArm) -> None:
    stem = _PLUGIN_AGENTS.get(arm.name)
    if stem is None:
        return
    agents = arm_dir / "fxp" / "agents"
    agents.mkdir(parents=True, exist_ok=True)
    shutil.copy(package / "fixtures" / "plugin-agents" / f"{stem}.md", agents)


PROBE = ClaudeOtelProbe(
    model="claude-sonnet-5-5",
    budget_usd="0.50",
    marker="AGENTSPEC-PROBE-MARKER-CCPLUG4",
    arms=(
        ClaudeArm(
            "read",
            "read",
            "Use the Agent tool to delegate to the probe-plug-read subagent. Do not answer "
            "yourself.",
            _ARGS,
            PluginDir(),
        ),
        ClaudeArm(
            "plugin_exact",
            "plugin_exact",
            "Use the Agent tool to delegate to the fxp:probe-plug-exact subagent. Do not answer "
            "yourself.",
            _ARGS,
            PluginDir(),
        ),
        ClaudeArm(
            "plugin_glob",
            "plugin_glob",
            "Use the Agent tool to delegate to the fxp:probe-plug-glob subagent. Do not answer "
            "yourself.",
            _ARGS,
            PluginDir(),
        ),
        ClaudeArm(
            "project_exact",
            "project_exact",
            "Use the Agent tool to delegate to the probe-plug-project-exact subagent. "
            "Do not answer yourself.",
            _ARGS,
            PluginDir(),
        ),
        ClaudeArm(
            "project_glob",
            "project_glob",
            "Use the Agent tool to delegate to the probe-plug-project-glob subagent. Do not answer "
            "yourself.",
            _ARGS,
            PluginDir(),
        ),
        ClaudeArm(
            "project_unprefixed",
            "project_unprefixed",
            "Use the Agent tool to delegate to the probe-plug-unprefixed subagent. Do not answer "
            "yourself.",
            _ARGS,
            PluginDir(),
        ),
    ),
    gates=(Model(), McpConnected(_PLUGIN_TOOLS), McpAbsent("mcp__fx__")),
    unset_env=_UNSET,
    # A claude.ai connector would add MCP tools the projection cannot tell from the fixture's.
    set_env=(("ENABLE_CLAUDEAI_MCP_SERVERS", "false"),),
    arrange_arm=_arrange_arm,
)
