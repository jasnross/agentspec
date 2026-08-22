#!/usr/bin/env bats
# Coverage for the three gates and view assembly in `probe-claude-otel.sh`.
#
# Every test drives fabricated views and fabricated sink directories, so the
# suite runs with no `claude` on PATH and costs nothing. That is the point of
# extracting the gates into `lib/`: duplicated in two runners, the only thing
# that would ever exercise them is a real, paid run — an untested control on a
# billed apparatus.

setup() {
	EXPERIMENTS="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
	OTEL="$EXPERIMENTS/lib/probe-claude-otel.sh"
	MARKER=AGENTSPEC-PROBE-MARKER-TEST
	VIEW="$BATS_TEST_TMPDIR/view.json"
	# The agent package's own marker, read out of its fixture rather than
	# restated, so the two cannot drift.
	MARKER_AGENT=$(sed -n 's/^\(AGENTSPEC-PROBE-MARKER-[A-Z0-9]*\)$/\1/p' \
		"$EXPERIMENTS/claude-agent-effort/fixtures/assertion/.claude/agents/probe-effort.md" 2>/dev/null | head -1)
	MARKER_SKILL=$(sed -n 's/^\(AGENTSPEC-PROBE-MARKER-[A-Z0-9]*\)$/\1/p' \
		"$EXPERIMENTS/claude-skill-effort/fixtures/assertion/.claude/skills/probe-effort/SKILL.md" 2>/dev/null | head -1)
}

# The library returns rather than exits, so it is sourced into a subshell whose
# exit status is the helper's return value. `set -e` is deliberately not set
# here: the helpers are contracted to return, and a bare `return 1` under
# `errexit` would abort before the diagnostic could be captured.
run_helper() {
	bash -c ". '$OTEL'
		$*"
}

# An arm-keyed view. Each argument is `<arm>=<json-array-of-request-bodies>`.
write_view() {
	local expr='{}' arg arm bodies
	local -a args=()
	local i=0
	for arg in "$@"; do
		arm="${arg%%=*}"
		bodies="${arg#*=}"
		args+=(--arg "a$i" "$arm" --argjson "v$i" "$bodies")
		expr="$expr | .[\$a$i] = \$v$i"
		i=$((i + 1))
	done
	jq -n "${args[@]}" "$expr" >"$VIEW"
}

# A request body governed by the fixture: the fixture's text is its system
# prompt, which is what "governed" means.
marked() {
	printf '{"system":"%s","output_config":{"effort":"%s"}}' "$MARKER" "$1"
}

# A request that merely quotes the marker back in a tool result — the shape a
# main thread takes after a subagent replies. Not governed by the fixture: its
# effort is the ungoverned level, and it must not be read as the arm's value.
echoed() {
	printf '{"system":"main thread","messages":[{"role":"user","content":"%s"}],"output_config":{"effort":"%s"}}' "$MARKER" "$1"
}

# A request body governed by a *skill* fixture: the skill's body lands in
# `messages[]` rather than in the system prompt, which is why the gates take the
# field to match on rather than assuming `.system`.
governed_in_messages() {
	printf '{"system":"plain","messages":[{"role":"user","content":"%s"}],"output_config":{"effort":"%s"}}' "$MARKER" "$1"
}

# A request body nothing governed: no marker, carries an effort.
ungoverned() {
	printf '{"system":"plain","output_config":{"effort":"%s"}}' "$1"
}

@test "gate_marker passes when every arm holds a marked request" {
	write_view "a=[$(marked low),$(ungoverned medium)]" "b=[$(marked low)]"

	run run_helper "probe_claude_gate_marker '$VIEW' '$MARKER'"
	[ "$status" -eq 0 ]
}

@test "gate_marker fails when one arm holds none" {
	# The failure that makes the inert arm assertable: without this gate, "the
	# fixture never engaged" reads identically to "Claude discarded its effort."
	write_view "a=[$(marked low)]" "b=[$(ungoverned medium)]"

	run run_helper "probe_claude_gate_marker '$VIEW' '$MARKER'"
	[ "$status" -ne 0 ]
	[[ "$output" == *"never engaged the fixture"* ]]
}

@test "gate_marker fails when an arm only echoes the marker in a tool result" {
	# The failure the `.system` narrowing exists for, measured on a real run: a
	# subagent's reply carries the fixture's text back to the main thread, on a
	# request the fixture governs not at all. Matched on `tostring`, that echo
	# would satisfy this gate for an arm whose fixture never engaged.
	write_view "a=[$(marked low)]" "b=[$(echoed medium)]"

	run run_helper "probe_claude_gate_marker '$VIEW' '$MARKER'"
	[ "$status" -ne 0 ]
	[[ "$output" == *".system carries the fixture marker"* ]]
}

@test "gate_marker scopes to the field it is given" {
	# A skill's body never reaches `.system`; measured at 2.1.232 it arrives in
	# `messages[]`. The default would read that arm as never having engaged, so
	# `claude-skill-effort` names the field instead of the library guessing.
	write_view "a=[$(governed_in_messages low)]" "b=[$(governed_in_messages low)]"

	run run_helper "probe_claude_gate_marker '$VIEW' '$MARKER'"
	[ "$status" -ne 0 ]

	run run_helper "probe_claude_gate_marker '$VIEW' '$MARKER' .messages"
	[ "$status" -eq 0 ]
}

@test "gate_control's control set is the complement of the field it is given" {
	# The two gates partition the requests between them, so they must be passed
	# the same field — a control set computed under one definition and an arm
	# value computed under the other would not describe the same run.
	#
	# Under `.messages` the two governed requests are excluded and the control is
	# the single ungoverned `medium`. Under `.system` nothing is excluded, so the
	# governed `low`s join the control set and it no longer agrees with itself.
	write_view "a=[$(governed_in_messages low),$(ungoverned medium)]" \
		"b=[$(governed_in_messages low)]"

	run run_helper "probe_claude_gate_control '$VIEW' '$MARKER' .messages"
	[ "$status" -eq 0 ]

	run run_helper "probe_claude_gate_control '$VIEW' '$MARKER'"
	[ "$status" -ne 0 ]
	[[ "$output" == *"internally inconsistent"* ]]
}

@test "gate_control does not admit a governed request when the field is multi-output" {
	# The field is the caller's to name, and nothing constrains it to one output.
	# A bare `$field` would re-emit its request once per output, so `select(… |
	# not)` admits a *governed* request whose other outputs lack the marker —
	# putting the arm's own level into the control the projection compares it
	# against. Gate 2 passes throughout, so the contamination is silent.
	#
	# Here the two requests disagree, so a control set that admitted the governed
	# `low` would be inconsistent and gate 3 must fail. It is the `[$field]`
	# collapse that keeps the control at the single ungoverned `medium`.
	multi_governed='{"messages":[{"content":"'"$MARKER"'"},{"content":"plain"}],"output_config":{"effort":"low"}}'
	multi_ungoverned='{"messages":[{"content":"plain"},{"content":"plain"}],"output_config":{"effort":"medium"}}'
	write_view "a=[$multi_governed,$multi_ungoverned]"

	run run_helper "probe_claude_gate_marker '$VIEW' '$MARKER' '.messages[].content'"
	[ "$status" -eq 0 ]

	run run_helper "probe_claude_gate_control '$VIEW' '$MARKER' '.messages[].content'"
	[ "$status" -eq 0 ]
}

@test "gate_control counts an echoed request as ungoverned" {
	# The complement of the above: an echoed request's effort was not set by the
	# fixture, so it belongs in the control set rather than being excluded from it.
	write_view "a=[$(marked low),$(echoed medium)]" "b=[$(marked low),$(ungoverned medium)]"

	run run_helper "probe_claude_gate_control '$VIEW' '$MARKER'"
	[ "$status" -eq 0 ]
}

@test "gate_control passes when ungoverned requests agree on one level" {
	write_view "a=[$(marked low),$(ungoverned medium)]" "b=[$(ungoverned medium)]"

	run run_helper "probe_claude_gate_control '$VIEW' '$MARKER'"
	[ "$status" -eq 0 ]
}

@test "gate_control fails when ungoverned requests disagree" {
	# The assertion compares each arm against this set, so a set that does not
	# agree with itself cannot be read as a baseline.
	write_view "a=[$(marked low),$(ungoverned medium)]" "b=[$(ungoverned high)]"

	run run_helper "probe_claude_gate_control '$VIEW' '$MARKER'"
	[ "$status" -ne 0 ]
	[[ "$output" == *"internally inconsistent"* ]]
}

@test "gate_control fails when no ungoverned request declares an effort" {
	# The loud direction: a Claude that stopped populating `output_config.effort`
	# empties the control set rather than silently comparing against nothing.
	write_view "a=[$(marked low)]" "b=[$(marked low)]"

	run run_helper "probe_claude_gate_control '$VIEW' '$MARKER'"
	[ "$status" -ne 0 ]
	[[ "$output" == *"empty or internally inconsistent"* ]]
}

@test "gate_control excludes the effort-less title sidecar rather than failing on it" {
	# Claude emits an intermittent title-generation request whose `output_config`
	# holds a `format` object and no `effort`. A control stated over all unmarked
	# requests would fail every time it appeared.
	sidecar='{"system":"generate a title","output_config":{"format":{"type":"json_schema"}}}'
	write_view "a=[$(marked low),$(ungoverned medium),$sidecar]" "b=[$(ungoverned medium)]"

	run run_helper "probe_claude_gate_control '$VIEW' '$MARKER'"
	[ "$status" -eq 0 ]
}

@test "assemble_view fails when an arm's sink holds no request files" {
	ws="$BATS_TEST_TMPDIR/ws"
	mkdir -p "$ws/a/sink" "$ws/b/sink"
	printf '{"x":1}\n' >"$ws/a/sink/one.request.json"

	run run_helper "probe_claude_assemble_view '$ws' '$BATS_TEST_TMPDIR/out.json' a b"
	[ "$status" -ne 0 ]
	[[ "$output" == *"captured no request files"* ]]
}

@test "assemble_view counts only *.request.json, so a response-only sink is empty" {
	# The sink writes `<uuid>.response.json` beside each request. A size check or
	# a bare `*.json` glob would read a response-only sink as a populated arm.
	ws="$BATS_TEST_TMPDIR/ws"
	mkdir -p "$ws/a/sink"
	printf '{"x":1}\n' >"$ws/a/sink/one.response.json"

	run run_helper "probe_claude_assemble_view '$ws' '$BATS_TEST_TMPDIR/out.json' a"
	[ "$status" -ne 0 ]
	[[ "$output" == *"captured no request files"* ]]
}

@test "assemble_view builds an arm-keyed object of parsed request bodies" {
	ws="$BATS_TEST_TMPDIR/ws"
	out="$BATS_TEST_TMPDIR/out.json"
	mkdir -p "$ws/a/sink" "$ws/b/sink"
	printf '{"n":1}\n' >"$ws/a/sink/one.request.json"
	printf '{"n":2}\n' >"$ws/a/sink/two.request.json"
	printf '{"n":3}\n' >"$ws/b/sink/three.request.json"
	printf '{"n":99}\n' >"$ws/b/sink/three.response.json"

	run run_helper "probe_claude_assemble_view '$ws' '$out' a b"
	[ "$status" -eq 0 ]
	[ "$(jq -r 'keys | join(",")' "$out")" = "a,b" ]
	[ "$(jq '.a | length' "$out")" -eq 2 ]
	[ "$(jq '.b | length' "$out")" -eq 1 ]
	# Parsed bodies, not strings — the projection indexes into them.
	[ "$(jq -r '.b[0].n' "$out")" = "3" ]
	[ "$(jq '[.a[].n] | sort | join(",")' "$out")" = '"1,2"' ]
}

@test "the committed manifest's projection does not confirm an unmeasured arm" {
	# The gates make a *runner-produced* record safe, but `record.sh --dry-run
	# --view <saved-view>` runs the projection with no gates at all — and that is
	# the workflow both READMEs point authors at for iterating a candidate
	# expression. A projection whose pass value can be reached by an arm that was
	# never measured is therefore reachable in normal use.
	#
	# The degenerate shape: an arm with zero governed requests and an empty
	# control set. `unique` yields `[]` for both, and an unguarded `. == $u`
	# collapses `[] == []` to the same-as-ungoverned pass value.
	#
	# `rel` closes that on its **control-set** branch, not on the arm-empty one:
	# once `($u | length) != 1` has been ruled out, a single-element `$u` can
	# never equal an empty arm, so `arm-had-no-governed-request` is legibility
	# rather than the guard. This view reaches the control-set branch — see the
	# skill package's sibling for one that reaches the other.
	manifest="$EXPERIMENTS/claude-agent-effort/probe.json"
	[ -f "$manifest" ] || skip "claude-agent-effort is not present"

	jq -n --arg m "$MARKER_AGENT" '{
		session_agent: [{system: "plain"}],
		delegated: [{system: $m, output_config: {effort: "low"}}]
	}' >"$BATS_TEST_TMPDIR/degenerate.json"

	run "$EXPERIMENTS/lib/record.sh" --manifest "$manifest" \
		--view "$BATS_TEST_TMPDIR/degenerate.json" --dry-run
	[ "$status" -eq 0 ]
	[[ "$output" != *'"status": "confirmed"'* ]]
}

@test "the committed skill manifest's projection does not confirm an unmeasured arm" {
	# The sibling of the check above, against the skill package's own manifest
	# and its own governed field. The gates make a *runner-produced* record safe,
	# but `record.sh --dry-run --view <saved-view>` runs the projection with no
	# gates at all — and that is the workflow both READMEs point authors at.
	#
	# Which branch of `rel` does the work here is worth stating, because it is
	# not the one the shape suggests: the pass value `. == $u` is only reachable
	# once `($u | length) != 1` has been ruled out, so a single-element `$u` can
	# never equal an empty arm. The **control-set** branch is what makes an
	# unmeasured arm unconfirmable; `arm-had-no-governed-request` is legibility,
	# and the next test is what pins it.
	manifest="$EXPERIMENTS/claude-skill-effort/probe.json"
	[ -f "$manifest" ] || skip "claude-skill-effort is not present"
	# Guarded, not assumed: a moved fixture would leave the marker empty, the
	# fabricated view unmarked, and this test passing for the wrong reason.
	[ -n "$MARKER_SKILL" ]

	jq -n --arg m "$MARKER_SKILL" '{
		inline: [{system: "plain"}],
		slash_entry: [{messages: [{role: "user", content: $m}], output_config: {effort: "low"}}],
		fork: [{messages: [{role: "user", content: $m}], output_config: {effort: "low"}}]
	}' >"$BATS_TEST_TMPDIR/degenerate-skill.json"

	run "$EXPERIMENTS/lib/record.sh" --manifest "$manifest" \
		--view "$BATS_TEST_TMPDIR/degenerate-skill.json" --dry-run
	[ "$status" -eq 0 ]
	[[ "$output" != *'"status": "confirmed"'* ]]
}

@test "the committed skill manifest names an arm that captured no governed request" {
	# The case the test above cannot reach: a control set that *is* single-valued,
	# so `rel` gets past its first branch, with one arm holding nothing governed.
	# Deleting `arm-had-no-governed-request` from the projection renders that arm
	# as a bare `[]` instead — still not confirmed, but no longer saying why. A
	# reader iterating a candidate expression against a saved view is the audience
	# for the difference.
	manifest="$EXPERIMENTS/claude-skill-effort/probe.json"
	[ -f "$manifest" ] || skip "claude-skill-effort is not present"
	[ -n "$MARKER_SKILL" ]

	jq -n --arg m "$MARKER_SKILL" '{
		inline: [{messages: [{role: "user", content: "plain"}], output_config: {effort: "medium"}}],
		slash_entry: [{messages: [{role: "user", content: $m}], output_config: {effort: "low"}}],
		fork: [{messages: [{role: "user", content: $m}], output_config: {effort: "low"}}]
	}' >"$BATS_TEST_TMPDIR/unmeasured-arm.json"

	run "$EXPERIMENTS/lib/record.sh" --manifest "$manifest" \
		--view "$BATS_TEST_TMPDIR/unmeasured-arm.json" --dry-run
	[ "$status" -eq 0 ]
	[[ "$output" != *'"status": "confirmed"'* ]]
	[[ "$output" == *"arm-had-no-governed-request"* ]]
}
