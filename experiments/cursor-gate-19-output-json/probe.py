"""Gate #19 — when a preToolUse hook exits 2 AND emits deny JSON, which of its
marker strings does Cursor surface?

Human-judged: the oracle is a person looking at the screen. The runner proves
the hook fired before asking, so the human answers only what no machine can.
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
  3. Ask the agent to run a shell command, e.g. "Run: ls".
     The hook will deny it.
  4. Look for AGENTSPEC_GATE19_USER_MARKER_0123456789 in Cursor's UI.
  5. In a follow-up prompt, ask the agent to repeat back the last hook
     message it saw — look for AGENTSPEC_GATE19_AGENT_MARKER_9876543210.

This script is waiting. Once the hook has fired it will ask you what you saw.
""",
    payload=CursorHookPayload,
    wait_for=_wait_for,
    capture_version=_capture_version,
)
