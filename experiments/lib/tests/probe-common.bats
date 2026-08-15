#!/usr/bin/env bats
# Coverage for the shared *Arrange* helpers.

setup() {
	EXPERIMENTS="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
	COMMON="$EXPERIMENTS/lib/probe-common.sh"
}

# `probe_prompt_selection` and `probe_require_tools` exit rather than return, so
# they are driven through a throwaway script rather than sourced into the test.
write_driver() {
	local script="$BATS_TEST_TMPDIR/driver.sh"
	{
		printf '#!/usr/bin/env bash\nset -euo pipefail\n'
		printf '. "%s"\n' "$COMMON"
		cat
	} >"$script"
	chmod +x "$script"
	printf '%s' "$script"
}

@test "probe_workspace_create returns a workspace carrying a run stamp" {
	driver=$(write_driver <<-'SH'
		probe_workspace_create unit-test
	SH
	)

	run "$driver"
	[ "$status" -eq 0 ]
	ws="$output"
	[ -d "$ws/capture" ]
	[ -s "$ws/capture/.run_stamp" ]
	rm -rf "$ws"
}

@test "two workspaces carry different run-stamp tokens" {
	driver=$(write_driver <<-'SH'
		ws=$(probe_workspace_create unit-test)
		probe_run_stamp_token "$ws"
		rm -rf "$ws"
	SH
	)

	first=$("$driver")
	second=$("$driver")
	[ -n "$first" ]
	[ "$first" != "$second" ]
}

@test "probe_run_stamp_epoch reads back the epoch that was written" {
	driver=$(write_driver <<-'SH'
		ws=$(probe_workspace_create unit-test)
		probe_run_stamp_epoch "$ws"
		rm -rf "$ws"
	SH
	)

	run "$driver"
	[ "$status" -eq 0 ]
	[[ "$output" =~ ^[0-9]+$ ]]
}

@test "probe_require_tools names every missing tool" {
	driver=$(write_driver <<-'SH'
		probe_require_tools jq definitely-not-a-real-tool also-not-real
	SH
	)

	run "$driver"
	[ "$status" -ne 0 ]
	# jq is present, so the rendered list must name the other two and only them.
	[[ "$output" == *"missing required tool(s): definitely-not-a-real-tool also-not-real"* ]]
}

@test "probe_require_tools succeeds when every tool is present" {
	driver=$(write_driver <<-'SH'
		probe_require_tools jq
		echo ok
	SH
	)

	run "$driver"
	[ "$status" -eq 0 ]
	[ "$output" = "ok" ]
}

@test "probe_wait_for_capture returns 0 once a matching payload is appended" {
	driver=$(write_driver <<-'SH'
		ws="$1"
		if probe_wait_for_capture "$ws" 'any(.[]; .hook_event_name == "target")' 20; then
			echo matched
		else
			echo timedout
		fi
	SH
	)

	ws="$BATS_TEST_TMPDIR/ws"
	mkdir -p "$ws/capture"
	(
		sleep 1
		printf '{"hook_event_name":"target"}\n' >>"$ws/capture/payloads.jsonl"
	) &

	run "$driver" "$ws"
	wait
	[ "$status" -eq 0 ]
	[ "$output" = "matched" ]
}

@test "probe_wait_for_capture returns 1 on timeout and records nothing" {
	driver=$(write_driver <<-'SH'
		ws="$1"
		if probe_wait_for_capture "$ws" 'any(.[]; .hook_event_name == "never")' 1; then
			echo matched
		else
			echo timedout
		fi
	SH
	)

	ws="$BATS_TEST_TMPDIR/ws-timeout"
	mkdir -p "$ws/capture"
	printf '{"hook_event_name":"other"}\n' >"$ws/capture/payloads.jsonl"

	run "$driver" "$ws"
	[ "$status" -eq 0 ]
	[ "$output" = "timedout" ]
	[ ! -d "$ws/results" ]
}

@test "probe_wait_for_capture slurps, so an any(.[]; …) filter matches JSONL" {
	# Without `jq -s` this filter never matches and the runner polls to timeout
	# on a capture that already holds the answer.
	driver=$(write_driver <<-'SH'
		ws="$1"
		probe_wait_for_capture "$ws" 'any(.[]; .cursor_version != null)' 5 && echo matched
	SH
	)

	ws="$BATS_TEST_TMPDIR/ws-slurp"
	mkdir -p "$ws/capture"
	printf '{"a":1}\n{"cursor_version":"3.16.17"}\n' >"$ws/capture/payloads.jsonl"

	run "$driver" "$ws"
	[ "$status" -eq 0 ]
	[ "$output" = "matched" ]
}

@test "PROBE_TIMEOUT_SECONDS overrides the caller's timeout" {
	driver=$(write_driver <<-'SH'
		ws="$1"
		probe_wait_for_capture "$ws" 'any(.[]; .never == true)' 3600 || echo timedout
	SH
	)

	ws="$BATS_TEST_TMPDIR/ws-override"
	mkdir -p "$ws/capture"
	printf '{"a":1}\n' >"$ws/capture/payloads.jsonl"

	PROBE_TIMEOUT_SECONDS=1 run "$driver" "$ws"
	[ "$status" -eq 0 ]
	[ "$output" = "timedout" ]
}

@test "probe_prompt_selection exits 2 without blocking when stdin is not a tty" {
	# The guard that keeps a human-judged probe from hanging `probe-run` forever.
	driver=$(write_driver <<-'SH'
		probe_prompt_selection '[{"id":"a","text":"first"},{"id":"couldnt-tell","text":"Could not determine","status":"inconclusive"}]' 'Which one?'
	SH
	)

	run "$driver" </dev/null
	[ "$status" -eq 2 ]
	[[ "$output" == *"stdin is not a terminal"* ]]
	[[ "$output" == *"Which one?"* ]]
	[[ "$output" == *"couldnt-tell"* ]]
}

@test "the selection loop re-prompts on an unrecognized answer, then returns the id" {
	# Drives `probe_read_selection` rather than `probe_prompt_selection`: the
	# latter's tty guard fires before the loop on any non-tty stdin, including
	# a here-doc.
	driver=$(write_driver <<-'SH'
		probe_read_selection '[{"id":"neither","text":"Neither marker"},{"id":"couldnt-tell","text":"Could not determine","status":"inconclusive"}]' 'Which markers?'
	SH
	)

	run "$driver" <<-'ANSWERS'
		nonsense
		neither
	ANSWERS

	[ "$status" -eq 0 ]
	[[ "$output" == *'"nonsense" is not one of the option ids'* ]]
	[[ "$output" == *"neither"* ]]
}
