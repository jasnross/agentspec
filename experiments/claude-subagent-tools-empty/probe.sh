#!/usr/bin/env bash
# Does a Claude subagent whose `tools` field is an empty list receive no tools,
# or the same tools as a subagent with no `tools` field?
#
# Three arms, each delegating to an agent that differs from its siblings only in
# `tools`. Every arm spends a billed model call, which is why this package
# declares `driver: billed` and `just probe-run` withholds it without
# `--billed`.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"
# shellcheck source=experiments/lib/probe-claude-otel.sh
. "$package/../lib/probe-claude-otel.sh"

probe_require_tools jq claude

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

# Pinned to Sonnet 5.5 at `--effort low`, as in `claude-subagent-mcp-tools`:
# on 2.1.287 Haiku did not delegate in any arm of that package, twice. See its
# README's "Model choice" section.
PROBE_CLAUDE_MODEL=claude-sonnet-5-5

# `--max-budget-usd` is print-mode only and counts subagent spend. A cap hit
# stops subagent spawns, which surfaces as a marker-gate failure rather than as
# a wrong answer.
PROBE_CLAUDE_BUDGET_USD=0.50

PROBE_MARKER=AGENTSPEC-PROBE-MARKER-CCEMPTY8

# Each would change what is measured: whether tool search is on
# (`ENABLE_TOOL_SEARCH`, a non-Anthropic `ANTHROPIC_BASE_URL`,
# `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`), which decides whether a tool can
# reach the model through the deferred listing, or which model the subagent
# runs (`CLAUDE_CODE_SUBAGENT_MODEL`, `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`).
unset ENABLE_TOOL_SEARCH ANTHROPIC_BASE_URL CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS \
	CLAUDE_CODE_SUBAGENT_MODEL CLAUDE_CODE_SUBAGENT_MODEL_FORCE

# Arm name, then the agent its prompt delegates to. Each arm's fixture tree
# holds only its own agent, so a delegation to the wrong name finds nothing.
ARMS=(
	inherit:probe-tools-inherit
	empty:probe-tools-empty
	read:probe-tools-read
)

ws=$(probe_workspace_create claude-subagent-tools-empty)

# Every library helper returns rather than exits, so the runner owns the exit.
probe_fail() {
	printf 'probe: the workspace has been kept for inspection: %s\n' "$ws" >&2
	exit 1
}

arm_names=()
for pair in "${ARMS[@]}"; do
	arm=${pair%%:*}
	agent=${pair#*:}
	arm_names+=("$arm")

	# `--strict-mcp-config` with no `--mcp-config` loads no MCP server at all,
	# which keeps the operator's own servers out of the view and the record;
	# `--setting-sources project` governs settings, not MCP servers.
	# `--allowedTools Agent` lets the delegation run without a permission grant.
	# The prompt must not contain the marker: a marked main-thread request would
	# read as governed.
	probe_claude_arm "$ws" "$arm" "$package/fixtures/$arm" \
		"Use the Agent tool to delegate to the $agent subagent. Do not answer yourself." \
		--strict-mcp-config --allowedTools Agent \
		--effort low || probe_fail
done

probe_claude_assemble_view "$ws" "$ws/view.json" "${arm_names[@]}" || probe_fail
probe_claude_gate_marker "$ws/view.json" "$PROBE_MARKER" || probe_fail
probe_claude_gate_model "$ws/view.json" "$PROBE_MARKER" "$PROBE_CLAUDE_MODEL" || probe_fail

# An unrestricted subagent gets built-ins. Without this, a `no-tools` value for
# `empty` could also mean every subagent in the run was offered nothing — a
# delegation path that strips tools, say — rather than the empty list
# restricting it. Runner-local, because only this package has an `inherit` arm
# whose built-ins are the precondition.
if ! jq -e --arg m "$PROBE_MARKER" '
	[.inherit[] | select(.system | tostring | contains($m))]
	| length > 0 and all(.[]; any(.tools[]?; .name == "Read"))
' "$ws/view.json" >/dev/null; then
	printf 'probe: arm inherit: a governed request offered no Read in tools[].\n' >&2
	printf 'probe: an unrestricted subagent did not receive built-ins, so an empty tool set elsewhere describes nothing.\n' >&2
	printf 'probe: this is a statement about the run, not about Claude. No record written.\n' >&2
	probe_fail
fi

# A non-empty allowlist restricts in this run: `read` is offered the tool it
# lists and fewer tools than `inherit`. Without it, a `same-as-inherit` value
# for `empty` could mean this run ignored every subagent's `tools` field, not
# that Claude reads the empty list as unrestricted.
if ! jq -e --arg m "$PROBE_MARKER" '
	def governed: [.[] | select(.system | tostring | contains($m))];
	(.read | governed) as $r | (.inherit | governed) as $i
	| ($r | length > 0)
	and all($r[]; any(.tools[]?; .name == "Read"))
	and (([$r[] | .tools[]?.name] | unique | length) < ([$i[] | .tools[]?.name] | unique | length))
' "$ws/view.json" >/dev/null; then
	printf 'probe: arm read: the governed requests did not offer Read with fewer tools than arm inherit.\n' >&2
	printf 'probe: a listed tools field did not restrict the subagent, so the value for empty cannot be read.\n' >&2
	printf 'probe: this is a statement about the run, not about Claude. No record written.\n' >&2
	probe_fail
fi

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
