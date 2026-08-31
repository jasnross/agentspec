#!/usr/bin/env bash
# Capture hook: append this invocation's argv *and* the payload as one JSON
# line, then get out of the way.
#
# Recording argv is a deliberate deviation from the contract's "the payload as
# the provider sent it, with nothing added." Argv is what this probe measures,
# and no other channel carries it — the provider's payload says nothing about
# how the `command` string became a process. The payload itself stays untouched
# under the `payload` key, which is why the manifest's `version_source` reads
# `.payload.cursor_version` rather than the top-level field.
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

# `--args` comes last on purpose: every argument after it is a positional, so
# nothing here can be read as a flag no matter what the provider passed.
#
# The payload filter carries both of the source script's guards across, in one
# expression. `fromjson?` handles an empty or unparsable read; `select(type ==
# "object")` handles a payload that parses as valid *non-object* JSON — a bare
# array or scalar — which `fromjson?` alone would pass through verbatim. That
# second case matters because `version_source` reads `.payload.cursor_version`,
# and indexing an array with a string makes the whole `jq -s` over
# payloads.jsonl throw. `record.sh` swallows that with `2>/dev/null || true`,
# so one such line would silently cost the recorded version for the entire
# capture.
#
# What is genuinely dropped is the source's outer empty-read guard: an empty
# read now appends a line rather than nothing. Diagnose a bad run by reading
# `argc` and `payload`, not the line count.
#
# `2>/dev/null` so a jq diagnostic never reaches a hook stderr whose handling
# by Cursor is itself unmeasured.
#
# shellcheck disable=SC2016 # the jq program is correctly single-quoted; $ARGS and $raw are jq variables
line=$("$JQ" -nc \
	--arg zero "$0" \
	--arg penv "${AGENTSPEC_PROBE_ENV-}" \
	--argjson argc "$#" \
	--arg raw "$payload" \
	'{
		case: ($ARGS.positional[0] // null),
		argv0: $zero,
		argc: $argc,
		argv: $ARGS.positional,
		probe_env: $penv,
		payload: ((($raw | fromjson?) | select(type == "object")) // {unparsed: $raw})
	}' \
	--args "$@" 2>/dev/null) || line=""

# A single printf to an append-mode fd. No flock: it is absent from stock macOS,
# and a provider-spawned hook may not find a mise-pinned one even though Arrange
# did. Eleven hooks firing at once is exactly the concurrent-append case this
# documents — and the atomicity is weaker here than in the source script this
# was copied from, which is worth stating rather than inheriting. PIPE_BUF
# governs pipes, not regular files, and this line now carries argv *plus* the
# payload, bounded only by the 1 MiB `head -c` above. Cursor's sessionStart
# payloads are small, so the interleave risk is low but not established. An
# interleaved append yields a payloads.jsonl that `jq -s` rejects, which
# `probe_capture_matches` reads as "no match" — polling to the full timeout on
# a capture that already arrived.
#
# The braces matter: a failing redirect is diagnosed by the shell before printf
# runs, so `printf ... 2>/dev/null` would not suppress it.
if [ -n "$line" ]; then
	{ printf '%s\n' "$line" >>"$PAYLOADS"; } 2>/dev/null || true
fi
