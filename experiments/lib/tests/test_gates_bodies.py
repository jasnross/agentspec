"""The provider models in `probe_harness.models`, as `gates/bodies.py` validates sinks into them."""

import json
from pathlib import Path
from typing import Any

import pytest
from probe_harness.families import claude_otel, opencode_fake
from probe_harness.families.claude_otel import ClaudeArm, ClaudeOtelProbe
from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe
from probe_harness.gates import GateFailure
from probe_harness.gates.bodies import (
    MarkerField,
    claude_arms,
    governed,
    listed_tools,
    opencode_arms,
)
from probe_harness.models.claude_request import ClaudeRequest, ToolResultBlock
from probe_harness.models.claude_response import AgentCall, ClaudeResponse
from probe_harness.views import SinkEntry, SinkValues
from pydantic import MISSING, ValidationError

REAL_REQUEST: dict[str, Any] = {
    "model": "claude-sonnet-5-5",
    "system": [{"type": "text", "text": "You are AGENTSPEC-PROBE-MARKER-X."}],
    "messages": [
        {"role": "user", "content": "delegate"},
        {
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "t1",
                    "content": [{"type": "text", "text": "Async agent launched"}],
                },
                {"type": "text", "text": "more"},
            ],
        },
    ],
    "tools": [
        {"name": "Read", "input_schema": {"type": "object", "properties": {"path": {}}}},
        {"name": "web_search", "type": "web_search_20250305"},
    ],
    "output_config": {"effort": "low"},
}

SINK_FILE = Path("a") / "sink" / "one.request.json"


def _request(**overrides: Any) -> ClaudeRequest:
    return ClaudeRequest.model_validate({**REAL_REQUEST, **overrides})


def _refusal(body: Any) -> str:
    """The first line of the refusal `claude_arms` raises for a request <body>."""
    values = SinkValues(requests=[SinkEntry(SINK_FILE, body)], responses=[])
    with pytest.raises(GateFailure) as exc:
        claude_arms({"a": values}, Path())
    assert exc.value.lines[1] == (
        "the gates cannot read this body, so the run describes nothing. No record written."
    )
    return exc.value.lines[0]


def test_a_real_shaped_request_validates() -> None:
    request = _request()
    assert request.model == "claude-sonnet-5-5"
    assert request.output_config.effort == "low"
    assert [t.name for t in request.tool_list] == ["Read", "web_search"]
    read, server = request.tool_list
    assert read.input_schema is not MISSING
    assert read.input_schema.properties.keys() == frozenset({"path"})
    assert server.input_schema is MISSING
    content = request.messages[1].content
    assert not isinstance(content, str)
    result = content[0]
    assert isinstance(result, ToolResultBlock)
    assert (result.tool_use_id, result.text) == ("t1", "Async agent launched")


def test_a_real_shaped_response_validates_its_tool_uses() -> None:
    response = ClaudeResponse.model_validate(
        {
            "content": [
                {"type": "thinking", "thinking": "…"},
                {"type": "text", "text": "ok"},
                {"type": "tool_use", "id": "t1", "name": "Agent", "input": {"x": 1}},
            ]
        }
    )
    assert response.content is not MISSING
    uses = [b for b in response.content if isinstance(b, AgentCall)]
    assert [(u.id, u.name, u.input.run_in_background) for u in uses] == [("t1", "Agent", MISSING)]


@pytest.mark.parametrize("field", ["model", "output_config", "messages", "system"])
def test_a_missing_required_field_is_refused_naming_the_file_and_path(field: str) -> None:
    body = {k: v for k, v in REAL_REQUEST.items() if k != field}
    assert _refusal(body) == f"arm a: {SINK_FILE}: {field}: Field required"


def test_a_misspelled_required_field_is_refused_rather_than_read_as_absent() -> None:
    body = {k: v for k, v in REAL_REQUEST.items() if k != "model"}
    body["modle"] = "claude-sonnet-5-5"
    assert _refusal(body) == f"arm a: {SINK_FILE}: model: Field required"


@pytest.mark.parametrize(
    ("override", "path"),
    [
        ({"messages": [{"role": "user", "content": 5}]}, "messages.0.content"),
        ({"tools": [{"name": 5}]}, "tools.0.name"),
        ({"output_config": {"effort": None}}, "output_config.effort"),
        (
            {"messages": [{"role": "user", "content": [{"type": "tool_result", "content": "x"}]}]},
            "messages.0.content.0.tool_use_id",
        ),
        (
            {
                "messages": [
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "tool_result",
                                "tool_use_id": "t",
                                "content": [{"type": "text"}],
                            }
                        ],
                    }
                ]
            },
            "messages.0.content.0.content.0.text",
        ),
    ],
)
def test_a_nested_field_is_refused_by_the_path_the_body_spells(
    override: dict[str, Any], path: str
) -> None:
    assert _refusal({**REAL_REQUEST, **override}).startswith(f"arm a: {SINK_FILE}: {path}: ")


def _response_refusal(body: Any) -> str:
    """The first line of the refusal `claude_arms` raises for a response <body>."""
    response_file = Path("a") / "sink" / "one.response.json"
    values = SinkValues(requests=[], responses=[SinkEntry(response_file, body)])
    with pytest.raises(GateFailure) as exc:
        claude_arms({"a": values}, Path())
    prefix = f"arm a: {response_file}: "
    assert exc.value.lines[0].startswith(prefix)
    return exc.value.lines[0].removeprefix(prefix)


@pytest.mark.parametrize(
    ("body", "problem"),
    [
        ({"content": "text"}, "content: Input should be a valid tuple"),
        ({"content": None}, "content: Input should be a valid tuple"),
        ({"content": ["text"]}, "content.0: Input should be a valid dictionary"),
        (
            {"content": [{"type": "tool_use", "id": "t", "name": "Agent", "input": "x"}]},
            "content.0.input: Input should be a valid dictionary",
        ),
        (
            {"content": [{"type": "tool_use", "name": "Agent", "input": {}}]},
            "content.0.id: Field required",
        ),
        (
            {
                "content": [
                    {
                        "type": "tool_use",
                        "id": "t",
                        "name": "Agent",
                        "input": {"run_in_background": "true"},
                    }
                ]
            },
            "content.0.input.run_in_background: Input should be a valid boolean",
        ),
    ],
)
def test_a_response_field_of_the_wrong_shape_is_refused_by_its_path(
    body: dict[str, Any], problem: str
) -> None:
    assert _response_refusal(body).startswith(problem)


def test_another_tool_s_malformed_input_does_not_refuse_the_run() -> None:
    # Claude Code's Bash tool also takes `run_in_background`; no gate reads it there.
    bash = {"type": "tool_use", "id": "t", "name": "Bash", "input": {"run_in_background": "true"}}
    response = ClaudeResponse.model_validate({"content": [bash]})
    assert response.content is not MISSING
    assert not any(isinstance(block, AgentCall) for block in response.content)


def test_a_refusal_names_the_value_s_position_in_a_file_holding_several() -> None:
    entries = [
        SinkEntry(SINK_FILE, REAL_REQUEST, position=1),
        SinkEntry(SINK_FILE, {"model": "m"}, position=2),
    ]
    with pytest.raises(GateFailure) as exc:
        claude_arms({"a": SinkValues(requests=entries, responses=[])}, Path())
    assert exc.value.lines[0] == f"arm a: {SINK_FILE} value 2: system: Field required"


def test_a_body_that_is_not_an_object_is_refused_as_such() -> None:
    assert _refusal("hi") == f"arm a: {SINK_FILE}: the body is not a JSON object"


def test_null_messages_are_refused_rather_than_read_as_empty() -> None:
    assert _refusal({**REAL_REQUEST, "messages": None}).startswith(
        f"arm a: {SINK_FILE}: messages: "
    )


def test_a_tool_result_without_content_is_refused() -> None:
    message = {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t"}]}
    assert _refusal({**REAL_REQUEST, "messages": [message]}) == (
        f"arm a: {SINK_FILE}: messages.0.content.0.content: Field required"
    )


def test_a_null_system_validates_and_carries_no_marker() -> None:
    assert not governed(_request(system=None), MarkerField.SYSTEM, "AGENTSPEC-PROBE-MARKER-X")


def test_an_output_config_without_effort_validates_with_effort_missing() -> None:
    assert _request(output_config={}).output_config.effort is MISSING


def test_a_string_system_is_accepted() -> None:
    assert governed(
        _request(system="x AGENTSPEC-PROBE-MARKER-X y"),
        MarkerField.SYSTEM,
        "AGENTSPEC-PROBE-MARKER-X",
    )


def test_a_request_without_tools_offers_none() -> None:
    body = {k: v for k, v in REAL_REQUEST.items() if k != "tools"}
    assert ClaudeRequest.model_validate(body).tool_list == ()


def test_a_tool_without_an_input_schema_validates() -> None:
    assert _request(tools=[{"name": "ToolSearch"}]).tool_list[0].input_schema is MISSING


def test_a_response_with_no_content_validates_as_missing() -> None:
    assert ClaudeResponse.model_validate({"type": "error"}).content is MISSING


def test_a_string_run_in_background_is_refused() -> None:
    with pytest.raises(ValidationError):
        ClaudeResponse.model_validate(
            {
                "content": [
                    {
                        "type": "tool_use",
                        "id": "t",
                        "name": "Agent",
                        "input": {"run_in_background": "true"},
                    }
                ]
            }
        )


def test_governed_finds_the_marker_in_an_array_system() -> None:
    request = _request()
    assert governed(request, MarkerField.SYSTEM, "AGENTSPEC-PROBE-MARKER-X")
    assert not governed(request, MarkerField.MESSAGES, "AGENTSPEC-PROBE-MARKER-X")


def test_governed_does_not_find_the_marker_in_a_key() -> None:
    request = _request(system=[{"AGENTSPEC-PROBE-MARKER-X": "text"}])
    assert not governed(request, MarkerField.SYSTEM, "AGENTSPEC-PROBE-MARKER-X")


def test_governed_searches_a_message_s_content_only() -> None:
    message = {"role": "user", "content": "plain", "note": "AGENTSPEC-PROBE-MARKER-X"}
    request = _request(messages=[message])
    assert not governed(request, MarkerField.MESSAGES, "AGENTSPEC-PROBE-MARKER-X")


def test_governed_finds_a_skill_marker_in_any_message() -> None:
    request = _request(
        messages=[
            {"role": "user", "content": "plain"},
            {"role": "user", "content": [{"type": "text", "text": "AGENTSPEC-PROBE-MARKER-X"}]},
        ]
    )
    assert governed(request, MarkerField.MESSAGES, "AGENTSPEC-PROBE-MARKER-X")


def test_listed_tools_names_direct_tools_then_the_deferred_listing() -> None:
    listing = "The following deferred tools are now available via ToolSearch:\nmcp__fx__alpha"
    request = _request(
        tools=[{"name": "Read"}],
        messages=[{"role": "user", "content": [{"type": "text", "text": listing}]}],
    )
    assert listed_tools(request) == [
        "Read",
        "The following deferred tools are now available via ToolSearch:",
        "mcp__fx__alpha",
    ]


def test_governed_searches_a_system_nested_past_the_recursion_limit() -> None:
    nested: Any = "AGENTSPEC-PROBE-MARKER-X"
    for _ in range(5000):
        nested = [nested]
    assert governed(_request(system=nested), MarkerField.SYSTEM, "AGENTSPEC-PROBE-MARKER-X")


def test_an_opencode_message_may_carry_null_content() -> None:
    values = SinkValues(
        requests=[SinkEntry(SINK_FILE, {"messages": [{"role": "assistant", "content": None}]})],
        responses=[],
    )
    (request,) = opencode_arms({"a": values}, Path())["a"]
    assert list(request.messages[0].content.strings()) == []


def test_the_claude_judge_refuses_a_body_that_does_not_fit(tmp_path: Path) -> None:
    sink = tmp_path / "a" / "sink"
    sink.mkdir(parents=True)
    body = {k: v for k, v in REAL_REQUEST.items() if k != "model"}
    (sink / "one.request.json").write_text(json.dumps(body) + "\n", encoding="utf-8")
    probe = ClaudeOtelProbe(
        model="m", budget_usd="0.10", marker="M", arms=(ClaudeArm("a", "tree", "p"),)
    )

    with pytest.raises(GateFailure) as exc:
        claude_otel.judge(tmp_path, probe)
    assert exc.value.lines == (
        f"arm a: {sink / 'one.request.json'}: model: Field required",
        "the gates cannot read this body, so the run describes nothing. No record written.",
    )


def test_the_opencode_judge_refuses_a_body_that_does_not_fit(tmp_path: Path) -> None:
    sink = tmp_path / "a" / "sink"
    sink.mkdir(parents=True)
    (sink / "one.request.json").write_text('{"messages": "hi"}\n', encoding="utf-8")
    probe = OpenCodeFakeProbe(marker="M", arms=(OpenCodeArm("a", "agent"),))

    with pytest.raises(GateFailure) as exc:
        opencode_fake.judge(tmp_path, probe)
    assert exc.value.lines[0] == (
        f"arm a: {sink / 'one.request.json'}: messages: Input should be a valid tuple"
    )
