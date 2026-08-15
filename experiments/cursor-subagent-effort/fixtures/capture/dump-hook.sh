#!/usr/bin/env bash
# Capture hook: append the payload as one JSON line, then get out of the way.
#
# This runs inside a live editor, so its contract is absolute: always print `{}`
# and always exit 0. A hook that stalls or fails turns a probe into a broken
# editor. The trap below enforces that on every path, including the ones this
# script does not anticipate — a failed capture is detected downstream, where
# `record.sh` refuses an empty or unstamped capture rather than recording it.
set -uo pipefail
trap 'printf "{}\n"; exit 0' EXIT

# jq's absolute path is templated in at Arrange rather than resolved from PATH.
# A Cursor-spawned hook does not necessarily inherit an interactive PATH — the
# same reasoning that rules out `flock` below, applied to the one tool this hook
# cannot run without. Resolving it here would make a missing PATH entry produce
# an empty capture, which means a refused record after a human has already spent
# a live Cursor session: the most expensive failure mode in this harness.
JQ="{{JQ}}"
PAYLOADS="{{PAYLOADS}}"
RUN_STAMP="{{RUN_STAMP}}"

# Bounds the payload *size*, not the wait: `head -c` returns at N bytes or EOF,
# so a host that holds stdin open holds this hook open too. Cursor closes the
# pipe after writing, which is what makes this safe in practice.
payload=$(head -c 1048576)

# An empty read must not produce a blank line. `jq '. + {…}'` on empty input
# exits 0 with no output, so the blank line would pass the non-empty check in
# record.sh and then fail the stamp-token grep — sending the operator to hunt a
# workspace mix-up that never happened.
if [ -n "$payload" ]; then
	# Stamp the payload with this invocation's token so `record.sh` can tell a
	# capture from this run apart from one an earlier run left in a workspace.
	# shellcheck disable=SC2016 # jq programs are correctly single-quoted; $stamp and $raw are jq variables
	if ! line=$(printf '%s' "$payload" | "$JQ" -c --arg stamp "$RUN_STAMP" '. + {run_stamp: $stamp}' 2>/dev/null) ||
		[ -z "$line" ]; then
		# `-c` is load-bearing: payloads.jsonl is one JSON value per line, and
		# jq -n pretty-prints by default, so an unparsable payload would break
		# the slurp for every later reader.
		# shellcheck disable=SC2016 # as above
		line=$("$JQ" -nc --arg stamp "$RUN_STAMP" --arg raw "$payload" \
			'{run_stamp: $stamp, unparsed: $raw}' 2>/dev/null) || line=""
	fi

	# A single printf to a file opened in append mode is atomic below PIPE_BUF
	# (512 bytes guaranteed by POSIX, 4096 on macOS and Linux), which covers one
	# hook payload written as one line. Serialization beyond that is unnecessary
	# at this payload volume.
	#
	# Deliberately no `flock`: it is absent from stock macOS, and a hook spawned
	# by the editor may not find a mise-pinned one even though Arrange did.
	# The braces matter: a failing redirect is diagnosed by the shell before
	# printf runs, so `printf ... 2>/dev/null` would not suppress it and the
	# hook would emit an error where Cursor expects only `{}`.
	if [ -n "$line" ]; then
		{ printf '%s\n' "$line" >>"$PAYLOADS"; } 2>/dev/null || true
	fi
fi
