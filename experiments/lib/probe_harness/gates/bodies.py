"""The request and response bodies the gates read, parsed once per arm into typed values.

The model states in one place what a body must look like, so every unexpected shape is one
loud refusal rather than a defence repeated in each gate. Its rules:

- `tools`, `output_config`, `input_schema` and `input_schema.properties` may be absent or
  null. `output_config.effort` may be absent, but a present one is a string.
- A missing or null request `system` or `messages`, or response `content`, counts as
  empty, so the gates decide and name the real cause. An API error response has no
  `content`; the gate that finds no call reports it.
- `system` and a message's `content` may be a string or an array.
- Every other field the model names must have the stated type when present and non-null.
  A present value of the wrong type raises `BodyShapeError`.

A shape error is an apparatus failure: the judge refuses, naming the arm, its sink, and the
body's position among the sink's bodies in file-name order, and never records.
"""

from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path
from typing import Any, NoReturn

from probe_harness.gates import GateFailure
from probe_harness.json_values import JsonValue
from probe_harness.views import SinkValues, read_sink

DEFERRED_LISTING = "The following deferred tools are now available via ToolSearch"


class MarkerField(StrEnum):
    """The request field a fixture's marker must appear in for the fixture to govern it.

    An agent file becomes the system prompt; a skill's body arrives in `messages[]`.
    """

    SYSTEM = "system"
    MESSAGES = "messages"


@dataclass(frozen=True)
class Tool:
    name: str
    # `input_schema.properties` keys; `None` without a schema.
    parameters: frozenset[str] | None


@dataclass(frozen=True)
class ToolResult:
    tool_use_id: str
    # The content as text: a string, or its text items joined.
    text: str


@dataclass(frozen=True)
class Message:
    role: str
    # A string or a list of blocks, kept whole for the marker search.
    content: JsonValue
    tool_results: tuple[ToolResult, ...]


@dataclass(frozen=True)
class ClaudeRequest:
    model: str | None
    # A string or a list of blocks.
    system: JsonValue
    messages: tuple[Message, ...]
    tools: tuple[Tool, ...]
    # `output_config.effort`; `None` when either is absent.
    effort: str | None
    # The whole body, for the deferred-listing search.
    body: JsonValue


@dataclass(frozen=True)
class ToolUse:
    id: str
    name: str
    input: Mapping[str, JsonValue]


@dataclass(frozen=True)
class ClaudeResponse:
    tool_uses: tuple[ToolUse, ...]


@dataclass(frozen=True)
class ClaudeArmBodies:
    """What one arm's sink holds, and where."""

    sink: Path
    requests: tuple[ClaudeRequest, ...]
    responses: tuple[ClaudeResponse, ...]


@dataclass(frozen=True)
class OpenCodeRequest:
    messages: tuple[Message, ...]


class BodyShapeError(ValueError):
    """A body that does not fit the model. `str(e)` names the body and the field."""


def parse_claude_request(value: JsonValue, where: str = "request") -> ClaudeRequest:
    body = _object(value, where, "the body")
    system = body.get("system")
    if system is None:
        system = []
    elif not isinstance(system, str | list):
        _misfit(where, "system", system, "a string or an array")
    output_config = _optional_object(body, "output_config", where)
    effort: str | None = None
    if output_config is not None and "effort" in output_config:
        # Absent means no effort was declared; a present one must be a level. Gate 3
        # compares levels, so a null one would silently leave the control set.
        effort = _required_str(output_config, "effort", where, "output_config.effort")
    return ClaudeRequest(
        model=_optional_str(body, "model", where, "model"),
        system=system,
        messages=_messages(body, where),
        tools=tuple(
            _tool(tool, where, f"tools[{i}]")
            for i, tool in enumerate(_optional_list(body, "tools", where, "tools"))
        ),
        effort=effort,
        body=body,
    )


def parse_claude_response(value: JsonValue, where: str = "response") -> ClaudeResponse:
    body = _object(value, where, "the body")
    tool_uses: list[ToolUse] = []
    for i, block in enumerate(_optional_list(body, "content", where, "content")):
        field = f"content[{i}]"
        block = _object(block, where, field)
        if block.get("type") != "tool_use":
            continue
        tool_input = block.get("input")
        if not isinstance(tool_input, dict):
            _misfit(where, f"{field}.input", tool_input, "an object")
        tool_uses.append(
            ToolUse(
                id=_required_str(block, "id", where, f"{field}.id"),
                name=_required_str(block, "name", where, f"{field}.name"),
                input=tool_input,
            )
        )
    return ClaudeResponse(tuple(tool_uses))


def parse_opencode_request(value: JsonValue, where: str = "request") -> OpenCodeRequest:
    return OpenCodeRequest(_messages(_object(value, where, "the body"), where))


def claude_arm(values: SinkValues, sink: Path) -> ClaudeArmBodies:
    return ClaudeArmBodies(
        sink=sink,
        requests=tuple(
            parse_claude_request(v, f"{sink}: request {n}")
            for n, v in enumerate(values.requests, start=1)
        ),
        responses=tuple(
            parse_claude_response(v, f"{sink}: response {n}")
            for n, v in enumerate(values.responses, start=1)
        ),
    )


def opencode_requests(values: SinkValues, sink: Path) -> tuple[OpenCodeRequest, ...]:
    return tuple(
        parse_opencode_request(v, f"{sink}: request {n}")
        for n, v in enumerate(values.requests, start=1)
    )


def claude_arms(sinks: Mapping[str, SinkValues], ws: Path) -> dict[str, ClaudeArmBodies]:
    """Each arm's sink values as typed bodies, refusing the run on a body that does not fit."""
    arms: dict[str, ClaudeArmBodies] = {}
    for name, values in sinks.items():
        try:
            arms[name] = claude_arm(values, ws / name / "sink")
        except BodyShapeError as e:
            raise shape_refusal(name, e) from None
    return arms


def opencode_arms(
    sinks: Mapping[str, SinkValues], ws: Path
) -> dict[str, tuple[OpenCodeRequest, ...]]:
    """Each arm's sink values as typed requests, refusing the run on a body that does not fit."""
    arms: dict[str, tuple[OpenCodeRequest, ...]] = {}
    for name, values in sinks.items():
        try:
            arms[name] = opencode_requests(values, ws / name / "sink")
        except BodyShapeError as e:
            raise shape_refusal(name, e) from None
    return arms


def read_claude_arms(ws: Path, arm_names: Sequence[str]) -> dict[str, ClaudeArmBodies]:
    """Read and parse each `<ws>/<arm>/sink`, for a judge or a probe function."""
    return claude_arms({name: read_sink(ws / name / "sink") for name in arm_names}, ws)


def shape_refusal(arm: str, error: BodyShapeError) -> GateFailure:
    return GateFailure(
        f"arm {arm}: {error}",
        "the gates cannot read this body, so the run describes nothing. No record written.",
    )


def governed(request: ClaudeRequest, field: MarkerField, marker: str) -> bool:
    """Whether <marker> appears in some string value inside the request's <field>."""
    match field:
        case MarkerField.SYSTEM:
            return contains(request.system, marker)
        case MarkerField.MESSAGES:
            return any(contains(message.content, marker) for message in request.messages)


def contains(value: JsonValue, text: str) -> bool:
    """Whether <text> appears in some string value inside <value>, keys excluded."""
    return any(text in s for s in _strings(value))


def deferred_tools(request: ClaudeRequest) -> list[str]:
    """The lines of every string in the body that carries the deferred-tools listing."""
    return [line for s in _strings(request.body) if DEFERRED_LISTING in s for line in s.split("\n")]


def listed_tools(request: ClaudeRequest) -> list[str]:
    """The request's direct tool names, then its deferred-listing lines."""
    return [tool.name for tool in request.tools] + deferred_tools(request)


def _strings(value: JsonValue) -> Iterator[str]:
    """Every string value inside <value>, in document order.

    Walked with an explicit stack, so a body nested past Python's recursion limit is
    searched like any other.
    """
    stack: list[JsonValue] = [value]
    while stack:
        match stack.pop():
            case str() as text:
                yield text
            case list() as items:
                stack.extend(reversed(items))
            case dict() as fields:
                stack.extend(reversed(list(fields.values())))
            case _:
                pass


def _messages(body: dict[str, Any], where: str) -> tuple[Message, ...]:
    return tuple(
        _message(message, where, f"messages[{i}]")
        for i, message in enumerate(_optional_list(body, "messages", where, "messages"))
    )


def _message(value: JsonValue, where: str, field: str) -> Message:
    message = _object(value, where, field)
    role = _required_str(message, "role", where, f"{field}.role")
    content = message.get("content")
    if content is not None and not isinstance(content, str | list):
        _misfit(where, f"{field}.content", content, "a string or an array")
    results: list[ToolResult] = []
    if role == "user" and isinstance(content, list):
        for i, block in enumerate(content):
            if isinstance(block, dict) and block.get("type") == "tool_result":
                results.append(_tool_result(block, where, f"{field}.content[{i}]"))
    return Message(role, content, tuple(results))


def _tool_result(block: dict[str, Any], where: str, field: str) -> ToolResult:
    content = block.get("content")
    parts: list[str] = []
    if isinstance(content, str):
        parts.append(content)
    elif isinstance(content, list):
        for i, item in enumerate(content):
            if isinstance(item, str):
                parts.append(item)
            elif isinstance(item, dict):
                parts.append(_optional_str(item, "text", where, f"{field}.content[{i}].text") or "")
            else:
                _misfit(where, f"{field}.content[{i}]", item, "a string or an object")
    elif content is not None:
        _misfit(where, f"{field}.content", content, "a string or an array")
    return ToolResult(
        _required_str(block, "tool_use_id", where, f"{field}.tool_use_id"), "".join(parts)
    )


def _tool(value: JsonValue, where: str, field: str) -> Tool:
    tool = _object(value, where, field)
    schema = _optional_object(tool, "input_schema", where, f"{field}.input_schema")
    parameters: frozenset[str] | None = None
    if schema is not None:
        properties = _optional_object(
            schema, "properties", where, f"{field}.input_schema.properties"
        )
        parameters = frozenset(properties or {})
    return Tool(_required_str(tool, "name", where, f"{field}.name"), parameters)


def _object(value: JsonValue, where: str, field: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        _misfit(where, field, value, "an object")
    return value


def _optional_object(
    container: dict[str, Any], key: str, where: str, field: str | None = None
) -> dict[str, Any] | None:
    value = container.get(key)
    if value is None:
        return None
    return _object(value, where, field or key)


def _optional_list(container: dict[str, Any], key: str, where: str, field: str) -> list[Any]:
    value = container.get(key)
    if value is None:
        return []
    if not isinstance(value, list):
        _misfit(where, field, value, "an array")
    return value


def _optional_str(container: dict[str, Any], key: str, where: str, field: str) -> str | None:
    value = container.get(key)
    if value is not None and not isinstance(value, str):
        _misfit(where, field, value, "a string")
    return value


def _required_str(container: dict[str, Any], key: str, where: str, field: str) -> str:
    value = container.get(key)
    if not isinstance(value, str):
        _misfit(where, field, value, "a string")
    return value


def _misfit(where: str, field: str, value: JsonValue, expected: str) -> NoReturn:
    raise BodyShapeError(f"{where}: {field} is {_type_name(value)}, expected {expected}")


def _type_name(value: JsonValue) -> str:
    match value:
        case None:
            return "null"
        case bool():
            return "a boolean"
        case int() | float():
            return "a number"
        case str():
            return "a string"
        case list():
            return "an array"
        case _:
            return "an object"
