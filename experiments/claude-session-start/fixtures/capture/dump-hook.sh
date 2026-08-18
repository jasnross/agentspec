#!/usr/bin/env bash
# Capture hook: append the payload as one JSON line, then get out of the way.
#
# This runs inside a live provider session, so its contract is absolute: always
# print `{}` and always exit 0. The trap enforces that on every path. A failed
# capture is caught downstream, where `record.sh` refuses an empty capture
# rather than recording it.
set -uo pipefail
trap 'printf "{}\n"; exit 0' EXIT

# jq's absolute path is templated in at Arrange rather than resolved from PATH:
# a provider-spawned hook does not necessarily inherit an interactive PATH, and
# an empty capture costs a live session.
JQ="{{JQ}}"
PAYLOADS="{{PAYLOADS}}"

# Bounds the payload size, not the wait.
payload=$(head -c 1048576)

# An empty read must not append a blank line: it would pass record.sh's
# non-empty check and then break the slurp for every later reader of
# payloads.jsonl.
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
