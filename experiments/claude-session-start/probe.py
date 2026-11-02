"""Does Claude Code's SessionStart hook fire again when a session is resumed?

Human-driven, machine-observed: the assertion is the ordered list of `source`
values, so a person drives Claude but nobody has to interpret the answer.
"""

from collections.abc import Sequence

from probe_harness.families.base import WrongVersion
from probe_harness.families.manual import CaptureLine, ManualProbe, Unparsed
from probe_harness.json_values import JsonValue
from probe_harness.models.claude_hook import ClaudeHookPayload
from pydantic import MISSING

type _Line = ClaudeHookPayload | Unparsed


def _wait_for(lines: Sequence[CaptureLine[ClaudeHookPayload]]) -> bool:
    """A resumed session has started."""
    return any(
        isinstance(line, ClaudeHookPayload)
        and line.hook_event_name == "SessionStart"
        and line.source == "resume"
        for line in lines
    )


def _assertion(lines: Sequence[_Line]) -> JsonValue:
    """The `source` of every `SessionStart` after the first `UserPromptSubmit`.

    With no `UserPromptSubmit`, every line after the first counts, as the jq this replaced
    read a missing index as zero. An `Unparsed` line keeps its position.
    """
    names = [
        line.hook_event_name if isinstance(line, ClaudeHookPayload) else None for line in lines
    ]
    start = (names.index("UserPromptSubmit") if "UserPromptSubmit" in names else 0) + 1
    return [
        None if line.source is MISSING else line.source
        for line in lines[start:]
        if isinstance(line, ClaudeHookPayload) and line.hook_event_name == "SessionStart"
    ]


def _drops_unparsed_lines(lines: Sequence[_Line]) -> JsonValue:
    """A wrong version of `_assertion`: it drops `Unparsed` lines before indexing, so with no
    `UserPromptSubmit` an unparsed first line no longer takes the place of the line skipped."""
    return _assertion([line for line in lines if isinstance(line, ClaudeHookPayload)])


PROBE = ManualProbe(
    instructions="""
  1. cd into that directory and run: claude
  2. Submit one prompt (anything), then exit.
  3. Relaunch with: claude --resume   (or: claude -c)
  4. Submit another prompt.

This script is waiting and will record the result automatically.
""",
    variants=("assertion", "discriminator"),
    payload=ClaudeHookPayload,
    wait_for=_wait_for,
    assertion=_assertion,
    wrong_versions=(
        WrongVersion("drops unparsed lines before indexing", "assertion", _drops_unparsed_lines),
    ),
)
