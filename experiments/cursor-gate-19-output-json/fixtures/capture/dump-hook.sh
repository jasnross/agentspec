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
RUN_STAMP="{{RUN_STAMP}}"

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
