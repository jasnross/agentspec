"""The typed request and response model in `gates/bodies.py`."""

from pathlib import Path
from typing import Any

import pytest
from probe_harness.families import claude_otel, opencode_fake
from probe_harness.families.claude_otel import ClaudeArm, ClaudeOtelProbe
from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe
from probe_harness.gates import GateFailure
from probe_harness.gates.bodies import (
    BodyShapeError,
    MarkerField,
    Tool,
    ToolResult,
    governed,
    listed_tools,
    parse_claude_request,
    parse_claude_response,
    parse_opencode_request,
)

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
        {"name": "ToolSearch", "input_schema": None},
    ],
    "output_config": {"effort": "low"},
}


def test_a_real_shaped_request_parses() -> None:
    request = parse_claude_request(REAL_REQUEST)
    assert request.model == "claude-sonnet-5-5"
    assert request.effort == "low"
    assert request.tools == (Tool("Read", frozenset({"path"})), Tool("ToolSearch", None))
    assert request.messages[1].tool_results == (ToolResult("t1", "Async agent launched"),)


def test_a_real_shaped_response_parses_its_tool_uses() -> None:
    response = parse_claude_response(
        {
            "content": [
                {"type": "thinking", "thinking": "…"},
                {"type": "text", "text": "ok"},
                {"type": "tool_use", "id": "t1", "name": "Agent", "input": {"x": 1}},
            ]
        }
    )
    assert [(u.id, u.name, dict(u.input)) for u in response.tool_uses] == [
        ("t1", "Agent", {"x": 1})
    ]


@pytest.mark.parametrize("field", ["system", "messages"])
@pytest.mark.parametrize("value", [None, "absent"])
def test_a_missing_or_null_system_or_messages_parses_as_empty(field: str, value: Any) -> None:
    body = {k: v for k, v in REAL_REQUEST.items() if k != field}
    if value is None:
        body[field] = None
    request = parse_claude_request(body)
    assert (request.system if field == "system" else request.messages) in ([], ())


@pytest.mark.parametrize("content", [None, "absent"])
def test_a_missing_or_null_response_content_parses_as_no_call(content: Any) -> None:
    body: dict[str, Any] = {"type": "error"}
    if content is None:
        body["content"] = None
    assert parse_claude_response(body).tool_uses == ()


@pytest.mark.parametrize(
    ("override", "field"),
    [
        ({"system": 5}, "system is a number, expected a string or an array"),
        ({"messages": {"role": "user"}}, "messages is an object, expected an array"),
        ({"messages": ["hi"]}, "messages[0] is a string, expected an object"),
        ({"messages": [{"content": "hi"}]}, "messages[0].role is null, expected a string"),
        (
            {"messages": [{"role": "user", "content": 5}]},
            "messages[0].content is a number, expected a string or an array",
        ),
        ({"tools": {"name": "Read"}}, "tools is an object, expected an array"),
        ({"tools": [{"name": 5}]}, "tools[0].name is a number, expected a string"),
        (
            {"tools": [{"name": "Read", "input_schema": []}]},
            "tools[0].input_schema is an array, expected an object",
        ),
        ({"output_config": "low"}, "output_config is a string, expected an object"),
        ({"output_config": {"effort": 1}}, "output_config.effort is a number, expected a string"),
        ({"output_config": {"effort": None}}, "output_config.effort is null, expected a string"),
        ({"model": True}, "model is a boolean, expected a string"),
        (
            {"messages": [{"role": "user", "content": [{"type": "tool_result", "content": "x"}]}]},
            "messages[0].content[0].tool_use_id is null, expected a string",
        ),
        (
            {
                "messages": [
                    {
                        "role": "user",
                        "content": [{"type": "tool_result", "tool_use_id": "t", "content": 5}],
                    }
                ]
            },
            "messages[0].content[0].content is a number, expected a string or an array",
        ),
    ],
)
def test_a_request_field_of_the_wrong_type_is_refused_by_name(
    override: dict[str, Any], field: str
) -> None:
    with pytest.raises(BodyShapeError) as exc:
        parse_claude_request({**REAL_REQUEST, **override}, "sink: request 3")
    assert str(exc.value) == f"sink: request 3: {field}"


@pytest.mark.parametrize(
    ("body", "field"),
    [
        ({"content": "text"}, "content is a string, expected an array"),
        ({"content": ["text"]}, "content[0] is a string, expected an object"),
        (
            {"content": [{"type": "tool_use", "id": "t", "name": "Agent", "input": "x"}]},
            "content[0].input is a string, expected an object",
        ),
        (
            {"content": [{"type": "tool_use", "name": "Agent", "input": {}}]},
            "content[0].id is null, expected a string",
        ),
    ],
)
def test_a_response_field_of_the_wrong_type_is_refused_by_name(
    body: dict[str, Any], field: str
) -> None:
    with pytest.raises(BodyShapeError) as exc:
        parse_claude_response(body, "sink: response 1")
    assert str(exc.value) == f"sink: response 1: {field}"


def test_an_opencode_message_may_carry_null_content() -> None:
    request = parse_opencode_request({"messages": [{"role": "assistant", "content": None}]})
    assert request.messages[0].content is None


def test_governed_finds_the_marker_in_a_string_system() -> None:
    request = parse_claude_request({"system": "x AGENTSPEC-PROBE-MARKER-X y"})
    assert governed(request, MarkerField.SYSTEM, "AGENTSPEC-PROBE-MARKER-X")


def test_governed_finds_the_marker_in_an_array_system() -> None:
    request = parse_claude_request(REAL_REQUEST)
    assert governed(request, MarkerField.SYSTEM, "AGENTSPEC-PROBE-MARKER-X")
    assert not governed(request, MarkerField.MESSAGES, "AGENTSPEC-PROBE-MARKER-X")


def test_governed_does_not_find_the_marker_in_a_key() -> None:
    request = parse_claude_request({"system": [{"AGENTSPEC-PROBE-MARKER-X": "text"}]})
    assert not governed(request, MarkerField.SYSTEM, "AGENTSPEC-PROBE-MARKER-X")


def test_governed_finds_a_skill_marker_in_any_message() -> None:
    request = parse_claude_request(
        {
            "messages": [
                {"role": "user", "content": "plain"},
                {"role": "user", "content": [{"type": "text", "text": "AGENTSPEC-PROBE-MARKER-X"}]},
            ]
        }
    )
    assert governed(request, MarkerField.MESSAGES, "AGENTSPEC-PROBE-MARKER-X")


def test_listed_tools_names_direct_tools_then_the_deferred_listing() -> None:
    listing = "The following deferred tools are now available via ToolSearch:\nmcp__fx__alpha"
    request = parse_claude_request(
        {
            "tools": [{"name": "Read"}],
            "messages": [{"role": "user", "content": [{"type": "text", "text": listing}]}],
        }
    )
    assert listed_tools(request) == [
        "Read",
        "The following deferred tools are now available via ToolSearch:",
        "mcp__fx__alpha",
    ]


def test_governed_searches_a_body_nested_past_the_recursion_limit() -> None:
    nested: Any = "AGENTSPEC-PROBE-MARKER-X"
    for _ in range(5000):
        nested = [nested]
    request = parse_claude_request({"system": nested})
    assert governed(request, MarkerField.SYSTEM, "AGENTSPEC-PROBE-MARKER-X")


def test_the_claude_judge_refuses_a_body_that_does_not_fit(tmp_path: Path) -> None:
    sink = tmp_path / "a" / "sink"
    sink.mkdir(parents=True)
    (sink / "one.request.json").write_text('{"system": 5}\n', encoding="utf-8")
    probe = ClaudeOtelProbe(
        model="m", budget_usd="0.10", marker="M", arms=(ClaudeArm("a", "tree", "p"),)
    )

    with pytest.raises(GateFailure) as exc:
        claude_otel.judge(tmp_path, probe)
    assert exc.value.lines == (
        f"probe: arm a: {sink}: request 1: system is a number, expected a string or an array",
        "probe: the gates cannot read this body, so the run describes nothing. No record written.",
    )


def test_the_opencode_judge_refuses_a_body_that_does_not_fit(tmp_path: Path) -> None:
    sink = tmp_path / "a" / "sink"
    sink.mkdir(parents=True)
    (sink / "one.request.json").write_text('{"messages": "hi"}\n', encoding="utf-8")
    probe = OpenCodeFakeProbe(marker="M", arms=(OpenCodeArm("a", "agent"),))

    with pytest.raises(GateFailure) as exc:
        opencode_fake.judge(tmp_path, probe)
    assert exc.value.lines[0] == (
        f"probe: arm a: {sink}: request 1: messages is a string, expected an array"
    )
