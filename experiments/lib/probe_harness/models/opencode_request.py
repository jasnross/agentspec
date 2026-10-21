"""An OpenAI-compatible chat request body, as OpenCode sends it to the fake provider."""

from typing import Annotated

from pydantic import Field

from probe_harness.models.raw import ProviderModel, RawJson


class OpenCodeMessage(ProviderModel):
    role: str
    content: RawJson


class OpenCodeRequest(ProviderModel):
    messages: Annotated[tuple[OpenCodeMessage, ...], Field(strict=False)]
