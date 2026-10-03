#!/usr/bin/env bash
# Under fork mode (`CLAUDE_CODE_FORK_SUBAGENT=1`, `-p`), is a Claude subagent
# whose `tools` is `Read` offered a connected MCP server's tools, where a
# subagent with no `tools` field is offered all of them?
#
# Two arms. `fork_restricted` delegates to an agent with `tools: Read`;
# `fork_inherit` delegates to an agent with no `tools` field, so the run shows a
# subagent under fork mode can be offered `fx` at all. Every arm spends a billed
# model call, which is why this package declares `driver: billed` and
# `just probe-run` withholds it without `--billed`.
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

PROBE_MARKER=AGENTSPEC-PROBE-MARKER-CCFORK8

# Each would change what is measured: whether tool search is on
# (`ENABLE_TOOL_SEARCH`, a non-Anthropic `ANTHROPIC_BASE_URL`,
# `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`), whether the MCP server has
# connected when the subagent spawns (`CLAUDE_CODE_MCP_STARTUP_WAIT_MS`), or
# which model the subagent runs (`CLAUDE_CODE_SUBAGENT_MODEL`,
# `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`). `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`
# changes how long `-p` waits for the background subagent before exiting, and
# `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` runs every subagent in the
# foreground even under fork mode.
unset ENABLE_TOOL_SEARCH ANTHROPIC_BASE_URL CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS \
	CLAUDE_CODE_MCP_STARTUP_WAIT_MS CLAUDE_CODE_SUBAGENT_MODEL CLAUDE_CODE_SUBAGENT_MODEL_FORCE \
	CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS CLAUDE_CODE_DISABLE_BACKGROUND_TASKS

# `-p` leaves fork mode off by default; `1` turns it on there.
export CLAUDE_CODE_FORK_SUBAGENT=1

# Arm name, then the agent its prompt delegates to. Each arm's fixture tree
# holds only its own agent, so a delegation to the wrong name finds nothing.
ARMS=(
	fork_restricted:probe-fork-restricted
	fork_inherit:probe-fork-inherit
)

ws=$(probe_workspace_create claude-fork-subagent-mcp-tools)

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

	# Fork mode removes the `Agent` tool's `run_in_background` parameter, so the
	# prompt names no mode. Whether fork mode was on, and the subagent ran in
	# the background, is gate 10's question, below.
	prompt="Use the Agent tool to delegate to the $agent subagent. Do not answer yourself."

	# `--strict-mcp-config` excludes every MCP server but `fx`, and `-p` with
	# `--mcp-config` waits for it to connect before the first turn. `--allowedTools
	# Agent` lets the delegation run without a permission grant. The prompt must
	# not contain the marker: a marked main-thread request would read as governed.
	# In `-p`, a session that starts a background subagent stays open until it
	# completes, so the subagent's requests reach the sink.
	probe_claude_arm "$ws" "$arm" "$package/fixtures/$arm" "$prompt" \
		--mcp-config "$ws/$arm/mcp.json" --strict-mcp-config --allowedTools Agent \
		--effort low || probe_fail
done

# Without a connected server, an arm's tool set says nothing about MCP grants:
# an empty `fork_restricted` would read as "the allowlist excluded fx" when fx
# never existed. Runner-local, because it tests a file's existence and has no
# projection logic for a fabricated view to exercise. The stamp shows only
# that fx answered at some point; `probe_claude_gate_mcp_connected` below shows
# it had connected before the delegation.
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
for arm in "${arm_names[@]}"; do
	probe_claude_gate_delegation_fork "$ws" "$arm" || probe_fail
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
