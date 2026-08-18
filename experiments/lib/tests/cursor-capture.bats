#!/usr/bin/env bats
# Coverage for the Cursor probe's capture handling, driven with hand-written
# payloads rather than a live Cursor session.
#
# The package is copied into the test's own tree and the copy is driven, so a
# test can never write into the tracked `results/` directory. A record produced
# by `record.sh` is byte-for-byte indistinguishable from a real measurement, and
# an interrupted suite does not run `teardown` — so an in-tree test record would
# survive as evidence of a run that never happened. This harness exists to make
# exactly that impossible.

setup() {
	EXPERIMENTS="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
	TREE="$BATS_TEST_TMPDIR/experiments"
	mkdir -p "$TREE"
	ln -s "$EXPERIMENTS/lib" "$TREE/lib"
	cp -R "$EXPERIMENTS/cursor-subagent-effort" "$TREE/cursor-subagent-effort"
	rm -rf "$TREE/cursor-subagent-effort/results"
	PROBE="$TREE/cursor-subagent-effort/probe.sh"
	RESULTS="$TREE/cursor-subagent-effort/results"
	probe_pid=""
}

teardown() {
	# A test that fails before its `wait` would otherwise leave the runner
	# polling until its timeout.
	[ -n "${probe_pid:-}" ] && kill "$probe_pid" 2>/dev/null
	[ -n "${leaked_ws:-}" ] && rm -rf "$leaked_ws"
	return 0
}

# A workspace shaped the way probe.sh shapes one, carrying a matching payload.
seed_workspace() {
	local ws="$1" model="${2:-claude-opus-5-thinking-low}"
	mkdir -p "$ws/capture"
	jq -nc --arg m "$model" \
		'{hook_event_name: "subagentStart", subagent_type: "arm-effort-low",
		  subagent_model: $m, cursor_version: "3.16.17"}' \
		>"$ws/capture/payloads.jsonl"
}

# Drive the Assert half against a seeded workspace. `probe_record_capture` is
# the only remaining way a capture reaches `record.sh`: the runner calls it at
# the end of its one blocking invocation, and there is no second entry point.
record_capture() {
	bash -c ". '$EXPERIMENTS/lib/probe-common.sh'
		probe_record_capture '$TREE/cursor-subagent-effort' '$1'"
}

newest_record() {
	find "$RESULTS" -name '*.json' 2>/dev/null | sort | tail -n 1
}

# Start the runner in the background, setting the globals `probe_pid` and `ws`.
# Deliberately not called via `$(...)`: command substitution is a subshell, so
# `probe_pid=$!` would be set in a child and lost, leaving `wait` with nothing.
start_runner() {
	local outfile="$1" timeout="$2"
	PROBE_TIMEOUT_SECONDS="$timeout" "$PROBE" >"$outfile" 2>&1 &
	probe_pid=$!

	project=""
	ws=""
	local _
	for _ in $(seq 1 40); do
		project=$(sed -n 's/^Workspace ready: //p' "$outfile" 2>/dev/null | head -n 1)
		[ -n "$project" ] && break
		sleep 0.25
	done
	# The runner prints the directory the operator opens; capture/ is its
	# sibling, deliberately outside it.
	[ -n "$project" ] && ws=$(dirname "$project")
}

@test "a capture resolves tool_version from its own cursor_version" {
	# The surviving job of `record.sh --capture`. Four of six manifests declare
	# `version_source.kind: "capture"`, so this is the only path by which a
	# Cursor record gets a version at all — and no CLI can supply it, since
	# `cursor-agent --version` reports a different artifact on a different
	# versioning scheme.
	ws="$BATS_TEST_TMPDIR/ws"
	seed_workspace "$ws"

	run record_capture "$ws"
	[ "$status" -eq 0 ]

	record=$(newest_record)
	[ -n "$record" ]
	[ "$(jq -r .status "$record")" = "confirmed" ]
	[ "$(jq -r .provider "$record")" = "cursor" ]
	[ "$(jq -r .tool_version "$record")" = "3.16.17" ]
	[ "$(jq -r .assertion.observed "$record")" = "claude-opus-5-thinking-low" ]
}

@test "a capture showing the default model records refuted, not confirmed" {
	# The non-default-value rule in action: the baseline must not pass.
	ws="$BATS_TEST_TMPDIR/ws-default"
	seed_workspace "$ws" claude-opus-5-thinking-high

	run record_capture "$ws"
	[ "$status" -eq 0 ]
	[ "$(jq -r .status "$(newest_record)")" = "refuted" ]
}

@test "a workspace with no payloads is refused and writes no record" {
	# The one failure a stamp was never needed to catch: the hook never fired.
	ws="$BATS_TEST_TMPDIR/ws-empty"
	mkdir -p "$ws/capture"

	run record_capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"no captured payloads"* ]]
	[ -z "$(newest_record)" ]
}

@test "a directory that is not a probe workspace is refused" {
	ws="$BATS_TEST_TMPDIR/not-a-workspace"
	mkdir -p "$ws"

	run record_capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"not a probe workspace"* ]]
}

@test "the runner takes no arguments" {
	# There is no resume, so every argument is a mistake — including the
	# `--capture <ws>` an operator may remember from the removed fallback.
	# Silently starting a fresh run would cost them a live session.
	run "$PROBE" --capture "$BATS_TEST_TMPDIR"
	[ "$status" -ne 0 ]
	[[ "$output" == *"unexpected argument: --capture"* ]]
	[[ "$output" != *"Workspace ready"* ]]

	run "$PROBE" --anything
	[ "$status" -ne 0 ]
	[[ "$output" != *"Workspace ready"* ]]
}

@test "the blocking path records without a second invocation" {
	# The phase's central claim: the operator runs one command and the record
	# appears. A backgrounded writer stands in for the live Cursor session.
	outfile="$BATS_TEST_TMPDIR/probe.out"
	start_runner "$outfile" 30
	leaked_ws="$ws"
	[ -n "$ws" ]
	[ -f "$project/.cursor/hooks.json" ]
	# The apparatus must not be reachable from inside the opened project.
	[ ! -e "$project/capture" ]
	[ -d "$ws/capture" ]

	jq -nc '{hook_event_name: "subagentStart", subagent_type: "arm-effort-low",
		  subagent_model: "claude-opus-5-thinking-low", cursor_version: "3.16.17"}' \
		>>"$ws/capture/payloads.jsonl"

	rc=0
	wait "$probe_pid" || rc=$?
	probe_pid=""
	[ "$rc" -eq 0 ]
	[ "$(jq -r .status "$(newest_record)")" = "confirmed" ]
}

@test "the generated workspace has no unsubstituted placeholder anywhere" {
	outfile="$BATS_TEST_TMPDIR/probe-tmpl.out"
	PROBE_TIMEOUT_SECONDS=1 run "$PROBE"
	project=$(printf '%s\n' "$output" | sed -n 's/^Workspace ready: //p' | head -n 1)
	[ -n "$project" ]
	ws=$(dirname "$project")
	leaked_ws="$ws"

	[ -f "$project/.cursor/hooks.json" ]
	run grep -c '{{' "$project/.cursor/hooks.json"
	[ "$output" = "0" ]
	run grep -c '{{' "$ws/capture/dump-hook.sh"
	[ "$output" = "0" ]

	# The generated hooks.json is valid JSON pointing at an executable hook
	# that writes into this workspace.
	run jq -e . "$project/.cursor/hooks.json"
	[ "$status" -eq 0 ]
	[ -x "$ws/capture/dump-hook.sh" ]
	grep -q "$ws/capture/payloads.jsonl" "$ws/capture/dump-hook.sh"
	[ -f "$project/.cursor/agents/arm-effort-low.md" ]
}

@test "the capture apparatus is not reachable from inside the opened project" {
	# A probe whose oracle is the agent's answer is defeated if the agent can
	# read the apparatus: a marker string findable by filesystem search makes
	# "the hook injected it" and "the agent grepped for it" indistinguishable.
	PROBE_TIMEOUT_SECONDS=1 run "$PROBE"
	project=$(printf '%s\n' "$output" | sed -n 's/^Workspace ready: //p' | head -n 1)
	[ -n "$project" ]
	ws=$(dirname "$project")
	leaked_ws="$ws"

	[ -d "$ws/capture" ]
	[ ! -e "$project/capture" ]
	# Nothing under the opened project may contain the payloads file or the
	# hook script body.
	run bash -c "find '$project' -type f -exec grep -l 'payloads.jsonl' {} + 2>/dev/null | grep -v hooks.json"
	[ -z "$output" ]
}

@test "a timeout leaves the workspace intact and says the run is over" {
	# Losing a workspace after a live session is the one unrecoverable failure,
	# so it gets its own test. With no resume path the message must not imply
	# one: the workspace is kept for inspection, not for a second invocation.
	PROBE_TIMEOUT_SECONDS=1 run "$PROBE"
	[ "$status" -ne 0 ]

	project=$(printf '%s\n' "$output" | sed -n 's/^Workspace ready: //p' | head -n 1)
	[ -n "$project" ]
	ws=$(dirname "$project")
	leaked_ws="$ws"
	[ -d "$ws" ]
	[ -d "$project" ]
	# The message names the workspace root, since that is where capture/ lives
	# — not the project directory the operator opened.
	[[ "$output" == *"kept for inspection: $ws"* ]]
	[[ "$output" == *"re-run the probe"* ]]
	[[ "$output" != *"--capture"* ]]
}

@test "the capture hook appends one JSON line and echoes {}" {
	ws="$BATS_TEST_TMPDIR/hookws"
	mkdir -p "$ws/capture"
	hook="$ws/capture/dump-hook.sh"
	# Templated the same way probe.sh does, so this exercises the real path.
	bash -c ". '$EXPERIMENTS/lib/probe-common.sh'
		probe_template_file '$TREE/cursor-subagent-effort/fixtures/capture/dump-hook.sh' '$hook' \
			'JQ=$(command -v jq)' 'PAYLOADS=$ws/capture/payloads.jsonl'"
	chmod +x "$hook"

	run bash -c "printf '{\"hook_event_name\":\"subagentStart\"}' | '$hook'"
	[ "$status" -eq 0 ]
	[ "$(printf '%s' "$output" | jq -c .)" = "{}" ]
	[ "$(jq -r .hook_event_name "$ws/capture/payloads.jsonl")" = "subagentStart" ]
	# One line, one JSON value — what every later reader's `jq -s` depends on.
	[ "$(wc -l <"$ws/capture/payloads.jsonl" | tr -d ' ')" -eq 1 ]
}

@test "the capture hook still echoes {} and exits 0 when jq is missing" {
	# Its contract inside a live editor is absolute. A failure here must be
	# invisible to Cursor and caught downstream by record.sh refusing an empty
	# capture — never a nonzero exit that breaks the session.
	ws="$BATS_TEST_TMPDIR/hookws-nojq"
	mkdir -p "$ws/capture"
	hook="$ws/capture/dump-hook.sh"
	bash -c ". '$EXPERIMENTS/lib/probe-common.sh'
		probe_template_file '$TREE/cursor-subagent-effort/fixtures/capture/dump-hook.sh' '$hook' \
			'JQ=/nonexistent/jq' 'PAYLOADS=$ws/capture/payloads.jsonl'"
	chmod +x "$hook"

	run bash -c "printf '{\"a\":1}' | '$hook'"
	[ "$status" -eq 0 ]
	[ "$(printf '%s' "$output" | jq -c .)" = "{}" ]
}

@test "the capture hook echoes {} and writes nothing on an empty payload" {
	# An empty read must not append a blank line: that would pass record.sh's
	# non-empty check and then fail the token grep, reporting a stale capture
	# for a mix-up that never happened.
	ws="$BATS_TEST_TMPDIR/hookws-empty"
	mkdir -p "$ws/capture"
	hook="$ws/capture/dump-hook.sh"
	bash -c ". '$EXPERIMENTS/lib/probe-common.sh'
		probe_template_file '$TREE/cursor-subagent-effort/fixtures/capture/dump-hook.sh' '$hook' \
			'JQ=$(command -v jq)' 'PAYLOADS=$ws/capture/payloads.jsonl'"
	chmod +x "$hook"

	run bash -c "printf '' | '$hook'"
	[ "$status" -eq 0 ]
	[ "$(printf '%s' "$output" | jq -c .)" = "{}" ]
	[ ! -s "$ws/capture/payloads.jsonl" ]
}

@test "an unparsable payload is still appended as one line" {
	# payloads.jsonl is one JSON value per line. A pretty-printed fallback
	# would break the slurp for every later reader, including record.sh.
	ws="$BATS_TEST_TMPDIR/hookws-unparsable"
	mkdir -p "$ws/capture"
	hook="$ws/capture/dump-hook.sh"
	bash -c ". '$EXPERIMENTS/lib/probe-common.sh'
		probe_template_file '$TREE/cursor-subagent-effort/fixtures/capture/dump-hook.sh' '$hook' \
			'JQ=$(command -v jq)' 'PAYLOADS=$ws/capture/payloads.jsonl'"
	chmod +x "$hook"

	run bash -c "printf 'not json at all' | '$hook'"
	[ "$status" -eq 0 ]

	[ "$(wc -l <"$ws/capture/payloads.jsonl" | tr -d ' ')" -eq 1 ]
	run jq -s -e 'length == 1 and (.[0].unparsed | type == "string")' "$ws/capture/payloads.jsonl"
	[ "$status" -eq 0 ]
}

@test "a scalar payload is wrapped rather than appended verbatim" {
	# Every line of payloads.jsonl must be an object. A bare scalar would be
	# valid JSON and land verbatim, and then every later `jq -s` filter indexing
	# a field errors on it — which `probe_capture_matches` reads as "no match",
	# so the runner polls to the full timeout on a capture that already arrived.
	ws="$BATS_TEST_TMPDIR/hookws-scalar"
	mkdir -p "$ws/capture"
	hook="$ws/capture/dump-hook.sh"
	bash -c ". '$EXPERIMENTS/lib/probe-common.sh'
		probe_template_file '$TREE/cursor-subagent-effort/fixtures/capture/dump-hook.sh' '$hook' \
			'JQ=$(command -v jq)' 'PAYLOADS=$ws/capture/payloads.jsonl'"
	chmod +x "$hook"

	run bash -c "printf '\"just a string\"' | '$hook'"
	[ "$status" -eq 0 ]

	[ "$(wc -l <"$ws/capture/payloads.jsonl" | tr -d ' ')" -eq 1 ]
	run jq -s -e 'length == 1 and (.[0] | type == "object")' "$ws/capture/payloads.jsonl"
	[ "$status" -eq 0 ]
	# A filter indexing a field must not error on it.
	run jq -s -e 'any(.[]; .hook_event_name == "nope") | not' "$ws/capture/payloads.jsonl"
	[ "$status" -eq 0 ]
}

@test "the capture hook echoes {} when its payloads path is unwritable" {
	ws="$BATS_TEST_TMPDIR/hookws-unwritable"
	mkdir -p "$ws/capture"
	hook="$ws/capture/dump-hook.sh"
	bash -c ". '$EXPERIMENTS/lib/probe-common.sh'
		probe_template_file '$TREE/cursor-subagent-effort/fixtures/capture/dump-hook.sh' '$hook' \
			'JQ=$(command -v jq)' 'PAYLOADS=$ws/no/such/dir/payloads.jsonl'"
	chmod +x "$hook"

	run bash -c "printf '{\"a\":1}' | '$hook'"
	[ "$status" -eq 0 ]
	[ "$(printf '%s' "$output" | jq -c .)" = "{}" ]
}
