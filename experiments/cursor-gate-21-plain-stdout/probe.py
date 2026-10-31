"""Gate #21 — does Cursor inject a sessionStart hook's plain stdout into the
agent's context, the way Claude does?

Human-judged: whether the agent knew the planted fact is a person's call.
"""

from collections.abc import Sequence

from probe_harness.families.manual import CaptureLine, ManualProbe, Unparsed
from probe_harness.models.cursor_hook import CursorHookPayload, cursor_version


def _wait_for(lines: Sequence[CaptureLine[CursorHookPayload]]) -> bool:
    """The hook fired at least once."""
    return len(lines) > 0


def _capture_version(lines: Sequence[CursorHookPayload | Unparsed]) -> str | None:
    return cursor_version(
        line.cursor_version for line in lines if isinstance(line, CursorHookPayload)
    )


PROBE = ManualProbe(
    instructions="""
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start.
  3. Start a FRESH conversation (not a resume — sessionStart does not fire
     on resume, per the cursor-session-start probe).
  4. Ask the agent: "What pet does the user own and what is its name?"

This script is waiting. Once the hook has fired it will ask you what happened.
""",
    payload=CursorHookPayload,
    wait_for=_wait_for,
    capture_version=_capture_version,
)
