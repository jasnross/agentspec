#!/usr/bin/env bash
# For each spelling of a grant for a plugin-bundled MCP server in a Claude
# subagent's `tools`, with the agent inside the plugin or in the project, which
# of that server's tools does the subagent's model request carry?
#
# Six arms, each delegating to an agent that differs from its siblings only in
# `tools` and in where it lives. Every arm spends a billed model call, which is
# why this package declares `driver: billed` and `just probe-run` withholds it
# without `--billed`.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"
# shellcheck source=experiments/lib/probe-claude-otel.sh
. "$package/../lib/probe-claude-otel.sh"

probe_require_tools jq claude python3

# No `PROBE_FIXTURE`: the arms discriminate against each other, so there is no
# discriminator fixture to select. See the README.
if [ $# -ne 0 ]; then
	printf 'probe: this runner takes no arguments (got: %s)\n' "$*" >&2
	printf 'probe: set PROBE_DRY_RUN=1 in the environment instead.\n' >&2
	exit 2
fi

dry_run="${PROBE_DRY_RUN:-0}"
case "$dry_run" in
0 | 1) ;;
*)
	printf 'probe: PROBE_DRY_RUN must be 0 or 1 (got: %s)\n' "$dry_run" >&2
	exit 2
	;;
esac

# The constants `probe-claude-otel.sh` reads out of this shell; see
# `claude-agent-effort/probe.sh` for why they are globals. shellcheck cannot
# follow the sourced library, so it sees them as unused.
# shellcheck disable=SC2034

# Pinned to Sonnet 5.5 at `--effort low`, matching `claude-subagent-mcp-tools`:
# Haiku 4.5 did not delegate on 2.1.287 there. The README's "Model choice"
# section has the reasoning.
PROBE_CLAUDE_MODEL=claude-sonnet-5-5

# `--max-budget-usd` is print-mode only and counts subagent spend. A cap hit
# stops subagent spawns, which surfaces as a marker-gate failure rather than as
# a wrong answer.
PROBE_CLAUDE_BUDGET_USD=0.50

PROBE_MARKER=AGENTSPEC-PROBE-MARKER-CCPLUG4

# Each would change what is measured: whether tool search is on
# (`ENABLE_TOOL_SEARCH`, a non-Anthropic `ANTHROPIC_BASE_URL`,
# `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`), whether the MCP server has
# connected when the subagent spawns (`CLAUDE_CODE_MCP_STARTUP_WAIT_MS`), or
# which model the subagent runs (`CLAUDE_CODE_SUBAGENT_MODEL`,
# `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`).
unset ENABLE_TOOL_SEARCH ANTHROPIC_BASE_URL CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS \
	CLAUDE_CODE_MCP_STARTUP_WAIT_MS CLAUDE_CODE_SUBAGENT_MODEL CLAUDE_CODE_SUBAGENT_MODEL_FORCE

# On 2.1.287 `--strict-mcp-config` also drops plugin-bundled MCP servers, so
# this package cannot use it to keep the operator's account-level claude.ai
# connectors out of the run. This variable excludes those connectors instead
# and leaves the plugin's `fx` server connected. User-scope servers still load;
# `probe_claude_gate_mcp_absent` below refuses a run where one is named `fx`.
export ENABLE_CLAUDEAI_MCP_SERVERS=false

# Arm name, then the name its prompt delegates to. A plugin agent is addressed
# as `<plugin>:<name>`. Each arm's project tree and plugin copy hold only its
# own agent, so a delegation to the wrong name finds nothing.
ARMS=(
	read:probe-plug-read
	plugin_exact:fxp:probe-plug-exact
	plugin_glob:fxp:probe-plug-glob
	project_exact:probe-plug-project-exact
	project_glob:probe-plug-project-glob
	project_unprefixed:probe-plug-unprefixed
)

ws=$(probe_workspace_create claude-subagent-plugin-mcp-tools)

# Every library helper returns rather than exits, so the runner owns the exit.
probe_fail() {
	printf 'probe: the workspace has been kept for inspection: %s\n' "$ws" >&2
	exit 1
}

# The MCP server and each arm's plugin copy sit beside the arm's project rather
# than in it. `probe_claude_arm` creates only `<arm>/project` and `<arm>/sink`.
mkdir -p "$ws/fx" || probe_fail
cp "$package/fixtures/fx_server.py" "$ws/fx/fx_server.py" || probe_fail

arm_names=()
for pair in "${ARMS[@]}"; do
	arm=${pair%%:*}
	agent=${pair#*:}
	arm_names+=("$arm")

	# Every arm installs the plugin, so every arm connects `fx` the same way;
	# only the plugin arms also carry an agent inside it.
	mkdir -p "$ws/$arm" || probe_fail
	cp -R "$package/fixtures/plugin" "$ws/$arm/fxp" || probe_fail
	probe_template_file "$package/fixtures/plugin/.mcp.json" "$ws/$arm/fxp/.mcp.json" \
		"FX_SERVER=$ws/fx/fx_server.py" \
		"FX_STAMP=$ws/$arm/fx.stamp"
	case "$agent" in
	fxp:*)
		mkdir -p "$ws/$arm/fxp/agents" || probe_fail
		cp "$package/fixtures/plugin-agents/${agent#fxp:}.md" "$ws/$arm/fxp/agents/" || probe_fail
		;;
	*) ;;
	esac

	# No `--mcp-config` and no `--strict-mcp-config`: the plugin is the only
	# source of `fx`. `--allowedTools Agent` lets the delegation run without a
	# permission grant. The prompt must not contain the marker: a marked
	# main-thread request would read as governed.
	probe_claude_arm "$ws" "$arm" "$package/fixtures/$arm" \
		"Use the Agent tool to delegate to the $agent subagent. Do not answer yourself." \
		--plugin-dir "$ws/$arm/fxp" --allowedTools Agent \
		--effort low || probe_fail
done

# Without a connected server, an arm's tool set says nothing about grants: an
# empty `project_unprefixed` would read as "the unprefixed spelling matched
# nothing" when fx never existed. Runner-local, because it tests a file's
# existence and has no projection logic for a fabricated view to exercise.
for arm in "${arm_names[@]}"; do
	if [ ! -e "$ws/$arm/fx.stamp" ]; then
		printf 'probe: arm %s: the plugin fx server never answered tools/list, so its tool set describes nothing.\n' "$arm" >&2
		probe_fail
	fi
done

probe_claude_assemble_view "$ws" "$ws/view.json" "${arm_names[@]}" || probe_fail
probe_claude_gate_marker "$ws/view.json" "$PROBE_MARKER" || probe_fail
probe_claude_gate_model "$ws/view.json" "$PROBE_MARKER" "$PROBE_CLAUDE_MODEL" || probe_fail
probe_claude_gate_mcp_connected "$ws/view.json" "$PROBE_MARKER" mcp__plugin_fxp_fx__alpha mcp__plugin_fxp_fx__beta || probe_fail
# Without `--strict-mcp-config`, a user-scope server named `fx` would still
# connect, and its tools would pass the projection's `mcp__fx__` filter.
probe_claude_gate_mcp_absent "$ws/view.json" "$PROBE_MARKER" mcp__fx__ || probe_fail

# Printed before any recording branch: a failed or dry `record.sh` leaves the
# workspace, and this path is what a candidate projection is iterated against.
printf 'probe: the assembled view is at %s\n' "$ws/view.json" >&2

if [ "$dry_run" = 1 ]; then
	"$package/../lib/record.sh" \
		--manifest "$package/probe.json" \
		--view "$ws/view.json" \
		--dry-run || probe_fail
	exit 0
fi

"$package/../lib/record.sh" \
	--manifest "$package/probe.json" \
	--view "$ws/view.json" || probe_fail

# Only on a successful recording run. Every path above keeps the workspace.
rm -rf "$ws"
