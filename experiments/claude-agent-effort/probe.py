"""On which invocation paths does Claude Code apply an agent's `effort:`
frontmatter to the outbound model request?

Two arms, one per path the same agent file can be reached by: `--agent` makes
it the session agent, and the Task tool delegates to it as a subagent. Both
spend a billed model call, which is why this package declares `driver: billed`
and `just probe-run` withholds it without `--billed`.
"""

from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeArms,
    ClaudeOtelProbe,
    Control,
    MarkerField,
)
from probe_harness.gates.claude_otel import relative_effort
from probe_harness.json_values import JsonValue

_MARKER = "AGENTSPEC-PROBE-MARKER-7Q4XKD"
# An agent file becomes the system prompt of the requests it governs.
_FIELD = MarkerField.SYSTEM


def _assertion(arms: ClaudeArms) -> JsonValue:
    """Each arm's governed efforts, relative to the session's own level."""
    return {arm: relative_effort(arms, arm, _MARKER, _FIELD) for arm in arms}


PROBE = ClaudeOtelProbe(
    model="claude-opus-4-8",
    budget_usd="0.50",
    marker=_MARKER,
    arms=(
        ClaudeArm(
            "session_agent",
            "project",
            "Reply with exactly one word: ok.",
            ("--agent", "probe-effort"),
        ),
        ClaudeArm(
            "delegated",
            "project",
            "Use the Task tool to delegate to the probe-effort subagent. Do not answer yourself.",
            ("--allowedTools", "Task"),
        ),
    ),
    gates=(Control(),),
    marker_field=_FIELD,
    assertion=_assertion,
    variants=("assertion", "discriminator"),
)
