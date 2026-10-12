"""Gate #21 — does Cursor inject a sessionStart hook's plain stdout into the
agent's context, the way Claude does?

Human-judged: whether the agent knew the planted fact is a person's call.
"""

from probe_harness.families.manual import ManualProbe

PROBE = ManualProbe(
    instructions="""
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start.
  3. Start a FRESH conversation (not a resume — sessionStart does not fire
     on resume, per the cursor-session-start probe).
  4. Ask the agent: "What pet does the user own and what is its name?"

This script is waiting. Once the hook has fired it will ask you what happened.
""",
)
