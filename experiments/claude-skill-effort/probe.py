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
    ClaudeOtelProbe,
    Control,
    MarkerField,
)

PROBE = ClaudeOtelProbe(
    model="claude-opus-4-8",
    budget_usd="0.50",
    marker="AGENTSPEC-PROBE-MARKER-K2M9RT",
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
    marker_field=MarkerField.MESSAGES,
    variants=("assertion", "discriminator"),
)
