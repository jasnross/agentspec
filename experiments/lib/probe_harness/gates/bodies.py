"""The request and response bodies the gates read, validated once per arm.

Each body is validated against its provider model in `probe_harness.models`, which states
in one place what a body must look like, so every unexpected shape is one loud refusal
rather than a defence repeated in each gate. A body that fails its model is an apparatus
failure: the judge refuses, naming the arm, the body's file, and the field's path, and
never records.

A gate reads a model's declared fields, and searches a body's source JSON only through
`RawJson`.
"""

from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

from pydantic import BaseModel, ValidationError

from probe_harness.gates import GateFailure
from probe_harness.models.claude_request import ClaudeMessage, ClaudeRequest
from probe_harness.models.claude_response import ClaudeResponse
from probe_harness.models.opencode_request import OpenCodeRequest
from probe_harness.models.raw import field_path
from probe_harness.views import SinkEntry, SinkValues, read_sink

DEFERRED_LISTING = "The following deferred tools are now available via ToolSearch"
SUBAGENT_FLAG = "cc_is_subagent=true"


class MarkerField(StrEnum):
    """The request field a fixture's marker must appear in for the fixture to govern it.

    An agent file becomes the system prompt; a skill's body arrives in `messages[]`.
    """

    SYSTEM = "system"
    MESSAGES = "messages"


@dataclass(frozen=True)
class ClaudeArmBodies:
    """What one arm's sink holds, and where."""

    sink: Path
    requests: tuple[ClaudeRequest, ...]
    responses: tuple[ClaudeResponse, ...]


def claude_arms(sinks: Mapping[str, SinkValues], ws: Path) -> dict[str, ClaudeArmBodies]:
    """Each arm's sink values as models, refusing the run on a body that does not fit."""
    return {
        name: ClaudeArmBodies(
            sink=ws / name / "sink",
            requests=_validated(ClaudeRequest, values.requests, name),
            responses=_validated(ClaudeResponse, values.responses, name),
        )
        for name, values in sinks.items()
    }


def opencode_arms(
    sinks: Mapping[str, SinkValues], ws: Path
) -> dict[str, tuple[OpenCodeRequest, ...]]:
    """Each arm's sink values as models, refusing the run on a body that does not fit."""
    return {
        name: _validated(OpenCodeRequest, values.requests, name) for name, values in sinks.items()
    }


def read_claude_arms(ws: Path, arm_names: Sequence[str]) -> dict[str, ClaudeArmBodies]:
    """Read and validate each `<ws>/<arm>/sink`, for a judge or a probe function."""
    return claude_arms({name: read_sink(ws / name / "sink") for name in arm_names}, ws)


def governed(request: ClaudeRequest, field: MarkerField, marker: str) -> bool:
    """Whether <marker> appears in some string value inside the request's <field>."""
    match field:
        case MarkerField.SYSTEM:
            return any(marker in s for s in request.system.strings())
        case MarkerField.MESSAGES:
            return any(marker in s for m in request.messages for s in _content_strings(m))


def subagent(request: ClaudeRequest) -> bool:
    """Whether some `.system` string carries the subagent flag Claude writes in its billing header.

    A subagent's follow-up turn after a tool call carries no agent system prompt, so no
    marker, yet runs at the subagent's effort; this flag is what still identifies it. It is
    matched anywhere in `.system`, as the effort projections' `$u` matches it.
    """
    return any(SUBAGENT_FLAG in s for s in request.system.strings())


def deferred_tools(request: ClaudeRequest) -> list[str]:
    """The lines of every string in the body that carries the deferred-tools listing."""
    return [line for s in request.raw.strings() if DEFERRED_LISTING in s for line in s.split("\n")]


def listed_tools(request: ClaudeRequest) -> list[str]:
    """The request's direct tool names, then its deferred-listing lines."""
    return [tool.name for tool in request.tool_list] + deferred_tools(request)


def _content_strings(message: ClaudeMessage) -> Iterator[str]:
    """Every string leaf of a message's content; the message's other fields are not searched."""
    if isinstance(message.content, str):
        yield message.content
        return
    for block in message.content:
        yield from block.raw.strings()


def _validated[M: BaseModel](
    model: type[M], entries: Sequence[SinkEntry], arm: str
) -> tuple[M, ...]:
    """Each entry as <model>, refusing at the first body that does not fit."""
    validated: list[M] = []
    for entry in entries:
        try:
            validated.append(model.model_validate(entry.value))
        except ValidationError as e:
            error = e.errors()[0]
            where = field_path(error["loc"])
            # An empty path is the body itself, which is not an object.
            problem = f"{where}: {error['msg']}" if where else "the body is not a JSON object"
            raise GateFailure(
                f"arm {arm}: {entry.label}: {problem}",
                "the gates cannot read this body, so the run describes nothing. No record written.",
            ) from None
    return tuple(validated)
