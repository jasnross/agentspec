"""Does Cursor parse multiple comma-separated bracket options in subagent
frontmatter?

Human-driven, machine-observed: a person drives Cursor, but the answer lands
in a hook payload and no one has to interpret it.
"""

from collections.abc import Sequence

from probe_harness.families.manual import CaptureLine, ManualProbe, Unparsed
from probe_harness.json_values import JsonValue
from probe_harness.models.cursor_hook import CursorHookPayload, cursor_version, subagent_starts
from pydantic import MISSING

# The subagent the operator invokes, whose `subagentStart` payload the probe reads.
_ARM = "arm-two-options"


def _wait_for(lines: Sequence[CaptureLine[CursorHookPayload]]) -> bool:
    return bool(subagent_starts(lines, _ARM))


def _assertion(lines: Sequence[CursorHookPayload | Unparsed]) -> JsonValue:
    """The model Cursor resolved for the subagent's first start; None when it never started."""
    model = next((start.subagent_model for start in subagent_starts(lines, _ARM)), None)
    return None if model is MISSING else model


def _capture_version(lines: Sequence[CursorHookPayload | Unparsed]) -> str | None:
    return cursor_version(
        line.cursor_version for line in lines if isinstance(line, CursorHookPayload)
    )


PROBE = ManualProbe(
    instructions="""
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start,
     so a window opened before this point will not fire the hook.
  3. Invoke the "arm-two-options" subagent.

This script is waiting and will record the result automatically.
Nothing else is required of you.
""",
    variants=("assertion", "discriminator"),
    payload=CursorHookPayload,
    wait_for=_wait_for,
    assertion=_assertion,
    capture_version=_capture_version,
)
