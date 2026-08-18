#!/usr/bin/env bash
# Gate #21 capture hook: record the payload, then emit PLAIN text and exit 0.
#
# No JSON envelope — that absence is the whole experiment. The trap guarantees
# the marker is emitted even if the capture fails, so the probe still exercises
# what it is measuring.
set -uo pipefail

JQ="{{JQ}}"
PAYLOADS="{{PAYLOADS}}"

emit_plain() {
	echo "AGENTSPEC_GATE21_CONTEXT_MARKER: The user owns a hamster named Quizzlebottom-2026."
	exit 0
}
trap emit_plain EXIT

payload=$(head -c 1048576)

if [ -n "$payload" ]; then
	# Two invariants in one filter. `-c` compacts to a single line, and
	# `select(type == "object")` keeps every line an *object*: a scalar or array
	# payload yields no output, so the fallback below wraps it instead. Without
	# that guard a stray scalar lands verbatim, and every later `jq -s` filter
	# indexing a field errors on it — which `probe_capture_matches` reads as "no
	# match", polling to the full timeout on a capture that already arrived.
	# shellcheck disable=SC2016 # the jq program is correctly single-quoted
	if ! line=$(printf '%s' "$payload" | "$JQ" -c 'select(type == "object")' 2>/dev/null) ||
		[ -z "$line" ]; then
		# Reached by an unparsable payload and by a non-object one alike; both
		# arrive as `{unparsed: …}` so the object invariant holds either way.
		# `-c` is load-bearing here too: jq -n pretty-prints by default, and a
		# multi-line value would break the slurp for every later reader.
		# shellcheck disable=SC2016 # the jq program is correctly single-quoted; $raw is a jq variable
		line=$("$JQ" -nc --arg raw "$payload" \
			'{unparsed: $raw}' 2>/dev/null) || line=""
	fi
	if [ -n "$line" ]; then
		{ printf '%s\n' "$line" >>"$PAYLOADS"; } 2>/dev/null || true
	fi
fi
