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

# shellcheck source=experiments/lib/probe-state.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/probe-state.sh"

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
	record=$(probe_newest_record "${package%/}")
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

	probe_resolve_installed_version "${package%/}/probe.json"
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
