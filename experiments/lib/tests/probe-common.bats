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

@test "probe_template_file substitutes every placeholder and keeps JSON valid" {
	src="$BATS_TEST_TMPDIR/hooks.json.tmpl"
	printf '{"version":1,"hooks":{"subagentStart":[{"command":"{{CAPTURE_SCRIPT}}"}]}}\n' >"$src"

	driver=$(write_driver <<-'SH'
		probe_template_file "$1" "$2" "CAPTURE_SCRIPT=/tmp/probe ws/dump-hook.sh"
	SH
	)

	run "$driver" "$src" "$BATS_TEST_TMPDIR/out/hooks.json"
	[ "$status" -eq 0 ]
	run jq -e . "$BATS_TEST_TMPDIR/out/hooks.json"
	[ "$status" -eq 0 ]
	[ "$(jq -r '.hooks.subagentStart[0].command' "$BATS_TEST_TMPDIR/out/hooks.json")" = "/tmp/probe ws/dump-hook.sh" ]
}

@test "probe_template_file substitutes several placeholders in one pass" {
	src="$BATS_TEST_TMPDIR/multi.tmpl"
	printf 'a={{ONE}} b={{TWO}} a-again={{ONE}}\n' >"$src"

	driver=$(write_driver <<-'SH'
		probe_template_file "$1" "$2" "ONE=first" "TWO=second"
	SH
	)

	run "$driver" "$src" "$BATS_TEST_TMPDIR/multi.out"
	[ "$status" -eq 0 ]
	[ "$(cat "$BATS_TEST_TMPDIR/multi.out")" = "a=first b=second a-again=first" ]
}

@test "probe_template_file fails rather than emitting an unsubstituted placeholder" {
	# An unsubstituted path is exactly the silent-never-fires failure this
	# helper exists to prevent.
	src="$BATS_TEST_TMPDIR/partial.tmpl"
	printf 'script={{CAPTURE_SCRIPT}} payloads={{PAYLOADS}}\n' >"$src"

	driver=$(write_driver <<-'SH'
		probe_template_file "$1" "$2" "CAPTURE_SCRIPT=/tmp/x"
	SH
	)

	run "$driver" "$src" "$BATS_TEST_TMPDIR/partial.out"
	[ "$status" -ne 0 ]
	[[ "$output" == *"unsubstituted placeholder"* ]]
	[[ "$output" == *"{{PAYLOADS}}"* ]]
	[ ! -f "$BATS_TEST_TMPDIR/partial.out" ]
}

@test "probe_template_file substitutes a value containing an ampersand literally" {
	# bash >= 5.2 expands `&` in a `${x//a/b}` replacement to the matched text
	# while 3.2 does not, and macOS ships both — so this would substitute
	# correctly on one machine and reinsert the placeholder on another.
	src="$BATS_TEST_TMPDIR/amp.tmpl"
	printf 'path={{P}}\n' >"$src"

	driver=$(write_driver <<-'SH'
		probe_template_file "$1" "$2" 'P=/tmp/a&b/dump.sh'
	SH
	)

	run "$driver" "$src" "$BATS_TEST_TMPDIR/amp.out"
	[ "$status" -eq 0 ]
	[ "$(cat "$BATS_TEST_TMPDIR/amp.out")" = "path=/tmp/a&b/dump.sh" ]
}

@test "probe_template_file substitutes a value containing shell and glob metacharacters" {
	src="$BATS_TEST_TMPDIR/meta.tmpl"
	printf 'path={{P}}\n' >"$src"

	driver=$(write_driver <<-'SH'
		probe_template_file "$1" "$2" 'P=/tmp/a$b*c?d[e]/dump.sh'
	SH
	)

	run "$driver" "$src" "$BATS_TEST_TMPDIR/meta.out"
	[ "$status" -eq 0 ]
	[ "$(cat "$BATS_TEST_TMPDIR/meta.out")" = 'path=/tmp/a$b*c?d[e]/dump.sh' ]
}

@test "probe_template_file rejects a key containing metacharacters" {
	# A key is interpolated into a glob pattern, so `P*` would match every
	# placeholder beginning with P.
	src="$BATS_TEST_TMPDIR/badkey.tmpl"
	printf 'a={{PX}} b={{P}}\n' >"$src"

	driver=$(write_driver <<-'SH'
		probe_template_file "$1" "$2" 'P*=Z'
	SH
	)

	run "$driver" "$src" "$BATS_TEST_TMPDIR/badkey.out"
	[ "$status" -ne 0 ]
	[[ "$output" == *"invalid template key"* ]]
}

@test "probe_template_file rejects an empty key" {
	src="$BATS_TEST_TMPDIR/emptykey.tmpl"
	printf 'a={{K}}\n' >"$src"

	driver=$(write_driver <<-'SH'
		probe_template_file "$1" "$2" '=value'
	SH
	)

	run "$driver" "$src" "$BATS_TEST_TMPDIR/emptykey.out"
	[ "$status" -ne 0 ]
	[[ "$output" == *"invalid template key"* ]]
}

@test "probe_template_file fails on a missing source" {
	driver=$(write_driver <<-'SH'
		probe_template_file "$1" "$2" "KEY=value"
	SH
	)

	run "$driver" "$BATS_TEST_TMPDIR/does-not-exist" "$BATS_TEST_TMPDIR/out"
	[ "$status" -ne 0 ]
	[[ "$output" == *"template source not found"* ]]
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
