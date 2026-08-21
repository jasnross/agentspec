#!/usr/bin/env bash
# Run every probe a batch run is allowed to execute, and record each.
#
# A package is skipped for one of two reasons, and the summary reports them
# separately: a `manual` package needs a live provider session, and a `billed`
# one spends model quota, so it runs only under an explicit opt-in. A directory
# with no `probe.json` is passed over silently. Every probe package carries one,
# so that guards a package being authored mid-session rather than describing a
# supported state.
#
# Probes execute sequentially. At this scale, obvious ordering when something
# breaks is worth more than the saved wall time.
set -uo pipefail

experiments_dir="${PROBE_EXPERIMENTS_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

if ! command -v jq >/dev/null 2>&1; then
	printf 'probe-run: jq is not installed\n' >&2
	exit 1
fi

allow_live="${PROBE_ALLOW_LIVE:-0}"
while [ $# -gt 0 ]; do
	case "$1" in
	--live) allow_live=1 ;;
	*)
		printf 'probe-run: unknown argument: %s (only --live is accepted)\n' "$1" >&2
		exit 2
		;;
	esac
	shift
done

ran=0
skipped_manual=0
skipped_billed=0
failed=""

# Shared by the `unattended` and `billed` arms: they differ in what gates
# reaching this point, not in what running a package means.
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
	case "$driver" in
	unattended)
		run_package "$package" "$name" unattended
		;;
	billed)
		# String comparison, not `-eq`: `PROBE_ALLOW_LIVE` is user-facing, and
		# `-eq` on a non-integer leaks a raw shell diagnostic. Anything that is
		# not exactly `1` fails safe by skipping.
		if [ "$allow_live" = 1 ]; then
			run_package "$package" "$name" billed
		else
			printf 'skipped  %-32s driver billed — needs credentials and a billed model call; run with --live\n' \
				"$name"
			skipped_billed=$((skipped_billed + 1))
		fi
		;;
	manual)
		printf 'skipped  %-32s driver manual — run it directly, then see %s/README.md\n' \
			"$name" "$name"
		skipped_manual=$((skipped_manual + 1))
		;;
	*)
		printf 'probe-run: %s declares an unknown driver: %s\n' "$name" "$driver" >&2
		failed="${failed:+$failed }$name"
		;;
	esac
done

printf '\nprobe-run: %s ran · %s skipped (%s manual · %s billed)\n' \
	"$ran" "$((skipped_manual + skipped_billed))" "$skipped_manual" "$skipped_billed"

if [ -n "$failed" ]; then
	printf 'probe-run: failed: %s\n' "$failed" >&2
	exit 1
fi

exit 0
