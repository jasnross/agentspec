"""How does Cursor turn a hooks.json `command` string into the hook process's
argv? Cursor documents neither the interpreter that reads the string nor any
escaping rules for it, so agentspec composes commands containing spaces and a
leading `VAR=value` assignment on an unverified assumption.

Machine-judged: the answer is counted from the capture by the assertion, so a
person drives Cursor but no person interprets the result.
"""

from collections.abc import Sequence

from probe_harness.families.base import GateFailure, ProbeFunctions
from probe_harness.families.manual import CaptureLine, ManualProbe, Unparsed
from probe_harness.json_values import JsonValue
from probe_harness.models.cursor_hook import CursorArgvEnvelope, CursorHookPayload, cursor_version
from probe_harness.workspace import workspace_root


def _controls(lines: Sequence[object]) -> bool:
    """Both controls, the hook entries passing no arguments, fired."""
    return sum(1 for line in lines if isinstance(line, CursorArgvEnvelope) and line.argc == 0) >= 2


def _wait_for(lines: Sequence[CaptureLine[CursorArgvEnvelope]]) -> bool:
    return _controls(lines)


def _assertion(lines: Sequence[CursorArgvEnvelope | Unparsed]) -> JsonValue:
    """Whether each case's command reached the hook with the argv, argv0 or environment it
    spells; a case whose hook never fired reads `false`."""
    # The last invocation of each case, as the jq this replaced kept it.
    cases = {
        line.case: line
        for line in lines
        if isinstance(line, CursorArgvEnvelope) and line.case is not None
    }

    def argv(case: str, *expected: str) -> bool:
        return case in cases and cases[case].argv == expected

    split = cases.get("split")
    var = cases["var"].argv[1] if "var" in cases and len(cases["var"].argv) > 1 else ""
    return {
        "controls": _controls(lines),
        "argv0": split is not None and split.argv0.endswith("dump-hook.sh"),
        "split": argv("split", "split", "alpha", "beta"),
        "dquote": argv("dquote", "dquote", "two words"),
        "squote": argv("squote", "squote", "two words"),
        "var": var != "$HOME" and var.startswith("/"),
        "subst": argv("subst", "subst", "SUBSTITUTED"),
        "meta": argv("meta", "meta", "a;b|c&d"),
        "utf8": argv("utf8", "utf8", "café-日本-ñ"),
        "envprefix": "envprefix" in cases and cases["envprefix"].probe_env == "probe-value",
        "envprefix_control": split is not None and split.probe_env == "",
        "anchor": argv("anchor", "anchor", "/tmp/user-script.sh", "my-hook-id", "arg with spaces"),
    }


def _capture_version(lines: Sequence[CursorArgvEnvelope | Unparsed]) -> str | None:
    return cursor_version(
        line.payload.cursor_version
        for line in lines
        if isinstance(line, CursorArgvEnvelope) and isinstance(line.payload, CursorHookPayload)
    )


# Whitespace, quotes, and every character a shell acts on.
_SHELL_SIGNIFICANT = frozenset(" \t\n'\"$`;&|()<>*?!\\")


def _preflight() -> None:
    """Refuse a probe workspace root that would invalidate the measurement.

    Every workspace is created under the workspace root (`$XDG_STATE_HOME`, or `$HOME`,
    plus `agentspec/probe-workspaces`), so it is the only component of the capture-script
    path that this repository does not control. That path is substituted
    into a `command` string the provider is expected to hand to a shell, so any character
    with meaning to a shell invalidates the measurement rather than merely inconveniencing
    it: whitespace puts an extra word before the case id, a quote re-quotes a later case,
    and `$` or a backtick expands to something that resolves to no executable, so nothing
    fires at all.

    Nothing downstream catches any of it: templating rejects only an unsubstituted
    placeholder, and the generated-JSON check rejects only a path that breaks JSON. Every
    character here produces valid JSON. Fail before the operator spends a live session.
    """
    root = str(workspace_root())
    if _SHELL_SIGNIFICANT.intersection(root):
        raise GateFailure(
            f"the probe workspace root contains whitespace or a shell metacharacter ({root});",
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
    variants=("assertion", "discriminator"),
    payload=CursorArgvEnvelope,
    wait_for=_wait_for,
    assertion=_assertion,
    capture_version=_capture_version,
)
