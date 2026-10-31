"""An OpenAI-compatible chat request body, as OpenCode sends it to the fake provider."""

from typing import Annotated

from pydantic import MISSING, Field

from probe_harness.models.raw import ProviderModel


class OpenCodeMessage(ProviderModel):
    role: str
    # A string for every role in every capture.
    content: str


class OpenCodeFunction(ProviderModel):
    name: str


class OpenCodeTool(ProviderModel):
    function: OpenCodeFunction


class OpenCodeRequest(ProviderModel):
    messages: Annotated[tuple[OpenCodeMessage, ...], Field(strict=False)]
    # Absent, never `[]`, on a title-generation request and on a governed request offering
    # no tools, as `opencode-agent-tools-deny-all`'s `reversed` arm sends.
    tools: Annotated[tuple[OpenCodeTool, ...], Field(strict=False)] | MISSING = MISSING

    @property
    def tool_names(self) -> tuple[str, ...]:
        """The offered tools' names, in request order; `()` when the request offers none."""
        if self.tools is MISSING:
            return ()
        return tuple(tool.function.name for tool in self.tools)
