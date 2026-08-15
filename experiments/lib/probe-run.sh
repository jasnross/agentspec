#!/usr/bin/env bash
# Run every script-driven probe and record each.
#
# Human-driven packages are listed as skipped with a pointer to their README —
# they need a live provider session, so a batch runner cannot drive them. A
# package with no `probe.json` is passed over silently: it has no runnable
# assertion and nothing to run.
#
# Probes execute sequentially. At this scale, obvious ordering when something
# breaks is worth more than the saved wall time.
set -uo pipefail

experiments_dir="${PROBE_EXPERIMENTS_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

if ! command -v jq >/dev/null 2>&1; then
	printf 'probe-run: jq is not installed\n' >&2
	exit 1
fi

ran=0
skipped=0
failed=""

for manifest in "$experiments_dir"/*/probe.json; do
	[ -e "$manifest" ] || continue
	package=$(dirname "$manifest")
	name=$(basename "$package")

	driver=$(jq -r '.driver // ""' "$manifest" 2>/dev/null)
	case "$driver" in
	script)
		printf '\n=== %s ===\n' "$name"
		if [ ! -x "$package/probe.sh" ]; then
			printf 'probe-run: %s declares driver "script" but has no executable probe.sh\n' "$name" >&2
			failed="${failed:+$failed }$name"
			continue
		fi
		# A failing probe must not abort the loop: records written by probes
		# that passed stay valid, and the run reports what broke at the end.
		if "$package/probe.sh"; then
			ran=$((ran + 1))
		else
			printf 'probe-run: %s FAILED\n' "$name" >&2
			failed="${failed:+$failed }$name"
		fi
		;;
	human-act | human-judge)
		printf 'skipped  %-32s driver %s — run it directly, then see %s/README.md\n' \
			"$name" "$driver" "$name"
		skipped=$((skipped + 1))
		;;
	*)
		printf 'probe-run: %s declares an unknown driver: %s\n' "$name" "$driver" >&2
		failed="${failed:+$failed }$name"
		;;
	esac
done

printf '\nprobe-run: %s ran · %s skipped (human-driven)\n' "$ran" "$skipped"

if [ -n "$failed" ]; then
	printf 'probe-run: failed: %s\n' "$failed" >&2
	exit 1
fi

exit 0
