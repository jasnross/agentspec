"""A Claude Messages API request body, as Claude Code sends it through the sink."""

from typing import Annotated

from pydantic import MISSING, Field

from probe_harness.models.raw import STRING_OR_LIST, ProviderModel, RawJson, tag, type_is


class InputSchema(ProviderModel):
    properties: RawJson


class ClaudeTool(ProviderModel):
    name: str
    # Server tools (`type`-tagged, such as web search) carry no schema.
    input_schema: InputSchema | MISSING = MISSING


class OutputConfig(ProviderModel):
    # Claude's intermittent title-generation request carries an `output_config` with no
    # `effort`; see `claude-agent-effort/README.md` and gate 3 (`gates.claude_otel.control`).
    effort: str | MISSING = MISSING


class TextPart(ProviderModel):
    text: str


class OtherBlock(ProviderModel):
    type: str


type ToolResultPart = Annotated[
    Annotated[TextPart, tag("text")] | Annotated[OtherBlock, tag("other")], type_is("text")
]


class ToolResultBlock(ProviderModel):
    tool_use_id: str
    content: Annotated[
        Annotated[str, tag("string")]
        | Annotated[tuple[ToolResultPart, ...], Field(strict=False), tag("list")],
        STRING_OR_LIST,
    ]

    @property
    def text(self) -> str:
        """The content as text: the string, or its text parts joined."""
        if isinstance(self.content, str):
            return self.content
        return "".join(part.text for part in self.content if isinstance(part, TextPart))


type ClaudeContentBlock = Annotated[
    Annotated[ToolResultBlock, tag("tool_result")] | Annotated[OtherBlock, tag("other")],
    type_is("tool_result"),
]


class ClaudeMessage(ProviderModel):
    role: str
    content: Annotated[
        Annotated[str, tag("string")]
        | Annotated[tuple[ClaudeContentBlock, ...], Field(strict=False), tag("list")],
        STRING_OR_LIST,
    ]


class ClaudeRequest(ProviderModel):
    model: str
    system: RawJson
    messages: Annotated[tuple[ClaudeMessage, ...], Field(strict=False)]
    # Absent on requests that offer no tools.
    tools: Annotated[tuple[ClaudeTool, ...], Field(strict=False)] | MISSING = MISSING
    output_config: OutputConfig

    @property
    def tool_list(self) -> tuple[ClaudeTool, ...]:
        """The request's tools; empty when it offers none."""
        return () if self.tools is MISSING else self.tools
