"""Does Cursor's sessionStart hook fire again when a conversation is resumed?

The finding is an absence, so the assertion is a COUNT of sessionStart
payloads. A poll cannot wait for a payload that never comes, so wait_for
counts prompts instead: two beforeSubmitPrompt payloads prove the operator
reached the second half of the procedure.
"""

from collections.abc import Sequence

from probe_harness.families.manual import CaptureLine, ManualProbe, Unparsed
from probe_harness.json_values import JsonValue
from probe_harness.models.cursor_hook import CursorHookPayload, cursor_version

type _Line = CursorHookPayload | Unparsed


def _prompt_conversations(lines: Sequence[object]) -> list[str]:
    """Each `beforeSubmitPrompt`'s conversation, in capture order, repeats kept."""
    return [
        line.conversation_id
        for line in lines
        if isinstance(line, CursorHookPayload) and line.hook_event_name == "beforeSubmitPrompt"
    ]


def _session_starts(lines: Sequence[object], conversation: str | None) -> int:
    """How many `sessionStart` payloads among <lines> belong to <conversation>."""
    return sum(
        1
        for line in lines
        if isinstance(line, CursorHookPayload)
        and line.hook_event_name == "sessionStart"
        and line.conversation_id == conversation
    )


def _resumed(conversations: list[str]) -> bool:
    """Two prompts or more, all in one conversation: the session was resumed."""
    return len(conversations) >= 2 and len(set(conversations)) == 1


def _wait_for(lines: Sequence[CaptureLine[CursorHookPayload]]) -> bool:
    """The session was resumed, and its conversation's `sessionStart` has fired."""
    conversations = _prompt_conversations(lines)
    return _resumed(conversations) and _session_starts(lines, conversations[0]) > 0


def _assertion(lines: Sequence[_Line]) -> JsonValue:
    """Whether the session resumed, whether its `sessionStart` fired, and how many times it
    fired after the first prompt.

    With no prompt, every line after the first counts, as the jq this replaced read a
    missing index as zero. An `Unparsed` line keeps its position.
    """
    conversations = _prompt_conversations(lines)
    target = min(conversations, default=None)
    names = [
        line.hook_event_name if isinstance(line, CursorHookPayload) else None for line in lines
    ]
    start = (names.index("beforeSubmitPrompt") if "beforeSubmitPrompt" in names else 0) + 1
    return {
        "resumed": _resumed(conversations),
        "fired": _session_starts(lines, target) > 0,
        "after_resume": _session_starts(lines[start:], target),
    }


def _capture_version(lines: Sequence[CursorHookPayload | Unparsed]) -> str | None:
    return cursor_version(
        line.cursor_version for line in lines if isinstance(line, CursorHookPayload)
    )


PROBE = ManualProbe(
    instructions="""
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start.
  3. Submit a prompt in a new conversation.
  4. Fully quit Cursor, reopen it, and RESUME that same conversation
     from the conversation list.
  5. Submit another prompt in the resumed conversation.

This script is waiting and will record the result automatically.
""",
    payload=CursorHookPayload,
    wait_for=_wait_for,
    assertion=_assertion,
    capture_version=_capture_version,
)
