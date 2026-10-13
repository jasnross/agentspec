"""How does Cursor turn a hooks.json `command` string into the hook process's
argv? Cursor documents neither the interpreter that reads the string nor any
escaping rules for it, so agentspec composes commands containing spaces and a
leading `VAR=value` assignment on an unverified assumption.

Machine-judged: the answer is counted from the capture by a projection, so a
person drives Cursor but no person interprets the result.
"""

import os

from probe_harness.families.base import GateFailure, ProbeFunctions
from probe_harness.families.manual import ManualProbe

# Whitespace, quotes, and every character a shell acts on.
_SHELL_SIGNIFICANT = frozenset(" \t\n'\"$`;&|()<>*?!\\")


def _preflight() -> None:
    """Refuse a `$TMPDIR` that would invalidate the measurement.

    Every workspace is created under `$TMPDIR`, so it is the only component of the
    capture-script path that this repository does not control. That path is substituted
    into a `command` string the provider is expected to hand to a shell, so any character
    with meaning to a shell invalidates the measurement rather than merely inconveniencing
    it: whitespace puts an extra word before the case id, a quote re-quotes a later case,
    and `$` or a backtick expands to something that resolves to no executable, so nothing
    fires at all.

    Nothing downstream catches any of it: templating rejects only an unsubstituted
    placeholder, and the generated-JSON check rejects only a path that breaks JSON. Every
    character here produces valid JSON. Fail before the operator spends a live session.
    """
    tmpdir = os.environ.get("TMPDIR") or "/tmp"
    if _SHELL_SIGNIFICANT.intersection(tmpdir):
        raise GateFailure(
            f"TMPDIR contains whitespace or a shell metacharacter ({tmpdir});",
            "the command strings this probe registers would not measure what they mean to.",
        )


PROBE = ManualProbe(
    instructions="""
  1. Open Cursor on that directory.
  2. Fully quit and reopen Cursor — it reads .cursor/hooks.json at start.
  3. Start a FRESH conversation (not a resume — sessionStart does not fire
     on resume, per the cursor-session-start probe).

This script is waiting and will record the result automatically.
""",
    functions=ProbeFunctions(preflight=_preflight),
)
