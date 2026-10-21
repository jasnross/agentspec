"""A Claude Messages API response body, as the sink records it."""

from typing import Annotated, Any

from pydantic import MISSING, Discriminator, Field

from probe_harness.models.claude_request import OtherBlock
from probe_harness.models.raw import ProviderModel, tag


class ToolUseBlock(ProviderModel):
    """A call to any tool. Its input stays undeclared: a gate never reads another tool's
    input, so a malformed one cannot refuse the run."""

    id: str
    name: str


class AgentInput(ProviderModel):
    # Set only when the model chose a delegation mode.
    run_in_background: bool | MISSING = MISSING


class AgentCall(ToolUseBlock):
    """A call to the `Agent` tool, whose input the delegation gates read."""

    input: AgentInput


def _block_kind(value: Any) -> str:
    if isinstance(value, dict):
        block_type, name = value.get("type"), value.get("name")
    else:
        block_type, name = getattr(value, "type", None), getattr(value, "name", None)
    if block_type != "tool_use":
        return "<other>"
    return "<agent>" if name == "Agent" else "<tool_use>"


type ResponseBlock = Annotated[
    Annotated[AgentCall, tag("agent")]
    | Annotated[ToolUseBlock, tag("tool_use")]
    | Annotated[OtherBlock, tag("other")],
    Discriminator(_block_kind),
]


class ClaudeResponse(ProviderModel):
    # An API error response has no `content`.
    content: Annotated[tuple[ResponseBlock, ...], Field(strict=False)] | MISSING = MISSING
