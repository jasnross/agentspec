#!/usr/bin/env bash
# Derive a report from the committed records. Invokes no probe.
#
# Everything printed comes from `experiments/*/results/*.json` plus the
# `version_source` each package's own manifest declares. There is no list of
# providers and no list of renderings that ought to be verified: the report
# describes what was measured, and a package with no records has no row.
#
# The one exception is a `billed` package, which gets a `not yet run` row from
# its manifest. No batch run will ever populate it, so "absent from the report"
# would be its permanent resting state rather than a transient one.
#
# Exits 0 unconditionally. This hangs off `just check`, and a check that can
# fail the build gets muted within a week.
set -uo pipefail

summary_only=0
case "${1:-}" in
--summary) summary_only=1 ;;
'') ;;
*)
	printf 'probe-status: unknown argument: %s\n' "$1" >&2
	exit 0
	;;
esac

experiments_dir="${PROBE_EXPERIMENTS_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

if ! command -v jq >/dev/null 2>&1; then
	printf 'probes: jq not installed — no status available\n'
	exit 0
fi

# Bound a version command so a wedged one cannot hang `just check` forever.
# A wedged build is worse than a failed one, which is the property this script
# exists to guarantee. `timeout` is GNU; macOS carries it only as `gtimeout`
# via coreutils, so its absence degrades to running unbounded rather than to
# skipping the command.
VERSION_COMMAND_TIMEOUT=10
if command -v timeout >/dev/null 2>&1; then
	timeout_prefix="timeout $VERSION_COMMAND_TIMEOUT"
elif command -v gtimeout >/dev/null 2>&1; then
	timeout_prefix="gtimeout $VERSION_COMMAND_TIMEOUT"
else
	timeout_prefix=""
fi

# Resolve the installed version for a package into two values:
#
#   installed_display     what to show, or empty for "say nothing"
#   installed_comparable  1 when it can be compared against the recorded
#                         version, 0 when no comparison is possible
#
# The flag is separate from the string on purpose. Sniffing the display text
# for a sentinel means a real version that happens to start with the sentinel
# word is silently treated as incomparable, under-counting genuine staleness.
resolve_installed_version() {
	local manifest="$1"
	installed_display=""
	installed_comparable=0

	[ -f "$manifest" ] || return 0

	local kind
	if ! kind=$(jq -er '.version_source.kind // "none"' "$manifest" 2>/dev/null); then
		# A broken manifest must not be indistinguishable from one that
		# declares no version source.
		installed_display="manifest unreadable"
		return 0
	fi

	case "$kind" in
	command)
		local command_line resolved
		local -a argv
		command_line=$(jq -r '.version_source.command // ""' "$manifest" 2>/dev/null)
		if [ -z "$command_line" ]; then
			installed_display="manifest declares no command"
			return 0
		fi
		# Split into an argv array and exec directly — never through a shell.
		# `just check` runs this on every invocation, and a manifest is a data
		# file reviewed as data; `eval` here would make it arbitrary code.
		# The cost is that a version command cannot use shell quoting or
		# metacharacters, which no real one needs.
		read -r -a argv <<<"$command_line"
		if ! command -v "${argv[0]}" >/dev/null 2>&1; then
			installed_display="not installed"
			return 0
		fi
		# stdin is closed: a command that reads it would otherwise block the
		# report on a terminal nobody is watching.
		# shellcheck disable=SC2086 # timeout_prefix is a deliberate word-split command prefix
		resolved=$($timeout_prefix "${argv[@]}" </dev/null 2>/dev/null | sed -n '1p' || true)
		if [ -z "$resolved" ]; then
			installed_display="version command produced nothing"
			return 0
		fi
		installed_display="$resolved"
		installed_comparable=1
		;;
	capture)
		# The common Cursor case: the IDE's version arrives only inside a
		# payload, so it is knowable by running the probe and by no other
		# means. This script runs no probe, so it has nothing to compare.
		installed_display="from capture"
		;;
	none)
		installed_display="human check"
		;;
	*)
		installed_display="unrecognized version_source kind"
		;;
	esac
}

newest_record() {
	# The filename carries a UTC date and time, so the shell's lexical glob
	# order is run order and the last match is the newest. The glob is
	# constrained to the stamped shape so an unrelated .json dropped into
	# results/ cannot sort after every record and be taken for the newest.
	local newest="" candidate
	for candidate in "$1"/results/[0-9]*T[0-9]*-*.json; do
		[ -e "$candidate" ] || continue
		newest="$candidate"
	done
	printf '%s' "$newest"
}

recorded=0
refuted=0
inconclusive=0
version_drift=0
rows=""
# Required rather than optional: this script runs under `set -u`, and the
# billed scan below reads it before anything else has assigned it. Matching
# against the tab-delimited `rows` string instead would depend on literal tabs
# inside a `case` pattern, which is fragile to edit.
charted=""

for package in "$experiments_dir"/*/; do
	[ -d "$package/results" ] || continue
	record=$(newest_record "${package%/}")
	[ -n "$record" ] || continue

	name=$(basename "${package%/}")
	# Charted before the record is parsed, not after. A package holding an
	# unreadable record has still been run, and the billed scan below would
	# otherwise label it `not yet run` — a false claim about a package that has
	# a record, rather than the mere invisibility an unparseable record earns
	# every other package.
	charted="${charted:+$charted }$name"

	jq -e . "$record" >/dev/null 2>&1 || continue

	provider=$(jq -r '.provider // "unknown"' "$record")
	record_status=$(jq -r '.status // "unknown"' "$record")
	depth=$(jq -r 'if .depth == null then "—" else .depth end' "$record")
	date=$(jq -r '.date // "—"' "$record")
	recorded_version=$(jq -r 'if .tool_version == null then "—" else .tool_version end' "$record")

	resolve_installed_version "${package%/}/probe.json"
	package_driver=$(jq -r '.driver // ""' "${package%/}/probe.json" 2>/dev/null || printf '')

	# Annotate the recorded version with what is installed now. Only a
	# comparable-and-different version is staleness; every other annotation
	# says why no comparison was possible.
	version_note=""
	if [ -n "$installed_display" ]; then
		if [ "$installed_comparable" -eq 1 ]; then
			if [ "$installed_display" != "$recorded_version" ]; then
				if [ "$package_driver" = "billed" ]; then
					# A billed package is never refreshed by a batch run, so its
					# drift is permanent and clearable only by a paid `--live`
					# run. Counting it would make the drift total structurally
					# nonzero, hiding the genuine drift of packages that a batch
					# run can clear.
					version_note=" (installed $installed_display · refresh: just probe-run --live)"
				else
					version_note=" (installed $installed_display)"
					version_drift=$((version_drift + 1))
				fi
			fi
		else
			version_note=" ($installed_display)"
		fi
	fi

	# `inconclusive` must not read as a pass: it is what a human-judged probe
	# produces when the operator could not tell.
	case "$record_status" in
	confirmed) label="confirmed" ;;
	refuted) label="REFUTED" ;;
	inconclusive) label="INCONCLUSIVE" ;;
	*) label="$record_status" ;;
	esac

	recorded=$((recorded + 1))
	[ "$record_status" = "refuted" ] && refuted=$((refuted + 1))
	[ "$record_status" = "inconclusive" ] && inconclusive=$((inconclusive + 1))

	rows="${rows}${provider}"$'\t'"${name}"$'\t'"${label}"$'\t'"${depth}"$'\t'"${date}"$'\t'"${recorded_version}${version_note}"$'\n'
done

# Counted from the manifests rather than from the records, because a billed
# package's most likely state is declared-and-never-run — and the record loop
# passes over a package with no results/ directory entirely.
billed=0
for manifest in "$experiments_dir"/*/probe.json; do
	[ -e "$manifest" ] || continue
	[ "$(jq -r '.driver // ""' "$manifest" 2>/dev/null)" = "billed" ] || continue
	billed=$((billed + 1))

	name=$(basename "$(dirname "$manifest")")
	case " $charted " in
	*" $name "*) continue ;;
	esac

	provider=$(jq -r '.provider // "unknown"' "$manifest")
	depth=$(jq -r 'if .depth == null then "—" else .depth end' "$manifest")
	rows="${rows}${provider}"$'\t'"${name}"$'\t'"not yet run"$'\t'"${depth}"$'\t'"—"$'\t'"— (billed; run: just probe-run --live)"$'\n'
done

summary_line() {
	printf 'probes: %s recorded · %s refuted · %s inconclusive · %s version drift' \
		"$recorded" "$refuted" "$inconclusive" "$version_drift"
	if [ "$billed" -gt 0 ]; then
		printf ' · %s billed (drift not tracked)' "$billed"
	fi
	printf '\n'
}

if [ "$summary_only" -eq 1 ]; then
	summary_line
	exit 0
fi

# `billed` is part of the guard, not just `rows`: a billed package whose only
# record is unreadable emits no row and increments no count, so without it the
# report would announce an empty tree while a declared billed package sits in
# it uncounted.
if [ "$recorded" -eq 0 ] && [ -z "$rows" ] && [ "$billed" -eq 0 ]; then
	printf 'No probe records. Write a probe: see experiments/README.md\n'
	exit 0
fi

printf '%s' "$rows" | sort | awk -F'\t' '
	$1 != current { current = $1; printf "\n%s\n", toupper($1) }
	{ printf "  %-32s %-13s %-17s %-12s %s\n", $2, $3, $4, $5, $6 }
'
printf '\n'
summary_line

exit 0
