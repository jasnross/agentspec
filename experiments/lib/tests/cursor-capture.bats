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
	local token=deadbeefcafe1234
	printf '%s %s\n' "$token" "$(date +%s)" >"$ws/capture/.run_stamp"
	jq -nc --arg t "$token" --arg m "$model" \
		'{hook_event_name: "subagentStart", subagent_type: "arm-effort-low",
		  subagent_model: $m, cursor_version: "3.16.17", run_stamp: $t}' \
		>"$ws/capture/payloads.jsonl"
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

@test "--capture records confirmed with the version taken from the capture" {
	ws="$BATS_TEST_TMPDIR/ws"
	seed_workspace "$ws"

	run "$PROBE" --capture "$ws"
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

	run "$PROBE" --capture "$ws"
	[ "$status" -eq 0 ]
	[ "$(jq -r .status "$(newest_record)")" = "refuted" ]
}

@test "the same capture with the run stamp removed is refused" {
	ws="$BATS_TEST_TMPDIR/ws-nostamp"
	seed_workspace "$ws"
	rm "$ws/capture/.run_stamp"

	run "$PROBE" --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"run stamp"* ]]
	[ -z "$(newest_record)" ]
}

@test "a capture whose payloads carry a different run stamp is refused" {
	ws="$BATS_TEST_TMPDIR/ws-othertoken"
	seed_workspace "$ws"
	printf 'adifferenttoken %s\n' "$(date +%s)" >"$ws/capture/.run_stamp"

	run "$PROBE" --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"stale capture"* ]]
	[ -z "$(newest_record)" ]
}

@test "--capture on a workspace with no payloads exits nonzero and writes no record" {
	ws="$BATS_TEST_TMPDIR/ws-empty"
	mkdir -p "$ws/capture"
	printf 'sometoken %s\n' "$(date +%s)" >"$ws/capture/.run_stamp"

	run "$PROBE" --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"no captured payloads"* ]]
	[ -z "$(newest_record)" ]
}

@test "--capture with an empty or missing path never starts a fresh run" {
	# Falling through to Arrange would silently convert a resume into a new
	# 15-minute live-session run.
	run "$PROBE" --capture ""
	[ "$status" -ne 0 ]
	[[ "$output" == *"requires a workspace path"* ]]
	[[ "$output" != *"Workspace ready"* ]]

	run "$PROBE" --capture "$BATS_TEST_TMPDIR/nope"
	[ "$status" -ne 0 ]
	[[ "$output" == *"no such workspace"* ]]
	[[ "$output" != *"Workspace ready"* ]]
}

@test "--capture on a directory that is not a probe workspace says so" {
	ws="$BATS_TEST_TMPDIR/not-a-workspace"
	mkdir -p "$ws"

	run "$PROBE" --capture "$ws"
	[ "$status" -ne 0 ]
	[[ "$output" == *"not a probe workspace"* ]]
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

	token=$(cut -d' ' -f1 <"$ws/capture/.run_stamp")
	jq -nc --arg t "$token" \
		'{hook_event_name: "subagentStart", subagent_type: "arm-effort-low",
		  subagent_model: "claude-opus-5-thinking-low", cursor_version: "3.16.17",
		  run_stamp: $t}' >>"$ws/capture/payloads.jsonl"

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

@test "a timeout leaves the workspace intact and prints a resume command naming it" {
	# Losing a workspace after a live session is the one unrecoverable failure,
	# so it gets its own test.
	PROBE_TIMEOUT_SECONDS=1 run "$PROBE"
	[ "$status" -ne 0 ]

	project=$(printf '%s\n' "$output" | sed -n 's/^Workspace ready: //p' | head -n 1)
	[ -n "$project" ]
	ws=$(dirname "$project")
	leaked_ws="$ws"
	[ -d "$ws" ]
	[ -d "$project" ]
	# The resume command names the workspace root, since that is where
	# capture/ lives — not the project directory the operator opened.
	[[ "$output" == *"--capture $ws"* ]]
}

@test "the capture hook appends a stamped JSON line and echoes {}" {
	ws="$BATS_TEST_TMPDIR/hookws"
	mkdir -p "$ws/capture"
	hook="$ws/capture/dump-hook.sh"
	# Templated the same way probe.sh does, so this exercises the real path.
	bash -c ". '$EXPERIMENTS/lib/probe-common.sh'
		probe_template_file '$TREE/cursor-subagent-effort/fixtures/capture/dump-hook.sh' '$hook' \
			'JQ=$(command -v jq)' 'PAYLOADS=$ws/capture/payloads.jsonl' 'RUN_STAMP=tok123'"
	chmod +x "$hook"

	run bash -c "printf '{\"hook_event_name\":\"subagentStart\"}' | '$hook'"
	[ "$status" -eq 0 ]
	[ "$(printf '%s' "$output" | jq -c .)" = "{}" ]
	[ "$(jq -r .run_stamp "$ws/capture/payloads.jsonl")" = "tok123" ]
	[ "$(jq -r .hook_event_name "$ws/capture/payloads.jsonl")" = "subagentStart" ]
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
			'JQ=/nonexistent/jq' 'PAYLOADS=$ws/capture/payloads.jsonl' 'RUN_STAMP=tok123'"
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
			'JQ=$(command -v jq)' 'PAYLOADS=$ws/capture/payloads.jsonl' 'RUN_STAMP=tok123'"
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
			'JQ=$(command -v jq)' 'PAYLOADS=$ws/capture/payloads.jsonl' 'RUN_STAMP=tok123'"
	chmod +x "$hook"

	run bash -c "printf 'not json at all' | '$hook'"
	[ "$status" -eq 0 ]

	[ "$(wc -l <"$ws/capture/payloads.jsonl" | tr -d ' ')" -eq 1 ]
	run jq -s -e 'length == 1 and .[0].run_stamp == "tok123"' "$ws/capture/payloads.jsonl"
	[ "$status" -eq 0 ]
}

@test "the capture hook echoes {} when its payloads path is unwritable" {
	ws="$BATS_TEST_TMPDIR/hookws-unwritable"
	mkdir -p "$ws/capture"
	hook="$ws/capture/dump-hook.sh"
	bash -c ". '$EXPERIMENTS/lib/probe-common.sh'
		probe_template_file '$TREE/cursor-subagent-effort/fixtures/capture/dump-hook.sh' '$hook' \
			'JQ=$(command -v jq)' 'PAYLOADS=$ws/no/such/dir/payloads.jsonl' 'RUN_STAMP=tok123'"
	chmod +x "$hook"

	run bash -c "printf '{\"a\":1}' | '$hook'"
	[ "$status" -eq 0 ]
	[ "$(printf '%s' "$output" | jq -c .)" = "{}" ]
}
