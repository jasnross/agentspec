"""The Claude and Cursor hook payload models a manual probe parses its capture into."""

import pytest
from probe_harness.models.claude_hook import ClaudeHookPayload
from probe_harness.models.cursor_hook import (
    CursorArgvEnvelope,
    CursorHookPayload,
    UnparsedPayload,
    cursor_version,
)
from pydantic import MISSING, ValidationError

CURSOR = {"hook_event_name": "sessionStart", "conversation_id": "c1", "cursor_version": "3.0.1"}


def test_a_claude_payload_carries_source_only_when_the_event_does() -> None:
    assert (
        ClaudeHookPayload.model_validate({"hook_event_name": "UserPromptSubmit"}).source is MISSING
    )
    start = ClaudeHookPayload.model_validate(
        {"hook_event_name": "SessionStart", "source": "resume"}
    )
    assert start.source == "resume"


def test_a_cursor_payload_without_its_conversation_is_refused() -> None:
    with pytest.raises(ValidationError):
        CursorHookPayload.model_validate({"hook_event_name": "sessionStart", "cursor_version": "3"})


def test_an_argv_envelope_holds_its_arguments_around_a_cursor_payload() -> None:
    envelope = CursorArgvEnvelope.model_validate(
        {
            "case": None,
            "argv0": "/tmp/dump-hook.sh",
            "argc": 0,
            "argv": [],
            "probe_env": "",
            "payload": {**CURSOR, "hook_event_name": "beforeSubmitPrompt"},
        }
    )
    assert envelope.case is None
    assert isinstance(envelope.payload, CursorHookPayload)
    assert envelope.payload.conversation_id == "c1"


def test_an_argv_envelope_accepts_the_stand_in_for_a_payload_that_was_not_json() -> None:
    # The hook writes `{"unparsed": <raw>}` for an empty or non-object stdin, so the line's
    # argv still counts rather than costing the capture.
    envelope = CursorArgvEnvelope.model_validate(
        {
            "case": "split",
            "argv0": "/tmp/dump-hook.sh",
            "argc": 3,
            "argv": ["split", "alpha", "beta"],
            "probe_env": "",
            "payload": {"unparsed": ""},
        }
    )
    assert envelope.payload == UnparsedPayload(unparsed="")


def test_cursor_version_is_the_first_version_or_none() -> None:
    assert cursor_version(["3.0.1", "3.0.2"]) == "3.0.1"
    assert cursor_version([]) is None
