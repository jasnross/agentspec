#!/usr/bin/env bash
# Gate #21 capture hook: record the payload, then emit PLAIN text and exit 0.
#
# No JSON envelope — that absence is the whole experiment. The trap guarantees
# the marker is emitted even if the capture fails, so the probe still exercises
# what it is measuring.
set -uo pipefail

JQ="{{JQ}}"
PAYLOADS="{{PAYLOADS}}"
RUN_STAMP="{{RUN_STAMP}}"

emit_plain() {
	echo "AGENTSPEC_GATE21_CONTEXT_MARKER: The user owns a hamster named Quizzlebottom-2026."
	exit 0
}
trap emit_plain EXIT

payload=$(head -c 1048576)

if [ -n "$payload" ]; then
	# shellcheck disable=SC2016 # jq programs are correctly single-quoted
	if ! line=$(printf '%s' "$payload" | "$JQ" -c --arg stamp "$RUN_STAMP" '. + {run_stamp: $stamp}' 2>/dev/null) ||
		[ -z "$line" ]; then
		# shellcheck disable=SC2016 # as above
		line=$("$JQ" -nc --arg stamp "$RUN_STAMP" --arg raw "$payload" \
			'{run_stamp: $stamp, unparsed: $raw}' 2>/dev/null) || line=""
	fi
	if [ -n "$line" ]; then
		{ printf '%s\n' "$line" >>"$PAYLOADS"; } 2>/dev/null || true
	fi
fi
