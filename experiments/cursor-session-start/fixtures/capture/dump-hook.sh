#!/usr/bin/env bash
# Capture hook: append the payload as one JSON line, then get out of the way.
#
# This runs inside a live provider session, so its contract is absolute: always
# print `{}` and always exit 0. The trap enforces that on every path. A failed
# capture is caught downstream, where `record.sh` refuses an empty or unstamped
# capture rather than recording it.
set -uo pipefail
trap 'printf "{}\n"; exit 0' EXIT

# jq's absolute path is templated in at Arrange rather than resolved from PATH:
# a provider-spawned hook does not necessarily inherit an interactive PATH, and
# an empty capture costs a live session.
JQ="{{JQ}}"
PAYLOADS="{{PAYLOADS}}"
RUN_STAMP="{{RUN_STAMP}}"

# Bounds the payload size, not the wait.
payload=$(head -c 1048576)

# An empty read must not append a blank line: it would pass record.sh's
# non-empty check and then fail the token grep, reporting a stale capture for a
# mix-up that never happened.
if [ -n "$payload" ]; then
	# shellcheck disable=SC2016 # jq programs are correctly single-quoted
	if ! line=$(printf '%s' "$payload" | "$JQ" -c --arg stamp "$RUN_STAMP" '. + {run_stamp: $stamp}' 2>/dev/null) ||
		[ -z "$line" ]; then
		# `-c` is load-bearing: payloads.jsonl is one JSON value per line.
		# shellcheck disable=SC2016 # as above
		line=$("$JQ" -nc --arg stamp "$RUN_STAMP" --arg raw "$payload" \
			'{run_stamp: $stamp, unparsed: $raw}' 2>/dev/null) || line=""
	fi

	# A single printf to an append-mode fd is atomic below PIPE_BUF. No flock:
	# it is absent from stock macOS, and a provider-spawned hook may not find a
	# mise-pinned one even though Arrange did.
	#
	# The braces matter: a failing redirect is diagnosed by the shell before
	# printf runs, so `printf ... 2>/dev/null` would not suppress it.
	if [ -n "$line" ]; then
		{ printf '%s\n' "$line" >>"$PAYLOADS"; } 2>/dev/null || true
	fi
fi
