"""When a skill is model-invoked mid-session, supplied as the session's entry
prompt, or forked, does Claude Code apply its `effort:` frontmatter to the
outbound model requests it governs?

Three arms, one per path this package measures. A fourth path — a slash
command typed into an already-running interactive session — is not reachable
from `claude -p`, which is one turn; see the README and `TODO.md`. Every arm
spends a billed model call, which is why this package declares `driver: billed`
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

_MARKER = "AGENTSPEC-PROBE-MARKER-K2M9RT"
# A skill's body arrives in `messages[]`, never in the system prompt.
_FIELD = MarkerField.MESSAGES


def _assertion(arms: ClaudeArms) -> JsonValue:
    """Each arm's governed efforts, relative to the session's own level."""
    return {arm: relative_effort(arms, arm, _MARKER, _FIELD) for arm in arms}


PROBE = ClaudeOtelProbe(
    model="claude-opus-4-8",
    budget_usd="0.50",
    marker=_MARKER,
    arms=(
        ClaudeArm(
            "inline",
            "project",
            "Run the agentspec effort probe check.",
            ("--allowedTools", "Skill"),
        ),
        ClaudeArm("slash_entry", "project", "/probe-effort run the check"),
        ClaudeArm(
            "fork",
            "project-fork",
            "Run the agentspec effort probe check.",
            ("--allowedTools", "Skill"),
        ),
    ),
    gates=(Control(),),
    marker_field=_FIELD,
    assertion=_assertion,
    variants=("assertion", "discriminator"),
)
