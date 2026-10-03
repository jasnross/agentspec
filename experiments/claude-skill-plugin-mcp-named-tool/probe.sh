#!/usr/bin/env bash
# Under `--permission-prompts none`, does a Claude skill whose `allowed-tools`
# names one tool of a plugin-bundled MCP server,
# `mcp__plugin_<plugin>_<server>__<tool>`, let the skill's call to that tool
# run, where `allowed-tools: Read` gets the same call refused for permission?
#
# Two arms, each invoking a `probe-skill` that differs from its sibling only in
# `allowed-tools`. Every arm spends a billed model call, which is why this
# package declares `driver: billed` and `just probe-run` withholds it without
# `--billed`.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"
# shellcheck source=experiments/lib/probe-claude-otel.sh
. "$package/../lib/probe-claude-otel.sh"

probe_require_tools jq claude python3

# No `PROBE_FIXTURE`: the arms discriminate against each other, so there is no
# discriminator fixture to select (`TODO.md` #24).
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

# Pinned to Sonnet 5.5 at `--effort low` so that every Claude record of the MCP
# tool grants design describes one model; the README's "Model choice" section
# has the reasoning.
PROBE_CLAUDE_MODEL=claude-sonnet-5-5

# `--max-budget-usd` is print-mode only. A cap hit ends the turn before the
# call is answered, which surfaces as a gate-9 failure rather than as a wrong
# answer.
PROBE_CLAUDE_BUDGET_USD=0.50

PROBE_MARKER=AGENTSPEC-PROBE-MARKER-CCSKPN9

# Each would change what is measured: whether tool search is on
# (`ENABLE_TOOL_SEARCH`, a non-Anthropic `ANTHROPIC_BASE_URL`,
# `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`), whether the MCP server has
# connected by the first turn (`CLAUDE_CODE_MCP_STARTUP_WAIT_MS`), or which
# model a subagent would run (`CLAUDE_CODE_SUBAGENT_MODEL`,
# `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`), kept from the sibling runners so the
# environment matches theirs.
unset ENABLE_TOOL_SEARCH ANTHROPIC_BASE_URL CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS \
	CLAUDE_CODE_MCP_STARTUP_WAIT_MS CLAUDE_CODE_SUBAGENT_MODEL CLAUDE_CODE_SUBAGENT_MODEL_FORCE

# On 2.1.287 `--strict-mcp-config` also drops plugin-bundled MCP servers, so
# the arms cannot use it to keep the operator's account-level claude.ai
# connectors out of the run. This variable excludes those connectors instead
# and leaves the plugin's `fx` server connected.
export ENABLE_CLAUDEAI_MCP_SERVERS=false

# The tool both arms' skill bodies tell the model to call.
PROBE_TOOL=mcp__plugin_fxp_fx__alpha
ARMS=(plugin_none plugin_named)

ws=$(probe_workspace_create claude-skill-plugin-mcp-named-tool)

# Every library helper returns rather than exits, so the runner owns the exit.
probe_fail() {
	printf 'probe: the workspace has been kept for inspection: %s\n' "$ws" >&2
	exit 1
}

# The MCP server and each arm's plugin sit beside the arm's project rather than
# in it. `probe_claude_arm` creates only `<arm>/project` and `<arm>/sink`.
mkdir -p "$ws/fx" || probe_fail
cp "$package/fixtures/fx_server.py" "$ws/fx/fx_server.py" || probe_fail

for arm in "${ARMS[@]}"; do
	mkdir -p "$ws/$arm" || probe_fail
	cp -R "$package/fixtures/plugin" "$ws/$arm/fxp" || probe_fail
	probe_template_file "$package/fixtures/plugin/.mcp.json" "$ws/$arm/fxp/.mcp.json" \
		"FX_SERVER=$ws/fx/fx_server.py" \
		"FX_STAMP=$ws/$arm/fx.stamp"

	# A skill's body reaches `messages[]`, never `.system`, so
	# `--append-system-prompt` puts the marker where the marker and model gates
	# look. `--permission-prompts none` denies any call the skill did not
	# pre-approve. `--allowedTools ToolSearch` keeps a deferred `fx` tool
	# loadable without pre-approving any `fx` tool itself.
	probe_claude_arm "$ws" "$arm" "$package/fixtures/$arm" "/probe-skill" \
		--plugin-dir "$ws/$arm/fxp" \
		--append-system-prompt "$PROBE_MARKER" \
		--permission-prompts none \
		--allowedTools ToolSearch \
		--effort low || probe_fail
done

# Without a connected server, an arm's call fails as an unknown tool rather than
# on any permission. Runner-local, because it tests a file's existence and has
# no projection logic for a fabricated view to exercise.
for arm in "${ARMS[@]}"; do
	if [ ! -e "$ws/$arm/fx.stamp" ]; then
		printf 'probe: arm %s: the fx server never answered tools/list, so its call describes nothing.\n' "$arm" >&2
		probe_fail
	fi
done

probe_claude_assemble_view "$ws" "$ws/view.json" "${ARMS[@]}" || probe_fail
probe_claude_gate_marker "$ws/view.json" "$PROBE_MARKER" || probe_fail
probe_claude_gate_model "$ws/view.json" "$PROBE_MARKER" "$PROBE_CLAUDE_MODEL" || probe_fail
for arm in "${ARMS[@]}"; do
	probe_claude_gate_tool_answered "$ws" "$arm" "$PROBE_TOOL" || probe_fail
done

# A request body carries no earlier assistant turn, so the call itself appears
# only in the arm's response bodies. Gate 9 above has already failed any arm
# with no response files; the guard below keeps an empty glob from handing
# `cat` no arguments, which would read the runner's stdin instead. They are
# appended to the arm's array here, after the gates above have read the
# request-only view, so the projection can match each `tool_use` id to its
# `tool_result`. A response has no `.system`, so it is never governed.
for arm in "${ARMS[@]}"; do
	shopt -s nullglob
	responses=("$ws/$arm/sink"/*.response.json)
	shopt -u nullglob
	[ "${#responses[@]}" -gt 0 ] || probe_fail
	jq --arg a "$arm" --slurpfile r <(cat "${responses[@]}") '.[$a] += $r' \
		"$ws/view.json" >"$ws/view.next.json" || probe_fail
	mv "$ws/view.next.json" "$ws/view.json" || probe_fail
done

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
