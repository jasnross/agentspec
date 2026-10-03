#!/usr/bin/env bats
# Coverage for the nine gates and view assembly in `probe-claude-otel.sh`.
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

# A subagent request under tool search: `ToolSearch` in `tools[]`, and each MCP
# tool named on its own line in the deferred-tools text block. Arguments are the
# deferred tool names; `MODEL` overrides the model.
deferred_request() {
	jq -n -c --arg m "$MARKER" --arg model "${MODEL:-pinned-model}" '
		{
			model: $model,
			system: $m,
			tools: [{name: "Read"}, {name: "ToolSearch"}],
			messages: [{role: "user", content: [{type: "text",
				text: (["The following deferred tools are now available via ToolSearch:"] + $ARGS.positional | join("\n"))}]}]
		}' --args "$@"
}

# The same subagent with tool search off: every MCP tool sits in `tools[]`.
direct_request() {
	jq -n -c --arg m "$MARKER" '
		{model: "pinned-model", system: $m, tools: ([{name: "Read"}] + ($ARGS.positional | map({name: .})))}
	' --args "$@"
}

@test "gate_deferred_mcp passes when the governed request lists every tool as deferred and carries ToolSearch" {
	write_view "inherit=[$(deferred_request mcp__fx__alpha mcp__fx__beta)]"

	run run_helper "probe_claude_gate_deferred_mcp '$VIEW' '$MARKER' inherit mcp__fx__alpha mcp__fx__beta"
	[ "$status" -eq 0 ]
}

@test "gate_deferred_mcp fails when the tools sit in tools[] instead (tool search off)" {
	# The model-fallback trigger: the package measures default tool search, and a
	# model Claude does not enable it for produces a readable view of something
	# else. The diagnostic must say which, because it decides the next step.
	write_view "inherit=[$(direct_request mcp__fx__alpha mcp__fx__beta)]"

	run run_helper "probe_claude_gate_deferred_mcp '$VIEW' '$MARKER' inherit mcp__fx__alpha mcp__fx__beta"
	[ "$status" -ne 0 ]
	[[ "$output" == *"tool search was not enabled"* ]]
}

@test "gate_deferred_mcp fails when a named tool is missing from the deferred listing" {
	write_view "inherit=[$(deferred_request mcp__fx__alpha)]"

	run run_helper "probe_claude_gate_deferred_mcp '$VIEW' '$MARKER' inherit mcp__fx__alpha mcp__fx__beta"
	[ "$status" -ne 0 ]
	[[ "$output" != *"tool search was not enabled"* ]]
}

@test "gate_deferred_mcp ignores a deferred listing on an ungoverned (main-thread) request" {
	# The main thread carries its own deferred listing. Read on a bare request,
	# it would satisfy the gate for a subagent that never received one.
	main=$(MARKER="main thread" deferred_request mcp__fx__alpha mcp__fx__beta)
	write_view "inherit=[$main,$(direct_request)]"

	run run_helper "probe_claude_gate_deferred_mcp '$VIEW' '$MARKER' inherit mcp__fx__alpha mcp__fx__beta"
	[ "$status" -ne 0 ]
}

@test "gate_deferred_mcp fails when the deferred listing is complete but ToolSearch is absent" {
	# A deferred tool with no `ToolSearch` cannot be loaded, so the listing alone
	# is not default tool search working. Pins the gate's second clause.
	no_search=$(deferred_request mcp__fx__alpha mcp__fx__beta | jq -c '.tools = [{name: "Read"}]')
	write_view "inherit=[$no_search]"

	run run_helper "probe_claude_gate_deferred_mcp '$VIEW' '$MARKER' inherit mcp__fx__alpha mcp__fx__beta"
	[ "$status" -ne 0 ]
}

@test "gate_deferred_mcp fails when the tools sit in tools[] beside ToolSearch, with no listing" {
	# Pins the header match: without it, the walk over every string in the
	# request would read `tools[].name` as a deferred listing.
	direct_with_search=$(direct_request mcp__fx__alpha mcp__fx__beta | jq -c '.tools += [{name: "ToolSearch"}]')
	write_view "inherit=[$direct_with_search]"

	run run_helper "probe_claude_gate_deferred_mcp '$VIEW' '$MARKER' inherit mcp__fx__alpha mcp__fx__beta"
	[ "$status" -ne 0 ]
}

@test "gate_deferred_mcp and gate_model read a .system sent as an array of text blocks" {
	# Real requests send `.system` as an array of blocks; the fabricated views
	# above use a string. Both gates match through `tostring`, which this pins.
	blocks=$(deferred_request mcp__fx__alpha mcp__fx__beta | jq -c '.system = [{type: "text", text: "preamble"}, {type: "text", text: .system}]')
	write_view "inherit=[$blocks]"

	run run_helper "probe_claude_gate_deferred_mcp '$VIEW' '$MARKER' inherit mcp__fx__alpha mcp__fx__beta"
	[ "$status" -eq 0 ]

	run run_helper "probe_claude_gate_model '$VIEW' '$MARKER' pinned-model"
	[ "$status" -eq 0 ]
}

@test "gate_model passes when every governed request carries the pinned model" {
	write_view "a=[$(deferred_request mcp__fx__alpha)]" "b=[$(direct_request)]"

	run run_helper "probe_claude_gate_model '$VIEW' '$MARKER' pinned-model"
	[ "$status" -eq 0 ]
}

@test "gate_model fails when one governed request carries a different model" {
	# A delegation call's own `model` parameter outranks the agent's
	# frontmatter, and only the request shows that it was passed.
	write_view "a=[$(deferred_request mcp__fx__alpha)]" "b=[$(MODEL=other-model deferred_request mcp__fx__alpha)]"

	run run_helper "probe_claude_gate_model '$VIEW' '$MARKER' pinned-model"
	[ "$status" -ne 0 ]
	[[ "$output" == *"arm b: other-model"* ]]
}

@test "gate_model ignores an ungoverned request on a different model" {
	# The main thread's title sidecar runs on another model by design.
	sidecar='{"model":"sidecar-model","system":"generate a title","tools":[]}'
	write_view "a=[$(deferred_request mcp__fx__alpha),$sidecar]"

	run run_helper "probe_claude_gate_model '$VIEW' '$MARKER' pinned-model"
	[ "$status" -eq 0 ]
}

# A main-thread request: unmarked, offering `Agent`, with the arguments listed
# as deferred tools.
main_thread() {
	deferred_request "$@" | jq -c '.system = "main thread" | .tools = [{name: "Agent"}, {name: "ToolSearch"}]'
}

@test "gate_mcp_connected passes when every arm's main thread lists the tools, deferred or direct" {
	direct_main='{"system":"main thread","tools":[{"name":"Agent"},{"name":"mcp__fx__alpha"},{"name":"mcp__fx__beta"}]}'
	write_view "a=[$(main_thread mcp__fx__alpha mcp__fx__beta),$(direct_request)]" "b=[$direct_main]"

	run run_helper "probe_claude_gate_mcp_connected '$VIEW' '$MARKER' mcp__fx__alpha mcp__fx__beta"
	[ "$status" -eq 0 ]
}

@test "gate_mcp_connected fails when one main-thread request lacks a tool" {
	# The server connecting after the delegation would show as a later request
	# listing the tools and an earlier one not; any-one would pass that.
	write_view "a=[$(main_thread mcp__fx__alpha mcp__fx__beta),$(main_thread mcp__fx__alpha)]"

	run run_helper "probe_claude_gate_mcp_connected '$VIEW' '$MARKER' mcp__fx__alpha mcp__fx__beta"
	[ "$status" -ne 0 ]
	[[ "$output" == *"may not have connected"* ]]
}

@test "gate_mcp_connected fails and names an arm with no request offering Agent" {
	write_view "a=[$(main_thread mcp__fx__alpha)]" "b=[$(direct_request mcp__fx__alpha)]"

	run run_helper "probe_claude_gate_mcp_connected '$VIEW' '$MARKER' mcp__fx__alpha"
	[ "$status" -ne 0 ]
	[[ "$output" == *"arm b: no ungoverned request offered Agent"* ]]
}

@test "gate_mcp_connected ignores governed and Agent-less requests" {
	# The subagent's own request and the title sidecar say nothing about the
	# main thread, so an empty tool set on either must not fail the gate.
	sidecar='{"system":"generate a title","tools":[]}'
	write_view "a=[$(main_thread mcp__fx__alpha),$(direct_request),$sidecar]"

	run run_helper "probe_claude_gate_mcp_connected '$VIEW' '$MARKER' mcp__fx__alpha"
	[ "$status" -eq 0 ]
}

@test "gate_mcp_absent passes when no main-thread request lists a tool with the prefix" {
	plugin_main='{"system":"main thread","tools":[{"name":"Agent"},{"name":"mcp__plugin_fxp_fx__alpha"}]}'
	write_view "a=[$plugin_main,$(direct_request)]" "b=[$(main_thread mcp__plugin_fxp_fx__beta)]"

	run run_helper "probe_claude_gate_mcp_absent '$VIEW' '$MARKER' mcp__fx__"
	[ "$status" -eq 0 ]
}

@test "gate_mcp_absent fails and names the tool when a main thread lists it directly" {
	foreign_main='{"system":"main thread","tools":[{"name":"Agent"},{"name":"mcp__fx__alpha"}]}'
	write_view "a=[$(main_thread mcp__plugin_fxp_fx__alpha)]" "b=[$foreign_main]"

	run run_helper "probe_claude_gate_mcp_absent '$VIEW' '$MARKER' mcp__fx__"
	[ "$status" -ne 0 ]
	[[ "$output" == *"arm b: mcp__fx__alpha"* ]]
}

@test "gate_mcp_absent fails when a main thread lists the tool as deferred" {
	write_view "a=[$(main_thread mcp__plugin_fxp_fx__alpha mcp__fx__beta)]"

	run run_helper "probe_claude_gate_mcp_absent '$VIEW' '$MARKER' mcp__fx__"
	[ "$status" -ne 0 ]
	[[ "$output" == *"arm a: mcp__fx__beta"* ]]
}

@test "gate_mcp_absent ignores governed and Agent-less requests" {
	# A subagent's own grant is the finding, not a foreign server, and the title
	# sidecar offers no Agent; neither may fail the gate.
	sidecar='{"system":"generate a title","tools":[{"name":"mcp__fx__alpha"}]}'
	write_view "a=[$(main_thread mcp__plugin_fxp_fx__alpha),$(direct_request mcp__fx__alpha),$sidecar]"

	run run_helper "probe_claude_gate_mcp_absent '$VIEW' '$MARKER' mcp__fx__"
	[ "$status" -eq 0 ]
}

@test "the committed MCP manifest's projection reads both direct and deferred tools" {
	# The two places an MCP tool can reach a request: deferred, named in a text
	# block, or loaded directly into `tools[]`. A projection reading only one
	# would report the other as an absent grant.
	manifest="$EXPERIMENTS/claude-subagent-mcp-tools/probe.json"
	[ -f "$manifest" ] || skip "claude-subagent-mcp-tools is not present"
	MARKER=$(sed -n 's/^\(AGENTSPEC-PROBE-MARKER-[A-Z0-9]*\)$/\1/p' \
		"$EXPERIMENTS/claude-subagent-mcp-tools/fixtures/inherit/.claude/agents/probe-mcp-inherit.md" | head -1)
	[ -n "$MARKER" ]

	deferred=$(deferred_request mcp__fx__alpha)
	direct=$(direct_request mcp__fx__beta)
	write_view "inherit=[$deferred]" "read=[$direct]" "exact=[$deferred]" \
		"exact_search=[$deferred]" "server=[$deferred]" "server_glob=[$deferred]"

	run "$EXPERIMENTS/lib/record.sh" --manifest "$manifest" --view "$VIEW" --dry-run
	[ "$status" -eq 0 ]
	observed=$(printf '%s\n' "$output" | sed -n '/^{/,/^}/p' | jq -c '.assertion.observed')
	[ "$(jq -c '.inherit' <<<"$observed")" = '{"direct":[],"deferred":["mcp__fx__alpha"],"tool_search":true}' ]
	[ "$(jq -c '.read' <<<"$observed")" = '{"direct":["mcp__fx__beta"],"deferred":[],"tool_search":false}' ]
}

@test "the committed MCP manifest's projection does not confirm an unmeasured arm" {
	# `read` expects no MCP tools and no ToolSearch — exactly what an arm with no
	# governed request would project to if the projection reduced it the same
	# way. The sentinel keeps an unmeasured `read` from confirming.
	manifest="$EXPERIMENTS/claude-subagent-mcp-tools/probe.json"
	[ -f "$manifest" ] || skip "claude-subagent-mcp-tools is not present"
	MARKER=$(sed -n 's/^\(AGENTSPEC-PROBE-MARKER-[A-Z0-9]*\)$/\1/p' \
		"$EXPERIMENTS/claude-subagent-mcp-tools/fixtures/inherit/.claude/agents/probe-mcp-inherit.md" | head -1)
	[ -n "$MARKER" ]

	both=$(deferred_request mcp__fx__alpha mcp__fx__beta)
	alpha=$(deferred_request mcp__fx__alpha)
	write_view "inherit=[$both]" 'read=[{"model":"pinned-model","system":"main thread","tools":[]}]' \
		"exact=[$alpha]" "exact_search=[$alpha]" "server=[$both]" "server_glob=[$both]"

	run "$EXPERIMENTS/lib/record.sh" --manifest "$manifest" --view "$VIEW" --dry-run
	[ "$status" -eq 0 ]
	[[ "$output" == *'"status": "refuted"'* ]]
	[ "$(printf '%s\n' "$output" | sed -n '/^{/,/^}/p' | jq -r '.assertion.observed.read')" = "arm-had-no-governed-request" ]
}

# A fabricated sink for one arm under $BATS_TEST_TMPDIR/ws. Each argument is
# `<id>:<mode>:<result>`: a response holding one `Agent` call with that id —
# `run_in_background` set to <mode>, or left out when <mode> is `omit` — and,
# unless <result> is `none`, a request answering it with a `tool_result` that
# opens "Async agent launched" when <result> is `async` and carries a
# subagent's report when it is `sync`. A <result> of `async-string` writes the
# same text as a bare string rather than an array of text blocks.
delegation_sink() {
	local arm="$1"
	shift
	local sink="$BATS_TEST_TMPDIR/ws/$arm/sink" spec id mode result n=0
	mkdir -p "$sink"
	for spec in "$@"; do
		IFS=: read -r id mode result <<<"$spec"
		n=$((n + 1))
		jq -n -c --arg id "$id" --arg mode "$mode" '
			{type: "message", role: "assistant",
			 content: [{type: "tool_use", id: $id, name: "Agent",
				input: ({subagent_type: "x"} + (if $mode == "omit" then {} else {run_in_background: ($mode == "true")} end))}]}
		' >"$sink/res$n.response.json"
		case "$result" in
		none) ;;
		*)
			jq -n -c --arg id "$id" --arg result "$result" '
				(if $result == "sync" then "ok" else "Async agent launched successfully." end) as $text
				| {system: "main thread", messages: [{role: "user", content: [{type: "tool_result", tool_use_id: $id,
					content: (if $result == "async-string" then $text else [{type: "text", text: $text}] end)}]}]}
			' >"$sink/req$n.request.json"
			;;
		esac
	done
}

@test "gate_delegation_background passes for true when every call asked for and ran in the background" {
	delegation_sink bg t1:true:async t2:true:async

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' bg true"
	[ "$status" -eq 0 ]
}

@test "gate_delegation_background passes for false when the call set false and ran synchronously" {
	delegation_sink fg t1:false:sync

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' fg false"
	[ "$status" -eq 0 ]
}

@test "gate_delegation_background fails for false when the field is absent and the call ran async" {
	# Measured on 2.1.287: a call with no `run_in_background` launched
	# asynchronously, so absence is not the foreground. Both clauses reject this.
	delegation_sink fg t1:omit:async

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' fg false"
	[ "$status" -ne 0 ]
	[[ "$output" == *"did not delegate with run_in_background=false"* ]]
}

@test "gate_delegation_background fails when the call asked for false but ran asynchronously" {
	delegation_sink fg t1:false:async

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' fg false"
	[ "$status" -ne 0 ]
}

@test "gate_delegation_background fails when the arm made no Agent call" {
	mkdir -p "$BATS_TEST_TMPDIR/ws/bg/sink"
	printf '{"type":"message","role":"assistant","content":[{"type":"text","text":"ok"}]}' >"$BATS_TEST_TMPDIR/ws/bg/sink/r.response.json"
	printf '{"system":"main thread","messages":[{"role":"user","content":"hi"}]}' >"$BATS_TEST_TMPDIR/ws/bg/sink/q.request.json"

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' bg true"
	[ "$status" -ne 0 ]
}

@test "gate_delegation_background fails when one of two calls disagrees" {
	delegation_sink bg t1:true:async t2:false:sync

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' bg true"
	[ "$status" -ne 0 ]
}

@test "gate_delegation_background fails when a call was never answered" {
	delegation_sink bg t1:true:async t2:true:none

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' bg true"
	[ "$status" -ne 0 ]
}

@test "gate_delegation_background fails for true when the field is absent even though the call ran async" {
	# The realistic slip for a background arm: the model drops the field and the
	# asynchronous default runs it anyway. Only the strict-boolean clause sees it.
	delegation_sink bg t1:omit:async

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' bg true"
	[ "$status" -ne 0 ]
}

@test "gate_delegation_background fails for false when the field is absent even though the call ran synchronously" {
	delegation_sink fg t1:omit:sync

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' fg false"
	[ "$status" -ne 0 ]
}

@test "gate_delegation_background fails when the call asked for true but ran synchronously" {
	delegation_sink bg t1:true:sync

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' bg true"
	[ "$status" -ne 0 ]
}

@test "gate_delegation_background reads a tool result whose content is a bare string" {
	delegation_sink bg t1:true:async-string

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' bg true"
	[ "$status" -eq 0 ]
}

@test "gate_delegation_background fails and says so when the sink holds no responses" {
	mkdir -p "$BATS_TEST_TMPDIR/ws/bg/sink"
	printf '{"system":"main thread","messages":[]}' >"$BATS_TEST_TMPDIR/ws/bg/sink/q.request.json"

	run run_helper "probe_claude_gate_delegation_background '$BATS_TEST_TMPDIR/ws' bg true"
	[ "$status" -ne 0 ]
	[[ "$output" == *"captured 0 response and 1 request files"* ]]
}

# A fabricated sink for one arm under $BATS_TEST_TMPDIR/ws holding one
# response whose assistant turn calls <tool> with id `c1`, and — unless the
# third argument is `unanswered` — a request answering it.
tool_call_sink() {
	local arm="$1" tool="$2" answer="${3:-answered}"
	local sink="$BATS_TEST_TMPDIR/ws/$arm/sink"
	mkdir -p "$sink"
	jq -n -c --arg tool "$tool" \
		'{type: "message", role: "assistant", content: [{type: "tool_use", id: "c1", name: $tool, input: {}}]}' \
		>"$sink/r.response.json"
	case "$answer" in
	unanswered)
		printf '{"system":"x","messages":[{"role":"user","content":"hi"}]}' >"$sink/q.request.json"
		;;
	*)
		printf '{"system":"x","messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"ok"}]}]}' \
			>"$sink/q.request.json"
		;;
	esac
}

@test "gate_tool_answered passes when the named tool was called and answered" {
	tool_call_sink a mcp__fx__alpha

	run run_helper "probe_claude_gate_tool_answered '$BATS_TEST_TMPDIR/ws' a mcp__fx__alpha"
	[ "$status" -eq 0 ]
}

@test "gate_tool_answered fails when the call has no tool_result" {
	tool_call_sink a mcp__fx__alpha unanswered

	run run_helper "probe_claude_gate_tool_answered '$BATS_TEST_TMPDIR/ws' a mcp__fx__alpha"
	[ "$status" -ne 0 ]
	[[ "$output" == *"no answered call to mcp__fx__alpha"* ]]
}

@test "gate_tool_answered fails when only a different tool was called" {
	tool_call_sink a ToolSearch

	run run_helper "probe_claude_gate_tool_answered '$BATS_TEST_TMPDIR/ws' a mcp__fx__alpha"
	[ "$status" -ne 0 ]
}

@test "gate_tool_answered fails when the name appears only in a user message's text" {
	sink="$BATS_TEST_TMPDIR/ws/a/sink"
	mkdir -p "$sink"
	printf '{"type":"message","role":"assistant","content":[{"type":"text","text":"done"}]}' >"$sink/r.response.json"
	printf '{"system":"x","messages":[{"role":"user","content":[{"type":"text","text":"Call the mcp__fx__alpha tool"}]}]}' \
		>"$sink/q.request.json"

	run run_helper "probe_claude_gate_tool_answered '$BATS_TEST_TMPDIR/ws' a mcp__fx__alpha"
	[ "$status" -ne 0 ]
}

@test "gate_tool_answered fails and says so when the sink holds no responses" {
	mkdir -p "$BATS_TEST_TMPDIR/ws/a/sink"
	printf '{"system":"x","messages":[]}' >"$BATS_TEST_TMPDIR/ws/a/sink/q.request.json"

	run run_helper "probe_claude_gate_tool_answered '$BATS_TEST_TMPDIR/ws' a mcp__fx__alpha"
	[ "$status" -ne 0 ]
	[[ "$output" == *"captured 0 response and 1 request files"* ]]
}

@test "gate_tool_answered fails when the only tool_result answers a different call" {
	# The shape that matters in practice: the request answers the ToolSearch
	# call, and the call to the named tool goes unanswered.
	sink="$BATS_TEST_TMPDIR/ws/a/sink"
	mkdir -p "$sink"
	printf '{"type":"message","role":"assistant","content":[{"type":"tool_use","id":"s1","name":"ToolSearch","input":{}},{"type":"tool_use","id":"c1","name":"mcp__fx__alpha","input":{}}]}' \
		>"$sink/r.response.json"
	printf '{"system":"x","messages":[{"role":"user","content":[{"type":"tool_result","tool_use_id":"s1","content":"loaded"}]}]}' \
		>"$sink/q.request.json"

	run run_helper "probe_claude_gate_tool_answered '$BATS_TEST_TMPDIR/ws' a mcp__fx__alpha"
	[ "$status" -ne 0 ]
	[[ "$output" == *"no answered call to mcp__fx__alpha"* ]]
}

@test "gate_tool_answered fails and says so when the sink holds no requests" {
	sink="$BATS_TEST_TMPDIR/ws/a/sink"
	mkdir -p "$sink"
	printf '{"type":"message","role":"assistant","content":[{"type":"tool_use","id":"c1","name":"mcp__fx__alpha","input":{}}]}' \
		>"$sink/r.response.json"

	run run_helper "probe_claude_gate_tool_answered '$BATS_TEST_TMPDIR/ws' a mcp__fx__alpha"
	[ "$status" -ne 0 ]
	[[ "$output" == *"captured 1 response and 0 request files"* ]]
}

