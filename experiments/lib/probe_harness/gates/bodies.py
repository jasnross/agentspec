"""The request and response bodies the gates read, validated once per arm.

Each body is validated against its provider model in `probe_harness.models`, which states
in one place what a body must look like, so every unexpected shape is one loud refusal
rather than a defence repeated in each gate. A body that fails its model is an apparatus
failure: the judge refuses, naming the arm, the body's file, and the field's path, and
never records.

A gate reads a model's declared fields, and searches a body's source JSON only through
`RawJson`.
"""

from collections.abc import Callable, Iterator, Mapping, Sequence
from enum import StrEnum
from pathlib import Path
from typing import Never

from pydantic import BaseModel, ValidationError

from probe_harness.arms import ArmCapture, Arms
from probe_harness.assertions import unique
from probe_harness.gates import GateFailure
from probe_harness.json_values import JsonValue
from probe_harness.models.claude_request import ClaudeMessage, ClaudeRequest
from probe_harness.models.claude_response import ClaudeResponse
from probe_harness.models.opencode_request import OpenCodeRequest
from probe_harness.models.raw import field_path
from probe_harness.views import SinkEntry, SinkValues, read_sink

DEFERRED_LISTING = "The following deferred tools are now available via ToolSearch"
READY_LISTING = "The following tools just became available and are ready to use"
SUBAGENT_FLAG = "cc_is_subagent=true"


class MarkerField(StrEnum):
    """The request field a fixture's marker must appear in for the fixture to govern it.

    An agent file becomes the system prompt; a skill's body arrives in `messages[]`.
    """

    SYSTEM = "system"
    MESSAGES = "messages"


type ClaudeArms = Arms[ClaudeRequest, ClaudeResponse]
type OpenCodeArms = Arms[OpenCodeRequest, Never]


def claude_arms(
    sinks: Mapping[str, SinkValues],
) -> dict[str, ArmCapture[ClaudeRequest, ClaudeResponse]]:
    """Each arm's sink values as models, refusing the run on a body that does not fit."""
    return {
        name: ArmCapture(
            requests=_validated(ClaudeRequest, _labelled(values.requests), name),
            responses=_validated(ClaudeResponse, _labelled(values.responses), name),
        )
        for name, values in sinks.items()
    }


def opencode_arms(sinks: Mapping[str, SinkValues]) -> dict[str, ArmCapture[OpenCodeRequest, Never]]:
    """Each arm's sink values as models, refusing the run on a body that does not fit."""
    return {
        name: ArmCapture(
            requests=_validated(OpenCodeRequest, _labelled(values.requests), name), responses=()
        )
        for name, values in sinks.items()
    }


def read_claude_arms(
    ws: Path, arm_names: Sequence[str]
) -> dict[str, ArmCapture[ClaudeRequest, ClaudeResponse]]:
    """Read and validate each `<ws>/<arm>/sink`."""
    return claude_arms({name: read_sink(ws / name / "sink") for name in arm_names})


def claude_sample_arms(
    document: JsonValue, label: str, arm_names: Sequence[str]
) -> dict[str, ArmCapture[ClaudeRequest, ClaudeResponse]]:
    """A sample's `{"<arm>": {"requests": [...], "responses": [...]}}` as models."""
    arms = _sample_arms(document, label, arm_names, ("requests", "responses"))
    return {
        name: ArmCapture(
            requests=_validated(ClaudeRequest, _elements(label, name, "requests", arm), name),
            responses=_validated(ClaudeResponse, _elements(label, name, "responses", arm), name),
        )
        for name, arm in arms.items()
    }


def opencode_sample_arms(
    document: JsonValue, label: str, arm_names: Sequence[str]
) -> dict[str, ArmCapture[OpenCodeRequest, Never]]:
    """A sample's `{"<arm>": {"requests": [...]}}` as models."""
    arms = _sample_arms(document, label, arm_names, ("requests",))
    return {
        name: ArmCapture(
            requests=_validated(OpenCodeRequest, _elements(label, name, "requests", arm), name),
            responses=(),
        )
        for name, arm in arms.items()
    }


def governed(request: ClaudeRequest, field: MarkerField, marker: str) -> bool:
    """Whether <marker> appears in some string value inside the request's <field>."""
    match field:
        case MarkerField.SYSTEM:
            return any(marker in s for s in request.system.strings())
        case MarkerField.MESSAGES:
            return any(marker in s for m in request.messages for s in _content_strings(m))


def governed_requests(
    requests: Sequence[ClaudeRequest], marker: str, field: MarkerField = MarkerField.SYSTEM
) -> list[ClaudeRequest]:
    """The requests whose <field> carries <marker>, in capture order."""
    return [r for r in requests if governed(r, field, marker)]


def subagent(request: ClaudeRequest) -> bool:
    """Whether some `.system` string carries the subagent flag Claude writes in its billing header.

    A subagent's follow-up turn after a tool call carries no agent system prompt, so no
    marker, yet runs at the subagent's effort; this flag is what still identifies it. It is
    matched anywhere in `.system`, as the effort assertions' control set matches it.
    """
    return any(SUBAGENT_FLAG in s for s in request.system.strings())


def deferred_tools(request: ClaudeRequest) -> list[str]:
    """The lines of each deferred-tools listing in the body, one tool name per line.

    A listing runs from the line after its header to the first blank line. The string that
    carries it also carries other request text, such as the environment section and the
    Agent tool's list of agent types, whose lines name tools without offering them.
    """
    return [line for s in request.raw.strings() for line in _listing(s, DEFERRED_LISTING)]


def ready_tools(request: ClaudeRequest) -> list[str]:
    """The lines of each listing of tools announced ready to use, one tool name per line.

    Claude Code announces a tool loaded mid-session this way, in the block that carries the
    deferred listing; the request offers it without listing it in `tools[]` or as deferred.
    """
    return [line for s in request.raw.strings() for line in _listing(s, READY_LISTING)]


def listed_tools(request: ClaudeRequest) -> list[str]:
    """The request's direct tool names, then its ready-to-use and deferred-listing lines."""
    return (
        [tool.name for tool in request.tool_list] + ready_tools(request) + deferred_tools(request)
    )


def offered(requests: Sequence[ClaudeRequest], keep: Callable[[str], bool]) -> list[JsonValue]:
    """The distinct tool names the requests offer, directly or deferred, kept by <keep>.

    In jq's `unique` order, the order committed records were written in.
    """
    return unique([name for r in requests for name in listed_tools(r) if keep(name)])


def _content_strings(message: ClaudeMessage) -> Iterator[str]:
    """Every string leaf of a message's content; the message's other fields are not searched."""
    if isinstance(message.content, str):
        yield message.content
        return
    for block in message.content:
        yield from block.raw.strings()


def _listing(text: str, header: str) -> list[str]:
    """The lines of each listing under <header> in <text>, up to its first blank line."""
    lines = text.split("\n")
    starts = [i + 1 for i, line in enumerate(lines) if header in line]
    listed: list[str] = []
    for start in starts:
        for line in lines[start:]:
            if not line:
                break
            listed.append(line)
    return listed


def _labelled(entries: Sequence[SinkEntry]) -> list[tuple[str, JsonValue]]:
    return [(entry.label, entry.value) for entry in entries]


def _sample_arms(
    document: JsonValue, label: str, arm_names: Sequence[str], keys: tuple[str, ...]
) -> dict[str, dict[str, JsonValue]]:
    """A sample document's arms, refusing unless it holds exactly the definition's arms."""
    if not isinstance(document, dict) or list(document) != list(arm_names):
        found = list(document) if isinstance(document, dict) else type(document).__name__
        raise GateFailure(
            f"{label}: input must be an object holding the arms {', '.join(arm_names)} in "
            f"order, but holds {found}"
        )
    arms: dict[str, dict[str, JsonValue]] = {}
    for name, arm in document.items():
        if not isinstance(arm, dict) or sorted(arm) != sorted(keys):
            raise GateFailure(f"{label}: {name} must be an object holding {', '.join(keys)}")
        arms[name] = arm
    return arms


def _elements(
    label: str, arm: str, key: str, document: Mapping[str, JsonValue]
) -> list[tuple[str, JsonValue]]:
    values = document[key]
    if not isinstance(values, list):
        raise GateFailure(f"{label}: {arm}.{key} must be a list")
    return [(f"{label} {arm}.{key}[{i}]", value) for i, value in enumerate(values)]


def _validated[M: BaseModel](
    model: type[M], entries: Sequence[tuple[str, JsonValue]], arm: str
) -> tuple[M, ...]:
    """Each labelled value as <model>, refusing at the first body that does not fit."""
    validated: list[M] = []
    for label, value in entries:
        try:
            validated.append(model.model_validate(value))
        except ValidationError as e:
            error = e.errors()[0]
            where = field_path(error["loc"])
            # An empty path is the body itself, which is not an object.
            problem = f"{where}: {error['msg']}" if where else "the body is not a JSON object"
            raise GateFailure(
                f"arm {arm}: {label}: {problem}",
                "the gates cannot read this body, so the run describes nothing. No record written.",
            ) from None
    return tuple(validated)
