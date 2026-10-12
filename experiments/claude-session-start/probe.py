"""Does Claude Code's SessionStart hook fire again when a session is resumed?

Human-driven, machine-observed: the assertion is the ordered list of `source`
values, so a person drives Claude but nobody has to interpret the answer.
"""

from probe_harness.families.manual import ManualProbe

PROBE = ManualProbe(
    instructions="""
  1. cd into that directory and run: claude
  2. Submit one prompt (anything), then exit.
  3. Relaunch with: claude --resume   (or: claude -c)
  4. Submit another prompt.

This script is waiting and will record the result automatically.
""",
)
