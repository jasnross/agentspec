#!/usr/bin/env bash
# Is a Claude subagent whose `tools` is `Read` offered a connected MCP server's
# tools when it runs in the background, where the same agent run in the
# foreground is offered none and a background subagent with no `tools` field
# is offered all of them?
#
# Three arms. `foreground` and `background` delegate to agents with
# `tools: Read` and differ only in the delegation mode the prompt asks for;
# `background_inherit` delegates in the background to an agent with no `tools`
# field, so the run shows a background subagent can be offered `fx` at all.
# Every arm spends a billed model call, which is why
# this package declares `driver: billed` and `just probe-run` withholds it
# without `--billed`.
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

# Pinned to Sonnet 5.5 at `--effort low`, matching `claude-subagent-mcp-tools`:
# Haiku 4.5 did not delegate on 2.1.287 there. The README's "Model choice"
# section has the reasoning.
PROBE_CLAUDE_MODEL=claude-sonnet-5-5

# `--max-budget-usd` is print-mode only and counts subagent spend. A cap hit
# stops subagent spawns, which surfaces as a marker-gate failure rather than as
# a wrong answer.
PROBE_CLAUDE_BUDGET_USD=0.50

PROBE_MARKER=AGENTSPEC-PROBE-MARKER-CCBG6

# Each would change what is measured: whether tool search is on
# (`ENABLE_TOOL_SEARCH`, a non-Anthropic `ANTHROPIC_BASE_URL`,
# `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`), whether the MCP server has
# connected when the subagent spawns (`CLAUDE_CODE_MCP_STARTUP_WAIT_MS`), or
# which model the subagent runs (`CLAUDE_CODE_SUBAGENT_MODEL`,
# `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`). `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`
# changes how long `-p` waits for the background subagent before exiting.
unset ENABLE_TOOL_SEARCH ANTHROPIC_BASE_URL CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS \
	CLAUDE_CODE_MCP_STARTUP_WAIT_MS CLAUDE_CODE_SUBAGENT_MODEL CLAUDE_CODE_SUBAGENT_MODEL_FORCE \
	CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS

# Arm name, then the agent its prompt delegates to. Each arm's fixture tree
# holds only its own agent, so a delegation to the wrong name finds nothing.
ARMS=(
	foreground:probe-bg-foreground
	background:probe-bg-background
	background_inherit:probe-bg-inherit
)

ws=$(probe_workspace_create claude-background-subagent-mcp-tools)

# Every library helper returns rather than exits, so the runner owns the exit.
probe_fail() {
	printf 'probe: the workspace has been kept for inspection: %s\n' "$ws" >&2
	exit 1
}

# The MCP server and each arm's config sit beside the arm's project rather than
# in it. `probe_claude_arm` creates only `<arm>/project` and `<arm>/sink`.
mkdir -p "$ws/fx" || probe_fail
cp "$package/fixtures/fx_server.py" "$ws/fx/fx_server.py" || probe_fail

arm_names=()
for pair in "${ARMS[@]}"; do
	arm=${pair%%:*}
	agent=${pair#*:}
	arm_names+=("$arm")
	mkdir -p "$ws/$arm" || probe_fail
	probe_template_file "$package/fixtures/mcp.json" "$ws/$arm/mcp.json" \
		"FX_SERVER=$ws/fx/fx_server.py" \
		"FX_STAMP=$ws/$arm/fx.stamp"

	# Every prompt names the mode explicitly. On 2.1.287 an `Agent` call with
	# no `run_in_background` field launched asynchronously, so the foreground
	# arm must ask for `false`. Whether the model asked as told, and how Claude
	# Code ran it, is gate 8's question, below.
	case "$arm" in
	foreground) mode=false ;;
	*) mode=true ;;
	esac
	prompt="Use the Agent tool with run_in_background set to $mode to delegate to the $agent subagent. Do not answer yourself."

	# `--strict-mcp-config` excludes every MCP server but `fx`, and `-p` with
	# `--mcp-config` waits for it to connect before the first turn. `--allowedTools
	# Agent` lets the delegation run without a permission grant. The prompt must
	# not contain the marker: a marked main-thread request would read as governed.
	# In `-p`, a session that starts a background subagent stays open until it
	# completes, so the background arm's subagent requests reach the sink.
	probe_claude_arm "$ws" "$arm" "$package/fixtures/$arm" "$prompt" \
		--mcp-config "$ws/$arm/mcp.json" --strict-mcp-config --allowedTools Agent \
		--effort low || probe_fail
done

# Without a connected server, an arm's tool set says nothing about MCP grants:
# an empty `foreground` would read as "the allowlist excluded fx" when fx never
# existed. Runner-local, because it tests a file's existence and has no
# projection logic for a fabricated view to exercise. The stamp shows only that
# fx answered at some point; `probe_claude_gate_mcp_connected` below shows it
# had connected before the delegation.
for arm in "${arm_names[@]}"; do
	if [ ! -e "$ws/$arm/fx.stamp" ]; then
		printf 'probe: arm %s: the fx server never answered tools/list, so its tool set describes nothing.\n' "$arm" >&2
		probe_fail
	fi
done

probe_claude_assemble_view "$ws" "$ws/view.json" "${arm_names[@]}" || probe_fail
probe_claude_gate_marker "$ws/view.json" "$PROBE_MARKER" || probe_fail
probe_claude_gate_model "$ws/view.json" "$PROBE_MARKER" "$PROBE_CLAUDE_MODEL" || probe_fail
probe_claude_gate_mcp_connected "$ws/view.json" "$PROBE_MARKER" mcp__fx__alpha mcp__fx__beta || probe_fail
probe_claude_gate_delegation_background "$ws" foreground false || probe_fail
probe_claude_gate_delegation_background "$ws" background true || probe_fail
probe_claude_gate_delegation_background "$ws" background_inherit true || probe_fail

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
