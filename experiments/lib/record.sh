#!/usr/bin/env bash
# Assert & record — the single writer of probe result records.
#
# Every record in this repository is produced by this script. There is one
# record shape and no hand-written exception, which is what lets `probe-status`
# join across packages and what makes hand-editing a record a defect rather
# than a workflow.
#
# Usage:
#   record.sh --manifest <probe.json> --view <view.json>      [--capture <ws>] [--dry-run]
#   record.sh --manifest <probe.json> --selection <option-id>  --capture <ws>  [--dry-run]
set -euo pipefail

lib_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=experiments/lib/probe-common.sh
. "$lib_dir/probe-common.sh"
# shellcheck source=experiments/lib/manifest-contract.sh
. "$lib_dir/manifest-contract.sh"

probe_require_tools jq

record_fail() {
	printf 'record: %s\n' "$1" >&2
	exit 1
}

manifest=""
view=""
selection=""
capture=""
dry_run=0

while [ $# -gt 0 ]; do
	case "$1" in
	--dry-run)
		dry_run=1
		shift
		;;
	--manifest | --view | --selection | --capture)
		[ $# -ge 2 ] || record_fail "$1 requires a value"
		case "$1" in
		--manifest) manifest="$2" ;;
		--view) view="$2" ;;
		--selection) selection="$2" ;;
		--capture) capture="$2" ;;
		esac
		shift 2
		;;
	*)
		record_fail "unknown argument: $1"
		;;
	esac
done

[ -n "$manifest" ] || record_fail "--manifest is required"
[ -f "$manifest" ] || record_fail "manifest not found: $manifest"
jq -e . "$manifest" >/dev/null 2>&1 || record_fail "manifest is not valid JSON: $manifest"

# Validate the manifest before anything reads a value out of it. A manifest is
# hand-authored, and the failures it can express are exactly the ones this
# harness exists to prevent: an absent `expected` compares `null == null` and
# records a vacuous `confirmed`, which is the misspelled-projection failure the
# contract claims to catch.
manifest_check() {
	jq -e "$1" "$manifest" >/dev/null 2>&1 || record_fail "$2: $manifest"
}
manifest_check '.schema_version == 1' 'manifest schema_version must be 1'
manifest_check '.provider | . == "claude" or . == "cursor" or . == "opencode"' \
	'manifest provider must be claude, cursor, or opencode'
manifest_check '.driver | . == "script" or . == "human-act" or . == "human-judge"' \
	'manifest driver must be script, human-act, or human-judge'
manifest_check '(.question | type) == "string" and (.question | length) > 0' \
	'manifest must declare a non-empty question'
manifest_check '.assertion | type == "object"' 'manifest must declare an assertion'
manifest_check '.assertion | has("expected")' 'manifest assertion declares no expected value'
manifest_check '.assertion | (has("projection") != has("options"))' \
	'manifest assertion must declare exactly one of projection or options'

# Both enums live in `manifest-contract.sh` so this gate and the bats suite
# cannot drift apart; the reasoning for each is there.
manifest_check "$MANIFEST_OPTION_STATUS_JQ" \
	'an option may declare only status "inconclusive"'
manifest_check ".depth | $MANIFEST_DEPTH_JQ" \
	'manifest depth must be resolved-config, outbound-request, or null'

# Status is always computed from the comparison, because a caller that could
# assert a status could assert one the evidence does not support. There is no
# `--status` override. The single exception is above: a selected option may
# declare `inconclusive`, and the gate narrows a declared status to that.
if [ -n "$view" ] && [ -n "$selection" ]; then
	record_fail "--view and --selection are mutually exclusive"
fi
if [ -z "$view" ] && [ -z "$selection" ]; then
	record_fail "exactly one of --view or --selection is required"
fi

# The invocation mode must match the assertion shape. Mismatched, a projection
# over an options manifest resolves to `null` and records `refuted` — and
# `refuted` is the strong signal, so an operator wiring error would raise a
# false "provider behavior changed" alarm.
if [ -n "$view" ]; then
	manifest_check '.assertion | has("projection")' \
		'--view needs a projection assertion; this manifest declares options (use --selection)'
else
	manifest_check '.assertion | has("options")' \
		'--selection needs an options assertion; this manifest declares a projection (use --view)'
fi

package=$(cd "$(dirname "$manifest")" && pwd)
results_dir="$package/results"

driver=$(jq -r '.driver // ""' "$manifest")
case "$driver" in
human-act | human-judge)
	[ -n "$capture" ] || record_fail "driver \"$driver\" requires --capture"
	;;
esac

# The capture directory has two jobs left: it is the version source for a
# manifest declaring `version_source.kind: "capture"`, and it is what the
# non-empty check below reads. That check targets the file the *provider* wrote
# and never `--view`, which the runner writes moments before this script runs.
#
# A hook that never fired is the failure this catches, and it needs no stamp:
# a runner is one blocking invocation, so the workspace was created by the
# invocation still running and there is no earlier run to point at.
if [ -n "$capture" ]; then
	payloads="$capture/capture/payloads.jsonl"
	[ -s "$payloads" ] || record_fail "capture payloads absent or empty: $payloads"
fi

expected=$(jq -c '.assertion.expected' "$manifest")

if [ -n "$selection" ]; then
	# Exactly one match: duplicate ids would make `option` multi-valued, and a
	# multi-line `status` would escape the confirmed/refuted/inconclusive enum.
	matches=$(jq --arg id "$selection" '[.assertion.options[]? | select(.id == $id)] | length' "$manifest")
	case "$matches" in
	0) record_fail "selection \"$selection\" is not one of the manifest's option ids" ;;
	1) ;;
	*) record_fail "manifest declares option id \"$selection\" $matches times" ;;
	esac
	option=$(jq -c --arg id "$selection" 'first(.assertion.options[] | select(.id == $id))' "$manifest")
	observed=$(jq -n --arg id "$selection" '$id')
	option_status=$(jq -r '.status // ""' <<<"$option")
else
	[ -f "$view" ] || record_fail "view not found: $view"
	projection=$(jq -r '.assertion.projection' "$manifest")
	observed=$(jq -c "$projection" "$view") || record_fail "projection failed: $projection"
	[ -n "$observed" ] || observed=null
	# A projection emitting more than one value cannot be compared structurally
	# against a single `expected`; say so rather than letting jq dump internals.
	case "$observed" in
	*"
"*)
		record_fail "projection must yield exactly one JSON value, but yielded several: $projection"
		;;
	esac
	option_status=""
fi

if [ -n "$option_status" ]; then
	# A `couldnt-tell` option declares `inconclusive`, so a tired operator
	# picking it cannot manufacture a pass.
	status="$option_status"
elif jq -e -n --argjson a "$expected" --argjson b "$observed" '$a == $b' >/dev/null; then
	# Structural comparison, never a string one: it is immune to key reordering
	# and whitespace, the instability class these providers' output exhibits.
	status=confirmed
else
	status=refuted
fi

version_kind=$(jq -r '.version_source.kind // "none"' "$manifest")
case "$version_kind" in
command)
	version_command=$(jq -r '.version_source.command' "$manifest")
	# Split into argv and exec directly rather than through a shell: a manifest
	# is reviewed as data, so it must not be able to express arbitrary code.
	# A version command therefore cannot use shell quoting or metacharacters.
	read -r -a version_argv <<<"$version_command"
	[ "${#version_argv[@]}" -gt 0 ] || record_fail "version_source declares an empty command"
	# `sed -n 1p` rather than `head -1`: sed reads its input to the end, so it
	# cannot SIGPIPE the producer out from under `pipefail`.
	tool_version=$("${version_argv[@]}" </dev/null 2>/dev/null | sed -n '1p' || true)
	;;
capture)
	[ -n "$capture" ] || record_fail 'version_source.kind "capture" requires --capture'
	version_jq=$(jq -r '.version_source.jq' "$manifest")
	# Slurped for the same reason `probe_wait_for_capture` slurps: every
	# declared capture expression is array-shaped, so without `-s` the version
	# resolves to nothing on every record.
	tool_version=$(jq -s -r "$version_jq" "$capture/capture/payloads.jsonl" 2>/dev/null || true)
	;;
none)
	tool_version=""
	;;
*)
	record_fail "unknown version_source.kind: $version_kind"
	;;
esac
if [ "$tool_version" = "null" ]; then tool_version=""; fi

# One UTC clock read feeds both the record's `date` and the filename. UTC keeps
# records written in different timezones sorting correctly; the time component
# is what stops a same-day re-run at the same version from overwriting its
# predecessor — the case that arises the moment someone re-runs a `refuted`
# probe to confirm it.
stamped=$(date -u +%FT%H%M%S)
record_date="${stamped%%T*}"

if [ -n "$selection" ]; then
	assertion_base=$(jq -c '{options: .assertion.options, expected: .assertion.expected}' "$manifest")
else
	assertion_base=$(jq -c '{projection: .assertion.projection, expected: .assertion.expected}' "$manifest")
fi
assertion=$(jq -c --argjson observed "$observed" '. + {observed: $observed}' <<<"$assertion_base")

# Seven keys, every one consumed by `probe-status`. No probe name, no driver, no
# capture provenance: a stored derivable fact is a fact that can disagree with
# its source.
record=$(jq -n \
	--argjson schema_version "$(jq -c '.schema_version' "$manifest")" \
	--arg provider "$(jq -r '.provider' "$manifest")" \
	--arg status "$status" \
	--argjson depth "$(jq -c '.depth' "$manifest")" \
	--arg date "$record_date" \
	--arg tool_version "$tool_version" \
	--argjson assertion "$assertion" \
	'{
		schema_version: $schema_version,
		provider: $provider,
		status: $status,
		depth: $depth,
		date: $date,
		tool_version: (if $tool_version == "" then null else $tool_version end),
		assertion: $assertion
	}')

if [ "$dry_run" -eq 1 ]; then
	# Not a record. It is what a record would contain, printed so a probe's
	# wiring can be checked before a live or billed run produces one. The
	# harness's central invariant is that every record is produced by a runner;
	# this exists to reduce the pressure to violate it, not to work around it.
	#
	# Exits 0 whatever the comparison computed: the discriminator run is
	# definitionally expected to refute, and refuted is a finding rather than a
	# failure on a real run too.
	printf '%s\n' "$record"
	printf 'record: dry run — %s — observed %s (no file written)\n' "$status" "$observed" >&2
	exit 0
fi

version_slug=$(printf '%s' "${tool_version:-unknown}" | tr -c 'A-Za-z0-9._-' '_')
record_path="$results_dir/${stamped}-$(jq -r '.provider' "$manifest")-${version_slug}.json"

mkdir -p "$results_dir"
printf '%s\n' "$record" >"$record_path"

printf '%s\n' "$record_path"
printf 'record: %s — observed %s\n' "$status" "$observed" >&2
