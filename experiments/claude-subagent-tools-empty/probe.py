"""Does a Claude subagent whose `tools` field is an empty list receive no tools,
or the same tools as a subagent with no `tools` field?

Three arms, each delegating to an agent that differs from its siblings only in
`tools`. Every arm spends a billed model call, which is why this package
declares `driver: billed` and `just probe-run` withholds it without
`--billed`.
"""

from probe_harness.families.base import CustomCheck
from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeOtelProbe,
    Model,
)

# Each would change what is measured: whether tool search is on, which decides whether a
# tool can reach the model through the deferred listing, or which model the subagent runs.
_UNSET = (
    "ENABLE_TOOL_SEARCH",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
)

_ARGS = ("--strict-mcp-config", "--allowedTools", "Agent", "--effort", "low")

_RUN_IS_SAFE = "probe: this is a statement about the run, not about Claude. No record written."

# An unrestricted subagent receives the built-ins, so an empty tool set elsewhere describes
# a restriction rather than a subagent that never got tools.
_INHERIT_GETS_BUILTINS = CustomCheck(
    expression="""
        [.inherit[] | select(.system | tostring | contains($m))]
        | length > 0 and all(.[]; any(.tools[]?; .name == "Read"))
    """,
    failure=(
        "probe: arm inherit: a governed request offered no Read in tools[].",
        "probe: an unrestricted subagent did not receive built-ins, so an empty tool set "
        "elsewhere describes nothing.",
        _RUN_IS_SAFE,
    ),
)

# A listed `tools` field restricts the subagent: `read` offers Read, and fewer tools than
# `inherit`. Without this, `empty`'s value could not be told from an ignored field.
_LISTED_TOOLS_RESTRICT = CustomCheck(
    expression="""
        def governed: [.[] | select(.system | tostring | contains($m))];
        (.read | governed) as $r | (.inherit | governed) as $i
        | ($r | length > 0)
        and all($r[]; any(.tools[]?; .name == "Read"))
        and (([$r[] | .tools[]?.name] | unique | length)
             < ([$i[] | .tools[]?.name] | unique | length))
    """,
    failure=(
        "probe: arm read: the governed requests did not offer Read with fewer tools than "
        "arm inherit.",
        "probe: a listed tools field did not restrict the subagent, so the value for empty "
        "cannot be read.",
        _RUN_IS_SAFE,
    ),
)

PROBE = ClaudeOtelProbe(
    model="claude-sonnet-5-5",
    budget_usd="0.50",
    marker="AGENTSPEC-PROBE-MARKER-CCEMPTY8",
    arms=(
        ClaudeArm(
            "inherit",
            "inherit",
            "Use the Agent tool to delegate to the probe-tools-inherit subagent. Do not answer "
            "yourself.",
            _ARGS,
        ),
        ClaudeArm(
            "empty",
            "empty",
            "Use the Agent tool to delegate to the probe-tools-empty subagent. Do not answer "
            "yourself.",
            _ARGS,
        ),
        ClaudeArm(
            "read",
            "read",
            "Use the Agent tool to delegate to the probe-tools-read subagent. Do not answer "
            "yourself.",
            _ARGS,
        ),
    ),
    gates=(Model(),),
    unset_env=_UNSET,
    custom_checks=(_INHERIT_GETS_BUILTINS, _LISTED_TOOLS_RESTRICT),
)
