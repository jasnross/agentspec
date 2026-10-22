"""Does Cursor parse a `[effort=…]` bracket option in subagent frontmatter?

Human-driven, machine-observed: a person drives Cursor, but the answer lands
in a hook payload and no one has to interpret it.
"""

from probe_harness.families.manual import ManualProbe

PROBE = ManualProbe(
    instructions="""
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start,
     so a window opened before this point will not fire the hook.
  3. Invoke the "arm-effort-low" subagent.

This script is waiting and will record the result automatically.
Nothing else is required of you.
""",
    variants=("assertion", "discriminator"),
)
