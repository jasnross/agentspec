"""The events `claude -p --output-format stream-json --verbose` prints that a probe reads.

`init` is the session Claude Code resolved before its first model request, which is what
the init family measures; `result` is how the run ended. Both are one JSON object on one
line of stdout.
"""

from typing import Annotated, Literal

from pydantic import Field

from probe_harness.models.raw import ProviderModel


class McpServerStatus(ProviderModel):
    # Where the server came from: `plugin`, `user` and `claudeai` in 2.1.306's captures.
    source: str
    # `connected`, or `pending` while it is still connecting, among others.
    status: str


class ClaudeInit(ProviderModel):
    type: Literal["system"]
    subtype: Literal["init"]
    model: str
    mcp_servers: Annotated[tuple[McpServerStatus, ...], Field(strict=False)]


class ClaudeResult(ProviderModel):
    type: Literal["result"]
    is_error: bool
    # The error a failed run reports, such as the unrecognized model.
    result: str
