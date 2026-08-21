#!/usr/bin/env bash
# On which invocation paths does Claude Code apply an agent's `effort:`
# frontmatter to the outbound model request?
#
# Two arms, one per path the same agent file can be reached by: `--agent` makes
# it the session agent, and the Task tool delegates to it as a subagent. Both
# spend a billed model call, which is why this package declares `driver: billed`
# and `just probe-run` skips it without `--live`.
set -euo pipefail

package=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$package/../lib/probe-common.sh"
# shellcheck source=experiments/lib/probe-claude-otel.sh
. "$package/../lib/probe-claude-otel.sh"

probe_require_tools jq claude

if [ $# -ne 0 ]; then
	printf 'probe: this runner takes no arguments (got: %s)\n' "$*" >&2
	printf 'probe: set PROBE_FIXTURE=discriminator or PROBE_DRY_RUN=1 in the environment instead.\n' >&2
	exit 2
fi

# Validated rather than interpolated blind: it names a directory under
# fixtures/, so an unchecked value is a path traversal.
fixture="${PROBE_FIXTURE:-assertion}"
case "$fixture" in
assertion | discriminator) ;;
*)
	printf 'probe: PROBE_FIXTURE must be assertion or discriminator (got: %s)\n' "$fixture" >&2
	exit 2
	;;
esac
dry_run="${PROBE_DRY_RUN:-0}"
case "$dry_run" in
0 | 1) ;;
*)
	printf 'probe: PROBE_DRY_RUN must be 0 or 1 (got: %s)\n' "$dry_run" >&2
	exit 2
	;;
esac

# The discriminator fixture exists to show the assertion discriminates, and its
# run is *expected* to refute. Recording that refutation would drop the harness's
# strong signal — "provider behavior changed" — into an append-only result set
# on the strength of a fixture chosen to disagree, where it would raise a
# permanent false alarm that costs a billed run to disprove. One forgotten
# environment variable is all that separates the two, so the runner refuses
# rather than trusting the operator to remember.
if [ "$fixture" = discriminator ] && [ "$dry_run" != 1 ]; then
	printf 'probe: the discriminator fixture is dry-run only; set PROBE_DRY_RUN=1\n' >&2
	printf 'probe: recording its run would commit a refuted record for a fixture built to disagree.\n' >&2
	exit 2
fi

# The two constants `probe-claude-otel.sh` reads out of this shell. They are
# globals rather than arguments because every arm shares them, and threading
# them through each call would invite one arm running at a different model than
# its siblings — which would make the ungoverned control set incomparable
# across arms. shellcheck cannot follow the sourced library, so it sees them as
# unused; `manifest-contract.sh` carries the same disable for the same reason.
# shellcheck disable=SC2034

# The model is pinned to keep the value domain known. It must support `low`
# (the fixture's value) and `max` (the discriminator's). Nothing depends on its
# default effort: the assertion is relational.
PROBE_CLAUDE_MODEL=claude-opus-4-8

# `--max-budget-usd` is print-mode only and counts subagent spend. A cap hit
# stops subagent spawns, which surfaces as a gate-2 failure rather than as a
# wrong answer — loud, which is why this is generous rather than tight.
PROBE_CLAUDE_BUDGET_USD=0.50

PROBE_MARKER=AGENTSPEC-PROBE-MARKER-7Q4XKD

ws=$(probe_workspace_create claude-agent-effort)

# Every library helper returns rather than exits, so the runner owns the exit —
# and one wrapper means the workspace-kept diagnostic is written once instead of
# at each call site.
probe_fail() {
	printf 'probe: the workspace has been kept for inspection: %s\n' "$ws" >&2
	exit 1
}

probe_claude_arm "$ws" session_agent "$package/fixtures/$fixture" \
	'Reply with exactly one word: ok.' \
	--agent probe-effort || probe_fail

# `--allowedTools Task` so the delegation needs no permission grant. The prompt
# must not contain the marker: a marked main-thread request would be read as
# governed by the fixture.
probe_claude_arm "$ws" delegated "$package/fixtures/$fixture" \
	'Use the Task tool to delegate to the probe-effort subagent. Do not answer yourself.' \
	--allowedTools Task || probe_fail

probe_claude_assemble_view "$ws" "$ws/view.json" session_agent delegated || probe_fail
probe_claude_gate_marker "$ws/view.json" "$PROBE_MARKER" || probe_fail
probe_claude_gate_control "$ws/view.json" "$PROBE_MARKER" || probe_fail

# Printed before any recording branch: a failed or dry `record.sh` leaves the
# workspace, and this path is what the author iterates a candidate projection
# against for free.
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
