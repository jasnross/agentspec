"""Which MCP server sources does Claude Code load into a `claude -p` run under
`--setting-sources project`: user-scope servers in `~/.claude.json`, claude.ai connectors, and a
`--plugin-dir` plugin's server, in an arm with no server and in a plugin arm, with and without
`--strict-mcp-config`?

Free: every arm runs under a model id no catalog holds, so it ends at model resolution, and its
stream-json `init` event is the oracle. The arms on the operator's login need that login, for
the account's connectors; the arms with a user-scope server run in a relocated home on a
placeholder key.
"""

from probe_harness.assertions import unique
from probe_harness.families.base import GateFailure, ProbeFunctions
from probe_harness.families.claude_init import (
    ClaudeInitArm,
    ClaudeInitArms,
    ClaudeInitProbe,
    UserConfig,
)
from probe_harness.families.claude_otel import PluginDir
from probe_harness.json_values import JsonValue

_STRICT = ("--strict-mcp-config",)

# Each would change which servers load or how long they take to: an exported
# `ENABLE_CLAUDEAI_MCP_SERVERS=false` drops the connectors from every arm.
_UNSET = ("ENABLE_CLAUDEAI_MCP_SERVERS", "CLAUDE_CODE_MCP_STARTUP_WAIT_MS")


def _assertion(arms: ClaudeInitArms) -> JsonValue:
    """The sources of the MCP servers each arm's `init` lists, sorted and distinct."""
    return {
        name: unique([server.source for server in capture.init.mcp_servers])
        for name, capture in arms.items()
    }


def _judge(arms: ClaudeInitArms) -> None:
    """The operator's account offers a connector, so an arm listing none says the arm dropped
    them rather than that there were none to load."""
    if not any(s.source == "claudeai" for s in arms["none"].init.mcp_servers):
        raise GateFailure(
            "arm none: init listed no claude.ai connector, so no arm can say whether one loads.",
            "Run the probe on a login whose account has at least one connector. No record written.",
        )


PROBE = ClaudeInitProbe(
    model="agentspec-probe-unrecognized-model",
    arms=(
        ClaudeInitArm("none", "project"),
        ClaudeInitArm("strict", "project", _STRICT),
        ClaudeInitArm("plugin", "project", server=PluginDir()),
        ClaudeInitArm("plugin_strict", "project", _STRICT, PluginDir()),
        ClaudeInitArm(
            "plugin_no_connectors",
            "project",
            server=PluginDir(),
            set_env=(("ENABLE_CLAUDEAI_MCP_SERVERS", "false"),),
        ),
        ClaudeInitArm("user_none", "project", home=UserConfig()),
        ClaudeInitArm("user_strict", "project", _STRICT, home=UserConfig()),
        ClaudeInitArm("user_plugin", "project", server=PluginDir(), home=UserConfig()),
        ClaudeInitArm("user_plugin_strict", "project", _STRICT, PluginDir(), UserConfig()),
        ClaudeInitArm("user_all_sources", "project", home=UserConfig(), setting_sources=None),
    ),
    unset_env=_UNSET,
    assertion=_assertion,
    functions=ProbeFunctions(judge=_judge),
    variants=("assertion", "discriminator"),
)
