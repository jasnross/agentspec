#!/usr/bin/env bash
# Run the probes this invocation has authorized, and record each.
#
# Every probe package is runnable by this script — including the `manual` ones,
# which block through a live provider session via `probe_human_run`. What
# separates them is not capability but cost: an `unattended` probe costs
# nothing, a `billed` one spends model quota, and a `manual` one spends a
# person's afternoon. So a run is defined by what it is willing to spend.
#
# Two orthogonal questions decide each package, in this order:
#
#   selection      is this package interesting to this run?  (`--stale`)
#   authorization  may this run pay what the package costs?  (`--billed`,
#                  `--manual`, `--all`)
#
# Authorization is a *set* of drivers, seeded with the free one and added to by
# each flag, so the flags stack in any order and a fourth driver costs one flag
# rather than a new combination. Selection is a filter over packages and knows
# nothing about drivers; keeping them separate is what lets `--stale --billed`
# mean the obvious thing.
#
# A directory with no `probe.json` is passed over silently. Every probe package
# carries one, so that guards a package being authored mid-session rather than
# describing a supported state.
#
# Probes execute sequentially. At this scale, obvious ordering when something
# breaks is worth more than the saved wall time.
set -uo pipefail

lib_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
experiments_dir="${PROBE_EXPERIMENTS_DIR:-$(cd "$lib_dir/.." && pwd)}"

if ! command -v jq >/dev/null 2>&1; then
	printf 'probe-run: jq is not installed\n' >&2
	exit 1
fi

# shellcheck source=experiments/lib/manifest-contract.sh
. "$lib_dir/manifest-contract.sh"
# shellcheck source=experiments/lib/probe-state.sh
. "$lib_dir/probe-state.sh"

# Set membership over a space-delimited string. The tree is eight packages and
# the driver enum is three values, so a real associative array would be
# machinery bought for nothing.
contains() {
	case " $1 " in
	*" $2 "*) return 0 ;;
	*) return 1 ;;
	esac
}

authorized="$MANIFEST_DRIVER_FREE"
authorize() {
	contains "$authorized" "$1" || authorized="$authorized $1"
}

# The environment form exists so a caller that cannot pass arguments can still
# opt in. It seeds the same set the flags add to, rather than being a second
# mechanism the flags have to agree with.
for driver in ${PROBE_AUTHORIZE_DRIVERS:-}; do
	if ! contains "$MANIFEST_DRIVERS" "$driver"; then
		printf 'probe-run: PROBE_AUTHORIZE_DRIVERS names an unknown driver: %s\n' "$driver" >&2
		exit 2
	fi
	authorize "$driver"
done

stale_only=0
while [ $# -gt 0 ]; do
	case "$1" in
	--billed) authorize billed ;;
	--manual) authorize manual ;;
	--all) authorized="$MANIFEST_DRIVERS" ;;
	--stale) stale_only=1 ;;
	*)
		printf 'probe-run: unknown argument: %s\n' "$1" >&2
		printf 'probe-run: accepted: --billed --manual --all --stale\n' >&2
		exit 2
		;;
	esac
	shift
done

# What a withheld package costs, and what it is waiting on. Phrased as the flag
# to type, because that is the only thing the reader can act on. `unattended`
# has no entry: it is never withheld.
#
# The manual arm keeps pointing at the package README even though `--manual`
# now runs the probe, because the flag starts the session and the README is
# where the procedure the operator has to follow during it lives.
withheld_note() {
	local driver="$1" name="$2"
	case "$driver" in
	billed) printf 'spends model quota; authorize with --billed' ;;
	manual) printf 'needs a live session; authorize with --manual, following %s/README.md' "$name" ;;
	*) printf 'authorize with --all' ;;
	esac
}

ran=0
fresh=0
# Parallel to `MANIFEST_DRIVERS`: one token appended per withheld package, so
# the summary counts by driver without a variable named after each one.
withheld=""
failed=""

run_package() {
	local package="$1" name="$2" driver="$3"

	printf '\n=== %s ===\n' "$name"
	if [ ! -x "$package/probe.sh" ]; then
		printf 'probe-run: %s declares driver "%s" but has no executable probe.sh\n' \
			"$name" "$driver" >&2
		failed="${failed:+$failed }$name"
		return
	fi
	# A failing probe must not abort the loop: records written by probes
	# that passed stay valid, and the run reports what broke at the end.
	if "$package/probe.sh"; then
		ran=$((ran + 1))
	else
		printf 'probe-run: %s FAILED\n' "$name" >&2
		failed="${failed:+$failed }$name"
	fi
}

for manifest in "$experiments_dir"/*/probe.json; do
	[ -e "$manifest" ] || continue
	package=$(dirname "$manifest")
	name=$(basename "$package")

	driver=$(jq -r '.driver // ""' "$manifest" 2>/dev/null)
	if ! contains "$MANIFEST_DRIVERS" "$driver"; then
		printf 'probe-run: %s declares an unknown driver: %s\n' "$name" "$driver" >&2
		failed="${failed:+$failed }$name"
		continue
	fi

	# Selection before authorization, so a fresh package is passed over
	# whatever it would have cost. A `--stale` run that still told you to go
	# authorize a probe whose answer cannot have changed would be filtering
	# nothing that matters.
	if [ "$stale_only" = 1 ] && ! probe_needs_run "$package"; then
		printf 'fresh    %-32s %s\n' "$name" "$probe_state_reason"
		fresh=$((fresh + 1))
		continue
	fi

	if contains "$authorized" "$driver"; then
		run_package "$package" "$name" "$driver"
	else
		printf 'skipped  %-32s driver %s — %s\n' "$name" "$driver" "$(withheld_note "$driver" "$name")"
		withheld="${withheld:+$withheld }$driver"
	fi
done

# One segment per withheld driver, generated from the enum rather than from a
# hand-written list, so the breakdown cannot fall out of step with the drivers
# that exist. `unattended` is skipped: it is always authorized, so a segment for
# it would be a structural zero.
breakdown=""
withheld_total=0
for driver in $MANIFEST_DRIVERS; do
	[ "$driver" = "$MANIFEST_DRIVER_FREE" ] && continue
	count=0
	for entry in $withheld; do
		[ "$entry" = "$driver" ] && count=$((count + 1))
	done
	withheld_total=$((withheld_total + count))
	breakdown="${breakdown:+$breakdown · }$count need --$driver"
done

# The `fresh` segment appears only under `--stale`. Printing `0 fresh` on a run
# that never evaluated freshness would claim it had been evaluated and found
# nothing.
if [ "$stale_only" = 1 ]; then
	breakdown="${breakdown:+$breakdown · }$fresh fresh"
fi

printf '\nprobe-run: %s ran · %s skipped (%s)\n' \
	"$ran" "$((withheld_total + fresh))" "$breakdown"

if [ -n "$failed" ]; then
	printf 'probe-run: failed: %s\n' "$failed" >&2
	exit 1
fi

exit 0
