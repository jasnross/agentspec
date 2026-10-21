"""On which invocation paths does Claude Code apply an agent's `effort:`
frontmatter to the outbound model request?

Two arms, one per path the same agent file can be reached by: `--agent` makes
it the session agent, and the Task tool delegates to it as a subagent. Both
spend a billed model call, which is why this package declares `driver: billed`
and `just probe-run` withholds it without `--billed`.
"""

from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeOtelProbe,
    Control,
)

PROBE = ClaudeOtelProbe(
    model="claude-opus-4-8",
    budget_usd="0.50",
    marker="AGENTSPEC-PROBE-MARKER-7Q4XKD",
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
    variants=("assertion", "discriminator"),
)
