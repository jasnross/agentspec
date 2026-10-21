"""Coverage for the gates in `probe_harness.gates.claude_otel`.

Every test drives fabricated views and fabricated sink directories, so the suite runs with
no `claude` on PATH and costs nothing. That is the point of extracting the gates into
`lib/`: duplicated in two runners, the only thing that would ever exercise them is a real,
paid run — an untested control on a billed apparatus.
"""

from pathlib import Path
from typing import Any

import pytest
from probe_harness.gates import GateFailure, bodies, claude_otel
from probe_harness.gates.bodies import ClaudeArmBodies, MarkerField
from probe_harness.views import SinkEntry, SinkValues

from tests.conftest import (
    MARKER,
    claude_request,
    compact,
    deferred_request,
    direct_request,
)


def marked(effort: str) -> dict[str, Any]:
    """A request body governed by the fixture: the fixture's text is its system prompt."""
    return claude_request(system=MARKER, output_config={"effort": effort})


def echoed(effort: str) -> dict[str, Any]:
    """A request that merely quotes the marker back in a tool result.

    The shape a main thread takes after a subagent replies. Not governed by the fixture:
    its effort is the ungoverned level, and it must not be read as the arm's value.
    """
    return claude_request(
        system="main thread",
        messages=[{"role": "user", "content": MARKER}],
        output_config={"effort": effort},
    )


def governed_in_messages(effort: str) -> dict[str, Any]:
    """A request body governed by a *skill* fixture, whose body lands in `messages[]`."""
    return claude_request(
        system="plain",
        messages=[{"role": "user", "content": MARKER}],
        output_config={"effort": effort},
    )


def ungoverned(effort: str) -> dict[str, Any]:
    """A request body nothing governed: no marker, carries an effort."""
    return claude_request(system="plain", output_config={"effort": effort})


def main_thread(*tools: str) -> dict[str, Any]:
    """A main-thread request: unmarked, offering `Agent`, the tools listed as deferred."""
    request = deferred_request(*tools)
    request["system"] = "main thread"
    request["tools"] = [{"name": "Agent"}, {"name": "ToolSearch"}]
    return request


def sink_dir(ws: Path, arm: str) -> Path:
    sink = ws / arm / "sink"
    sink.mkdir(parents=True, exist_ok=True)
    return sink


def delegation_sink(ws: Path, arm: str, *specs: str) -> None:
    """A fabricated sink for one arm under <ws>.

    Each spec is `<id>:<mode>:<result>`: a response holding one `Agent` call with that id —
    `run_in_background` set to <mode>, or left out when <mode> is `omit` — and, unless
    <result> is `none`, a request answering it with a `tool_result` that opens "Async agent
    launched" when <result> is `async` and carries a subagent's report when it is `sync`. A
    <result> of `async-string` writes the same text as a bare string rather than an array
    of text blocks.
    """
    sink = sink_dir(ws, arm)
    for n, spec in enumerate(specs, start=1):
        call_id, mode, result = spec.split(":")
        agent_input: dict[str, Any] = {"subagent_type": "x"}
        if mode != "omit":
            agent_input["run_in_background"] = mode == "true"
        response = {
            "type": "message",
            "role": "assistant",
            "content": [{"type": "tool_use", "id": call_id, "name": "Agent", "input": agent_input}],
        }
        (sink / f"res{n}.response.json").write_text(compact(response) + "\n", encoding="utf-8")
        if result == "none":
            continue
        text = "ok" if result == "sync" else "Async agent launched successfully."
        content: Any = text if result == "async-string" else [{"type": "text", "text": text}]
        request = claude_request(
            system="main thread",
            messages=[
                {
                    "role": "user",
                    "content": [
                        {"type": "tool_result", "tool_use_id": call_id, "content": content}
                    ],
                }
            ],
        )
        (sink / f"req{n}.request.json").write_text(compact(request) + "\n", encoding="utf-8")


def agent_schema_request(ws: Path, arm: str, param: str) -> None:
    """Add one main-thread request offering the `Agent` tool to the arm's sink.

    Its input schema declares `run_in_background` when <param> is `with` and leaves it out
    when it is `without`, as fork mode does.
    """
    props: dict[str, Any] = {"prompt": {"type": "string"}, "subagent_type": {"type": "string"}}
    if param == "with":
        props["run_in_background"] = {"type": "boolean"}
    request = claude_request(
        system="main thread",
        messages=[{"role": "user", "content": "delegate"}],
        tools=[
            {"name": "Read", "input_schema": {"type": "object", "properties": {}}},
            {"name": "Agent", "input_schema": {"type": "object", "properties": props}},
        ],
    )
    sink = sink_dir(ws, arm)
    (sink / "schema.request.json").write_text(compact(request) + "\n", encoding="utf-8")


def tool_call_sink(ws: Path, arm: str, tool: str, answer: str = "answered") -> None:
    """One response calling <tool> with id `c1`, and — unless `unanswered` — its answer."""
    sink = sink_dir(ws, arm)
    response = {
        "type": "message",
        "role": "assistant",
        "content": [{"type": "tool_use", "id": "c1", "name": tool, "input": {}}],
    }
    (sink / "r.response.json").write_text(compact(response) + "\n", encoding="utf-8")
    content: Any = (
        "hi"
        if answer == "unanswered"
        else [{"type": "tool_result", "tool_use_id": "c1", "content": "ok"}]
    )
    request = claude_request(system="x", messages=[{"role": "user", "content": content}])
    (sink / "q.request.json").write_text(compact(request), encoding="utf-8")


def arms_from_view(
    view: dict[str, list[Any]], responses: dict[str, list[Any]] | None = None
) -> dict[str, ClaudeArmBodies]:
    """Each arm's fabricated request bodies, and any response bodies, validated as a judge does."""
    return bodies.claude_arms(
        {
            arm: SinkValues(
                requests=_entries(arm, "request", requests),
                responses=_entries(arm, "response", (responses or {}).get(arm, [])),
            )
            for arm, requests in view.items()
        },
        Path(),
    )


def _entries(arm: str, kind: str, values: list[Any]) -> list[SinkEntry]:
    return [
        SinkEntry(Path(arm) / "sink" / f"{n:04}.{kind}.json", value)
        for n, value in enumerate(values)
    ]


def arms_from_ws(ws: Path, *arms: str) -> dict[str, ClaudeArmBodies]:
    """The fabricated sinks under <ws>, read and parsed as a judge reads them."""
    return bodies.read_claude_arms(ws, arms)


def failure_output(exc: pytest.ExceptionInfo[GateFailure]) -> str:
    """The lines the gate would have printed to stderr."""
    return "\n".join(exc.value.lines)


def test_gate_marker_passes_when_every_arm_holds_a_marked_request(tmp_path: Path) -> None:
    arms = arms_from_view({"a": [marked("low"), ungoverned("medium")], "b": [marked("low")]})

    claude_otel.marker(arms, MARKER)


def test_gate_marker_fails_when_one_arm_holds_none(tmp_path: Path) -> None:
    # The failure that makes the inert arm assertable: without this gate, "the fixture
    # never engaged" reads identically to "Claude discarded its effort."
    arms = arms_from_view({"a": [marked("low")], "b": [ungoverned("medium")]})

    with pytest.raises(GateFailure) as exc:
        claude_otel.marker(arms, MARKER)
    assert "never engaged the fixture" in failure_output(exc)


def test_gate_marker_fails_when_an_arm_only_echoes_the_marker_in_a_tool_result(
    tmp_path: Path,
) -> None:
    # The failure the `.system` narrowing exists for, measured on a real run: a subagent's
    # reply carries the fixture's text back to the main thread, on a request the fixture
    # governs not at all. Matched on `tostring`, that echo would satisfy this gate for an
    # arm whose fixture never engaged.
    arms = arms_from_view({"a": [marked("low")], "b": [echoed("medium")]})

    with pytest.raises(GateFailure) as exc:
        claude_otel.marker(arms, MARKER)
    assert ".system carries the fixture marker" in failure_output(exc)


def test_gate_marker_scopes_to_the_field_it_is_given(tmp_path: Path) -> None:
    # A skill's body never reaches `.system`; measured at 2.1.232 it arrives in
    # `messages[]`. The default would read that arm as never having engaged, so
    # `claude-skill-effort` names the field instead of the library guessing.
    arms = arms_from_view(
        {"a": [governed_in_messages("low")], "b": [governed_in_messages("low")]},
    )

    with pytest.raises(GateFailure):
        claude_otel.marker(arms, MARKER)

    claude_otel.marker(arms, MARKER, MarkerField.MESSAGES)


def test_gate_control_s_control_set_is_the_complement_of_the_field_it_is_given(
    tmp_path: Path,
) -> None:
    # The two gates partition the requests between them, so they must be passed the same
    # field — a control set computed under one definition and an arm value computed under
    # the other would not describe the same run.
    #
    # Under `.messages` the two governed requests are excluded and the control is the
    # single ungoverned `medium`. Under `.system` nothing is excluded, so the governed
    # `low`s join the control set and it no longer agrees with itself.
    arms = arms_from_view(
        {
            "a": [governed_in_messages("low"), ungoverned("medium")],
            "b": [governed_in_messages("low")],
        },
    )

    claude_otel.control(arms, MARKER, MarkerField.MESSAGES)

    with pytest.raises(GateFailure) as exc:
        claude_otel.control(arms, MARKER)
    assert "internally inconsistent" in failure_output(exc)


def test_gate_control_does_not_admit_a_request_governed_by_one_of_its_messages(
    tmp_path: Path,
) -> None:
    # A request is governed as a whole, whichever of its messages carries the marker. Judged
    # once per message, the messages lacking it would admit a *governed* request to the
    # control set — putting the arm's own level into the control the projection compares
    # it against. Gate 2 passes throughout, so the contamination would be silent.
    #
    # Here the two requests disagree, so a control set that admitted the governed `low`
    # would be inconsistent and gate 3 would fail.
    multi_governed = claude_request(
        messages=[{"role": "user", "content": MARKER}, {"role": "user", "content": "plain"}],
        output_config={"effort": "low"},
    )
    multi_ungoverned = claude_request(
        messages=[{"role": "user", "content": "plain"}, {"role": "user", "content": "plain"}],
        output_config={"effort": "medium"},
    )
    arms = arms_from_view({"a": [multi_governed, multi_ungoverned]})

    claude_otel.marker(arms, MARKER, MarkerField.MESSAGES)

    claude_otel.control(arms, MARKER, MarkerField.MESSAGES)


def test_gate_control_counts_an_echoed_request_as_ungoverned(tmp_path: Path) -> None:
    # The complement of the above: an echoed request's effort was not set by the fixture,
    # so it belongs in the control set rather than being excluded from it.
    arms = arms_from_view(
        {"a": [marked("low"), echoed("medium")], "b": [marked("low"), ungoverned("medium")]},
    )

    claude_otel.control(arms, MARKER)


def test_gate_control_passes_when_ungoverned_requests_agree_on_one_level(tmp_path: Path) -> None:
    arms = arms_from_view(
        {"a": [marked("low"), ungoverned("medium")], "b": [ungoverned("medium")]},
    )

    claude_otel.control(arms, MARKER)


def test_gate_control_fails_when_ungoverned_requests_disagree(tmp_path: Path) -> None:
    # The assertion compares each arm against this set, so a set that does not agree with
    # itself cannot be read as a baseline.
    arms = arms_from_view(
        {"a": [marked("low"), ungoverned("medium")], "b": [ungoverned("high")]},
    )

    with pytest.raises(GateFailure) as exc:
        claude_otel.control(arms, MARKER)
    assert "internally inconsistent" in failure_output(exc)


def test_gate_control_fails_when_no_ungoverned_request_declares_an_effort(tmp_path: Path) -> None:
    # The loud direction: a Claude that stopped populating `output_config.effort` empties
    # the control set rather than silently comparing against nothing.
    arms = arms_from_view({"a": [marked("low")], "b": [marked("low")]})

    with pytest.raises(GateFailure) as exc:
        claude_otel.control(arms, MARKER)
    assert "empty or internally inconsistent" in failure_output(exc)


def test_gate_control_excludes_the_effort_less_title_sidecar_rather_than_failing_on_it(
    tmp_path: Path,
) -> None:
    # Claude emits an intermittent title-generation request whose `output_config` holds a
    # `format` object and no `effort`. A control stated over all unmarked requests would
    # fail every time it appeared.
    sidecar = claude_request(
        system="generate a title",
        output_config={"format": {"type": "json_schema"}},
    )
    arms = arms_from_view(
        {"a": [marked("low"), ungoverned("medium"), sidecar], "b": [ungoverned("medium")]},
    )

    claude_otel.control(arms, MARKER)


def test_gate_deferred_mcp_passes_when_the_governed_request_lists_every_tool_as_deferred_and_carries_toolsearch(  # noqa: E501
    tmp_path: Path,
) -> None:
    arms = arms_from_view({"inherit": [deferred_request("mcp__fx__alpha", "mcp__fx__beta")]})

    claude_otel.deferred_mcp(arms, MARKER, "inherit", ["mcp__fx__alpha", "mcp__fx__beta"])


def test_gate_deferred_mcp_fails_when_the_tools_sit_in_tools_instead_tool_search_off(
    tmp_path: Path,
) -> None:
    # The model-fallback trigger: the package measures default tool search, and a model
    # Claude does not enable it for produces a readable view of something else. The
    # diagnostic must say which, because it decides the next step.
    arms = arms_from_view({"inherit": [direct_request("mcp__fx__alpha", "mcp__fx__beta")]})

    with pytest.raises(GateFailure) as exc:
        claude_otel.deferred_mcp(arms, MARKER, "inherit", ["mcp__fx__alpha", "mcp__fx__beta"])
    assert "tool search was not enabled" in failure_output(exc)


def test_gate_deferred_mcp_fails_when_a_named_tool_is_missing_from_the_deferred_listing(
    tmp_path: Path,
) -> None:
    arms = arms_from_view({"inherit": [deferred_request("mcp__fx__alpha")]})

    with pytest.raises(GateFailure) as exc:
        claude_otel.deferred_mcp(arms, MARKER, "inherit", ["mcp__fx__alpha", "mcp__fx__beta"])
    assert "tool search was not enabled" not in failure_output(exc)


def test_gate_deferred_mcp_ignores_a_deferred_listing_on_an_ungoverned_main_thread_request(
    tmp_path: Path,
) -> None:
    # The main thread carries its own deferred listing. Read on a bare request, it would
    # satisfy the gate for a subagent that never received one.
    main = deferred_request("mcp__fx__alpha", "mcp__fx__beta", marker="main thread")
    arms = arms_from_view({"inherit": [main, direct_request()]})

    with pytest.raises(GateFailure):
        claude_otel.deferred_mcp(arms, MARKER, "inherit", ["mcp__fx__alpha", "mcp__fx__beta"])


def test_gate_deferred_mcp_fails_when_the_deferred_listing_is_complete_but_toolsearch_is_absent(
    tmp_path: Path,
) -> None:
    # A deferred tool with no `ToolSearch` cannot be loaded, so the listing alone is not
    # default tool search working. Pins the gate's second clause.
    no_search = deferred_request("mcp__fx__alpha", "mcp__fx__beta")
    no_search["tools"] = [{"name": "Read"}]
    arms = arms_from_view({"inherit": [no_search]})

    with pytest.raises(GateFailure):
        claude_otel.deferred_mcp(arms, MARKER, "inherit", ["mcp__fx__alpha", "mcp__fx__beta"])


def test_gate_deferred_mcp_fails_when_the_tools_sit_in_tools_beside_toolsearch_with_no_listing(
    tmp_path: Path,
) -> None:
    # Pins the header match: without it, the walk over every string in the request would
    # read `tools[].name` as a deferred listing.
    direct_with_search = direct_request("mcp__fx__alpha", "mcp__fx__beta")
    direct_with_search["tools"] += [{"name": "ToolSearch"}]
    arms = arms_from_view({"inherit": [direct_with_search]})

    with pytest.raises(GateFailure):
        claude_otel.deferred_mcp(arms, MARKER, "inherit", ["mcp__fx__alpha", "mcp__fx__beta"])


def test_gate_deferred_mcp_and_gate_model_read_a_system_sent_as_an_array_of_text_blocks(
    tmp_path: Path,
) -> None:
    # Real requests send `.system` as an array of blocks; the fabricated views above use a
    # string. Both gates match through `tostring`, which this pins.
    blocks = deferred_request("mcp__fx__alpha", "mcp__fx__beta")
    blocks["system"] = [
        {"type": "text", "text": "preamble"},
        {"type": "text", "text": blocks["system"]},
    ]
    arms = arms_from_view({"inherit": [blocks]})

    claude_otel.deferred_mcp(arms, MARKER, "inherit", ["mcp__fx__alpha", "mcp__fx__beta"])

    claude_otel.model(arms, MARKER, "pinned-model")


def test_gate_model_passes_when_every_governed_request_carries_the_pinned_model(
    tmp_path: Path,
) -> None:
    arms = arms_from_view({"a": [deferred_request("mcp__fx__alpha")], "b": [direct_request()]})

    claude_otel.model(arms, MARKER, "pinned-model")


def test_gate_model_fails_when_one_governed_request_carries_a_different_model(
    tmp_path: Path,
) -> None:
    # A delegation call's own `model` parameter outranks the agent's frontmatter, and only
    # the request shows that it was passed.
    arms = arms_from_view(
        {
            "a": [deferred_request("mcp__fx__alpha")],
            "b": [deferred_request("mcp__fx__alpha", model="other-model")],
        },
    )

    with pytest.raises(GateFailure) as exc:
        claude_otel.model(arms, MARKER, "pinned-model")
    assert "arm b: other-model" in failure_output(exc)


def test_gate_model_ignores_an_ungoverned_request_on_a_different_model(tmp_path: Path) -> None:
    # The main thread's title sidecar runs on another model by design.
    sidecar = claude_request(model="sidecar-model", system="generate a title", tools=[])
    arms = arms_from_view({"a": [deferred_request("mcp__fx__alpha"), sidecar]})

    claude_otel.model(arms, MARKER, "pinned-model")


def test_gate_mcp_connected_passes_when_every_arm_s_main_thread_lists_the_tools_deferred_or_direct(
    tmp_path: Path,
) -> None:
    direct_main = claude_request(
        system="main thread",
        tools=[{"name": "Agent"}, {"name": "mcp__fx__alpha"}, {"name": "mcp__fx__beta"}],
    )
    arms = arms_from_view(
        {
            "a": [main_thread("mcp__fx__alpha", "mcp__fx__beta"), direct_request()],
            "b": [direct_main],
        },
    )

    claude_otel.mcp_connected(arms, MARKER, ["mcp__fx__alpha", "mcp__fx__beta"])


def test_gate_mcp_connected_fails_when_one_main_thread_request_lacks_a_tool(
    tmp_path: Path,
) -> None:
    # The server connecting after the delegation would show as a later request listing the
    # tools and an earlier one not; any-one would pass that.
    arms = arms_from_view(
        {"a": [main_thread("mcp__fx__alpha", "mcp__fx__beta"), main_thread("mcp__fx__alpha")]},
    )

    with pytest.raises(GateFailure) as exc:
        claude_otel.mcp_connected(arms, MARKER, ["mcp__fx__alpha", "mcp__fx__beta"])
    assert "may not have connected" in failure_output(exc)


def test_gate_mcp_connected_fails_and_names_an_arm_with_no_request_offering_agent(
    tmp_path: Path,
) -> None:
    arms = arms_from_view(
        {"a": [main_thread("mcp__fx__alpha")], "b": [direct_request("mcp__fx__alpha")]},
    )

    with pytest.raises(GateFailure) as exc:
        claude_otel.mcp_connected(arms, MARKER, ["mcp__fx__alpha"])
    assert "arm b: no ungoverned request offered Agent" in failure_output(exc)


def test_gate_mcp_connected_ignores_governed_and_agent_less_requests(tmp_path: Path) -> None:
    # The subagent's own request and the title sidecar say nothing about the main thread,
    # so an empty tool set on either must not fail the gate.
    sidecar = claude_request(system="generate a title", tools=[])
    arms = arms_from_view(
        {"a": [main_thread("mcp__fx__alpha"), direct_request(), sidecar]},
    )

    claude_otel.mcp_connected(arms, MARKER, ["mcp__fx__alpha"])


def test_gate_mcp_absent_passes_when_no_main_thread_request_lists_a_tool_with_the_prefix(
    tmp_path: Path,
) -> None:
    plugin_main = claude_request(
        system="main thread",
        tools=[{"name": "Agent"}, {"name": "mcp__plugin_fxp_fx__alpha"}],
    )
    arms = arms_from_view(
        {"a": [plugin_main, direct_request()], "b": [main_thread("mcp__plugin_fxp_fx__beta")]},
    )

    claude_otel.mcp_absent(arms, MARKER, "mcp__fx__")


def test_gate_mcp_absent_fails_and_names_the_tool_when_a_main_thread_lists_it_directly(
    tmp_path: Path,
) -> None:
    foreign_main = claude_request(
        system="main thread",
        tools=[{"name": "Agent"}, {"name": "mcp__fx__alpha"}],
    )
    arms = arms_from_view(
        {"a": [main_thread("mcp__plugin_fxp_fx__alpha")], "b": [foreign_main]},
    )

    with pytest.raises(GateFailure) as exc:
        claude_otel.mcp_absent(arms, MARKER, "mcp__fx__")
    assert "arm b: mcp__fx__alpha" in failure_output(exc)


def test_gate_mcp_absent_fails_when_a_main_thread_lists_the_tool_as_deferred(
    tmp_path: Path,
) -> None:
    arms = arms_from_view({"a": [main_thread("mcp__plugin_fxp_fx__alpha", "mcp__fx__beta")]})

    with pytest.raises(GateFailure) as exc:
        claude_otel.mcp_absent(arms, MARKER, "mcp__fx__")
    assert "arm a: mcp__fx__beta" in failure_output(exc)


def test_gate_mcp_absent_ignores_governed_and_agent_less_requests(tmp_path: Path) -> None:
    # A subagent's own grant is the finding, not a foreign server, and the title sidecar
    # offers no Agent; neither may fail the gate.
    sidecar = claude_request(system="generate a title", tools=[{"name": "mcp__fx__alpha"}])
    arms = arms_from_view(
        {
            "a": [
                main_thread("mcp__plugin_fxp_fx__alpha"),
                direct_request("mcp__fx__alpha"),
                sidecar,
            ]
        },
    )

    claude_otel.mcp_absent(arms, MARKER, "mcp__fx__")


def test_gate_delegation_background_passes_for_true_when_every_call_asked_for_and_ran_in_the_background(  # noqa: E501
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    delegation_sink(ws, "bg", "t1:true:async", "t2:true:async")

    claude_otel.delegation_background(arms_from_ws(ws, "bg"), "bg", True)


def test_gate_delegation_background_passes_for_false_when_the_call_set_false_and_ran_synchronously(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    delegation_sink(ws, "fg", "t1:false:sync")

    claude_otel.delegation_background(arms_from_ws(ws, "fg"), "fg", False)


def test_gate_delegation_background_fails_for_false_when_the_field_is_absent_and_the_call_ran_async(
    tmp_path: Path,
) -> None:
    # Measured on 2.1.287: a call with no `run_in_background` launched asynchronously, so
    # absence is not the foreground. Both clauses reject this.
    ws = tmp_path / "ws"
    delegation_sink(ws, "fg", "t1:omit:async")

    with pytest.raises(GateFailure) as exc:
        claude_otel.delegation_background(arms_from_ws(ws, "fg"), "fg", False)
    assert "did not delegate with run_in_background=false" in failure_output(exc)


def test_gate_delegation_background_fails_when_the_call_asked_for_false_but_ran_asynchronously(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    delegation_sink(ws, "fg", "t1:false:async")

    with pytest.raises(GateFailure):
        claude_otel.delegation_background(arms_from_ws(ws, "fg"), "fg", False)


def test_gate_delegation_background_fails_when_the_arm_made_no_agent_call(tmp_path: Path) -> None:
    ws = tmp_path / "ws"
    sink = sink_dir(ws, "bg")
    (sink / "r.response.json").write_text(
        '{"type":"message","role":"assistant","content":[{"type":"text","text":"ok"}]}',
        encoding="utf-8",
    )
    request = claude_request(system="main thread", messages=[{"role": "user", "content": "hi"}])
    (sink / "q.request.json").write_text(compact(request), encoding="utf-8")

    with pytest.raises(GateFailure):
        claude_otel.delegation_background(arms_from_ws(ws, "bg"), "bg", True)


def test_gate_delegation_background_fails_when_one_of_two_calls_disagrees(tmp_path: Path) -> None:
    ws = tmp_path / "ws"
    delegation_sink(ws, "bg", "t1:true:async", "t2:false:sync")

    with pytest.raises(GateFailure):
        claude_otel.delegation_background(arms_from_ws(ws, "bg"), "bg", True)


def test_gate_delegation_background_fails_when_a_call_was_never_answered(tmp_path: Path) -> None:
    ws = tmp_path / "ws"
    delegation_sink(ws, "bg", "t1:true:async", "t2:true:none")

    with pytest.raises(GateFailure):
        claude_otel.delegation_background(arms_from_ws(ws, "bg"), "bg", True)


def test_gate_delegation_background_fails_for_true_when_the_field_is_absent_even_though_the_call_ran_async(  # noqa: E501
    tmp_path: Path,
) -> None:
    # The realistic slip for a background arm: the model drops the field and the
    # asynchronous default runs it anyway. Only the strict-boolean clause sees it.
    ws = tmp_path / "ws"
    delegation_sink(ws, "bg", "t1:omit:async")

    with pytest.raises(GateFailure):
        claude_otel.delegation_background(arms_from_ws(ws, "bg"), "bg", True)


def test_gate_delegation_background_fails_for_false_when_the_field_is_absent_even_though_the_call_ran_synchronously(  # noqa: E501
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    delegation_sink(ws, "fg", "t1:omit:sync")

    with pytest.raises(GateFailure):
        claude_otel.delegation_background(arms_from_ws(ws, "fg"), "fg", False)


def test_gate_delegation_background_does_not_read_a_number_as_true(tmp_path: Path) -> None:
    ws = tmp_path / "ws"
    delegation_sink(ws, "bg", "t1:true:async")
    response = ws / "bg/sink/res1.response.json"
    response.write_text(
        response.read_text().replace('"run_in_background":true', '"run_in_background":1')
    )

    with pytest.raises(GateFailure):
        claude_otel.delegation_background(arms_from_ws(ws, "bg"), "bg", True)


def test_gate_delegation_background_fails_when_the_call_asked_for_true_but_ran_synchronously(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    delegation_sink(ws, "bg", "t1:true:sync")

    with pytest.raises(GateFailure):
        claude_otel.delegation_background(arms_from_ws(ws, "bg"), "bg", True)


def test_gate_delegation_background_reads_a_tool_result_whose_content_is_a_bare_string(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    delegation_sink(ws, "bg", "t1:true:async-string")

    claude_otel.delegation_background(arms_from_ws(ws, "bg"), "bg", True)


def test_gate_delegation_background_fails_and_says_so_when_the_sink_holds_no_responses(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    sink = sink_dir(ws, "bg")
    (sink / "q.request.json").write_text(
        compact(claude_request(system="main thread")), encoding="utf-8"
    )

    with pytest.raises(GateFailure) as exc:
        claude_otel.delegation_background(arms_from_ws(ws, "bg"), "bg", True)
    assert "captured 0 response and 1 request files" in failure_output(exc)


def test_gate_delegation_fork_passes_when_the_agent_schema_lacks_run_in_background_and_every_call_ran_in_the_background(  # noqa: E501
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    agent_schema_request(ws, "fork", "without")
    delegation_sink(ws, "fork", "t1:omit:async", "t2:omit:async-string")

    claude_otel.delegation_fork(arms_from_ws(ws, "fork"), "fork")


def test_gate_delegation_fork_fails_when_the_agent_schema_declares_run_in_background_though_the_call_ran_async(  # noqa: E501
    tmp_path: Path,
) -> None:
    # With fork mode off, a call that leaves the field out also launches in the background
    # (measured on 2.1.287), so only the schema tells the modes apart.
    ws = tmp_path / "ws"
    agent_schema_request(ws, "fork", "with")
    delegation_sink(ws, "fork", "t1:omit:async")

    with pytest.raises(GateFailure) as exc:
        claude_otel.delegation_fork(arms_from_ws(ws, "fork"), "fork")
    output = failure_output(exc)
    assert "declares run_in_background, so fork mode was off" in output
    assert "No record written" in output


def test_gate_delegation_fork_fails_when_no_request_offers_the_agent_tool(tmp_path: Path) -> None:
    ws = tmp_path / "ws"
    delegation_sink(ws, "fork", "t1:omit:async")

    with pytest.raises(GateFailure) as exc:
        claude_otel.delegation_fork(arms_from_ws(ws, "fork"), "fork")
    assert "has no request offering the Agent tool" in failure_output(exc)


def test_gate_delegation_fork_fails_when_a_call_s_result_is_not_asynchronous(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    agent_schema_request(ws, "fork", "without")
    delegation_sink(ws, "fork", "t1:omit:sync")

    with pytest.raises(GateFailure) as exc:
        claude_otel.delegation_fork(arms_from_ws(ws, "fork"), "fork")
    assert "did not run in the background" in failure_output(exc)


def test_gate_delegation_fork_fails_when_a_call_was_never_answered(tmp_path: Path) -> None:
    ws = tmp_path / "ws"
    agent_schema_request(ws, "fork", "without")
    delegation_sink(ws, "fork", "t1:omit:async", "t2:omit:none")

    with pytest.raises(GateFailure) as exc:
        claude_otel.delegation_fork(arms_from_ws(ws, "fork"), "fork")
    assert "did not run in the background" in failure_output(exc)


def test_gate_delegation_fork_fails_when_the_arm_made_no_agent_call(tmp_path: Path) -> None:
    ws = tmp_path / "ws"
    agent_schema_request(ws, "fork", "without")
    (ws / "fork/sink/r.response.json").write_text(
        '{"type":"message","role":"assistant","content":[{"type":"text","text":"ok"}]}',
        encoding="utf-8",
    )

    with pytest.raises(GateFailure) as exc:
        claude_otel.delegation_fork(arms_from_ws(ws, "fork"), "fork")
    assert "made no Agent call" in failure_output(exc)


def test_a_response_that_is_not_json_refuses_before_any_gate_reads_the_arm(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    agent_schema_request(ws, "fork", "without")
    delegation_sink(ws, "fork", "t1:omit:async")
    (ws / "fork/sink/bad.response.json").write_text("not json", encoding="utf-8")

    with pytest.raises(GateFailure) as exc:
        arms_from_ws(ws, "fork")
    assert f"{ws}/fork/sink/bad.response.json is not valid JSON: " in failure_output(exc)


def test_gate_delegation_fork_fails_and_says_so_when_the_sink_holds_no_responses(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    agent_schema_request(ws, "fork", "without")

    with pytest.raises(GateFailure) as exc:
        claude_otel.delegation_fork(arms_from_ws(ws, "fork"), "fork")
    assert "captured 0 response and 1 request files" in failure_output(exc)


def test_gate_tool_answered_passes_when_the_named_tool_was_called_and_answered(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    tool_call_sink(ws, "a", "mcp__fx__alpha")

    claude_otel.tool_answered(arms_from_ws(ws, "a"), "a", "mcp__fx__alpha")


def test_gate_tool_answered_fails_when_the_call_has_no_tool_result(tmp_path: Path) -> None:
    ws = tmp_path / "ws"
    tool_call_sink(ws, "a", "mcp__fx__alpha", "unanswered")

    with pytest.raises(GateFailure) as exc:
        claude_otel.tool_answered(arms_from_ws(ws, "a"), "a", "mcp__fx__alpha")
    assert "no answered call to mcp__fx__alpha" in failure_output(exc)


def test_gate_tool_answered_fails_when_only_a_different_tool_was_called(tmp_path: Path) -> None:
    ws = tmp_path / "ws"
    tool_call_sink(ws, "a", "ToolSearch")

    with pytest.raises(GateFailure):
        claude_otel.tool_answered(arms_from_ws(ws, "a"), "a", "mcp__fx__alpha")


def test_gate_tool_answered_fails_when_the_name_appears_only_in_a_user_message_s_text(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    sink = sink_dir(ws, "a")
    (sink / "r.response.json").write_text(
        '{"type":"message","role":"assistant","content":[{"type":"text","text":"done"}]}',
        encoding="utf-8",
    )
    content = [{"type": "text", "text": "Call the mcp__fx__alpha tool"}]
    request = claude_request(system="x", messages=[{"role": "user", "content": content}])
    (sink / "q.request.json").write_text(compact(request), encoding="utf-8")

    with pytest.raises(GateFailure):
        claude_otel.tool_answered(arms_from_ws(ws, "a"), "a", "mcp__fx__alpha")


def test_gate_tool_answered_fails_and_says_so_when_the_sink_holds_no_responses(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    sink = sink_dir(ws, "a")
    (sink / "q.request.json").write_text(compact(claude_request(system="x")), encoding="utf-8")

    with pytest.raises(GateFailure) as exc:
        claude_otel.tool_answered(arms_from_ws(ws, "a"), "a", "mcp__fx__alpha")
    assert "captured 0 response and 1 request files" in failure_output(exc)


def test_gate_tool_answered_fails_when_the_only_tool_result_answers_a_different_call(
    tmp_path: Path,
) -> None:
    # The shape that matters in practice: the request answers the ToolSearch call, and the
    # call to the named tool goes unanswered.
    ws = tmp_path / "ws"
    sink = sink_dir(ws, "a")
    (sink / "r.response.json").write_text(
        '{"type":"message","role":"assistant","content":['
        '{"type":"tool_use","id":"s1","name":"ToolSearch","input":{}},'
        '{"type":"tool_use","id":"c1","name":"mcp__fx__alpha","input":{}}]}',
        encoding="utf-8",
    )
    content = [{"type": "tool_result", "tool_use_id": "s1", "content": "loaded"}]
    request = claude_request(system="x", messages=[{"role": "user", "content": content}])
    (sink / "q.request.json").write_text(compact(request), encoding="utf-8")

    with pytest.raises(GateFailure) as exc:
        claude_otel.tool_answered(arms_from_ws(ws, "a"), "a", "mcp__fx__alpha")
    assert "no answered call to mcp__fx__alpha" in failure_output(exc)


def test_gate_tool_answered_fails_and_says_so_when_the_sink_holds_no_requests(
    tmp_path: Path,
) -> None:
    ws = tmp_path / "ws"
    sink = sink_dir(ws, "a")
    (sink / "r.response.json").write_text(
        '{"type":"message","role":"assistant","content":'
        '[{"type":"tool_use","id":"c1","name":"mcp__fx__alpha","input":{}}]}',
        encoding="utf-8",
    )

    with pytest.raises(GateFailure) as exc:
        claude_otel.tool_answered(arms_from_ws(ws, "a"), "a", "mcp__fx__alpha")
    assert "captured 1 response and 0 request files" in failure_output(exc)
