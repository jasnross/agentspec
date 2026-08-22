#!/usr/bin/env bash
# When a skill is model-invoked mid-session, supplied as the session's entry
# prompt, or forked, does Claude Code apply its `effort:` frontmatter to the
# outbound model requests it governs?
#
# Three arms, one per path this package measures. A fourth path — a slash
# command typed into an already-running interactive session — is not reachable
# from `claude -p`, which is one turn; see the README and `TODO.md`. Every arm
# spends a billed model call, which is why this package declares `driver: billed`
# and `just probe-run` withholds it without `--billed`.
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

# Validated rather than interpolated blind: it names directories under
# fixtures/, so an unchecked value is a path traversal. This package
# interpolates it into *two* paths — `$fixture` and `$fixture-fork` — so the
# validation matters more here, not less.
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

# Distinct from `claude-agent-effort`'s marker, so a stray workspace from one
# package cannot satisfy the other's gate.
PROBE_MARKER=AGENTSPEC-PROBE-MARKER-K2M9RT

ws=$(probe_workspace_create claude-skill-effort)

# Every library helper returns rather than exits, so the runner owns the exit —
# and one wrapper means the workspace-kept diagnostic is written once instead of
# at each call site.
probe_fail() {
	printf 'probe: the workspace has been kept for inspection: %s\n' "$ws" >&2
	exit 1
}

# `inline` is the path a model-invoked skill takes: Claude chooses the skill
# itself, mid-session, through the Skill tool.
probe_claude_arm "$ws" inline "$package/fixtures/$fixture" \
	'Run the agentspec effort probe check.' \
	--allowedTools Skill || probe_fail

# `slash_entry`, not `slash`: this measures a slash command supplied as the
# session's entry prompt. A slash command typed mid-session is routed through
# the Skill tool instead, which is the `inline` path — see the README.
probe_claude_arm "$ws" slash_entry "$package/fixtures/$fixture" \
	'/probe-effort run the check' || probe_fail

# `fork` uses the `context: fork` tree, so the skill runs in a forked subagent
# and returns to a main thread still at the ungoverned level.
probe_claude_arm "$ws" fork "$package/fixtures/$fixture-fork" \
	'Run the agentspec effort probe check.' \
	--allowedTools Skill || probe_fail

probe_claude_assemble_view "$ws" "$ws/view.json" inline slash_entry fork || probe_fail

# `.messages`, not the `.system` default `claude-agent-effort` takes. An agent
# file becomes the system prompt of the request it governs; a skill's body never
# reaches `.system` at all — measured at 2.1.232 it arrives in `messages[]`, as
# a `tool_result` block on the `inline` path and as `messages[0]` text on
# `slash_entry` and `fork`. The projection in `probe.json` selects on the same
# field, so gate and assertion agree on what this fixture governs.
probe_claude_gate_marker "$ws/view.json" "$PROBE_MARKER" .messages || probe_fail
probe_claude_gate_control "$ws/view.json" "$PROBE_MARKER" .messages || probe_fail

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
