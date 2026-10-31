"""A Cursor hook payload, and the envelope `cursor-hook-command-argv`'s hook wraps it in.

Field names are the JSON keys. Each model declares only what a probe reads; the rest of a
payload stays searchable through `raw`.
"""

from collections.abc import Iterable
from typing import Annotated, Any

from pydantic import MISSING, Discriminator, Field

from probe_harness.models.raw import ProviderModel, tag


class CursorHookPayload(ProviderModel):
    """Every event carries the event name, the conversation and the Cursor version."""

    hook_event_name: str
    conversation_id: str
    cursor_version: str
    # Only `subagentStart` carries the subagent's type and resolved model.
    subagent_type: str | MISSING = MISSING
    subagent_model: str | MISSING = MISSING


class UnparsedPayload(ProviderModel):
    """What the argv probe's hook writes as `payload` when its stdin was empty or not a JSON
    object: the raw read, so the line's argv still counts."""

    unparsed: str


def _payload_kind(value: Any) -> str:
    if isinstance(value, dict):
        return "<unparsed>" if list(value) == ["unparsed"] else "<payload>"
    return "<unparsed>" if isinstance(value, UnparsedPayload) else "<payload>"


class CursorArgvEnvelope(ProviderModel):
    """What the argv probe's hook writes per invocation: its arguments around the payload."""

    # The case the hook command named in its first argument; `null` for the two controls,
    # which pass no arguments.
    case: str | None
    argv0: str
    argc: int
    argv: Annotated[tuple[str, ...], Field(strict=False)]
    probe_env: str
    payload: Annotated[
        Annotated[CursorHookPayload, tag("payload")] | Annotated[UnparsedPayload, tag("unparsed")],
        Discriminator(_payload_kind),
    ]


def cursor_version(versions: Iterable[str]) -> str | None:
    """The first Cursor version a capture's payloads carry; None for a capture with none."""
    return next(iter(versions), None)


def subagent_starts(lines: Iterable[object], subagent_type: str) -> list[CursorHookPayload]:
    """The `subagentStart` payloads among <lines> for <subagent_type>, in capture order."""
    return [
        line
        for line in lines
        if isinstance(line, CursorHookPayload)
        and line.hook_event_name == "subagentStart"
        and line.subagent_type == subagent_type
    ]
