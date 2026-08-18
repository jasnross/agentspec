#!/usr/bin/env bash
# Gate #19 capture hook: record the payload, then emit deny JSON AND exit 2.
#
# Unlike every other capture hook here, this one deliberately does not print
# `{}` and exit 0 — emitting both a JSON body and exit 2 is precisely what the
# probe measures. The trap therefore guarantees the *deny* shape rather than the
# no-op shape, so a capture failure still produces the output under test.
set -uo pipefail

JQ="{{JQ}}"
PAYLOADS="{{PAYLOADS}}"

emit_deny() {
	cat <<'JSON'
{
  "permission": "deny",
  "user_message": "AGENTSPEC_GATE19_USER_MARKER_0123456789",
  "agent_message": "AGENTSPEC_GATE19_AGENT_MARKER_9876543210"
}
JSON
	exit 2
}
trap emit_deny EXIT

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
