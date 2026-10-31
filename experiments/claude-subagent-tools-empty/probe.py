"""Does a Claude subagent whose `tools` field is an empty list receive no tools,
or the same tools as a subagent with no `tools` field?

Three arms, each delegating to an agent that differs from its siblings only in
`tools`. Every arm spends a billed model call, which is why this package
declares `driver: billed` and `just probe-run` withholds it without
`--billed`.
"""

from probe_harness.assertions import consistent, sorted_values, unique
from probe_harness.families.base import CustomCheck
from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeArms,
    ClaudeOtelProbe,
    Model,
)
from probe_harness.gates.bodies import DEFERRED_LISTING, governed_requests
from probe_harness.json_values import JsonValue
from probe_harness.models.claude_request import ClaudeRequest

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

_RUN_IS_SAFE = "this is a statement about the run, not about Claude. No record written."

_MARKER = "AGENTSPEC-PROBE-MARKER-CCEMPTY8"
# What a subagent offered no tools at all yields: nothing direct, and no deferred listing.
_NO_TOOLS: JsonValue = {"direct": [], "deferred": False}


def _offered(arm: str, arms: ClaudeArms) -> JsonValue:
    """The arm's tools: one `{direct, deferred}` when its governed requests agree, else
    `{"inconsistent": [...]}`, or `NO_GOVERNED_REQUEST` when it had none."""
    shapes: list[JsonValue] = [
        {
            "direct": sorted_values([t.name for t in r.tool_list]),
            "deferred": any(DEFERRED_LISTING in s for s in r.raw.strings()),
        }
        for r in governed_requests(arms[arm].requests, _MARKER)
    ]
    return consistent(unique(shapes))


def _assertion(arms: ClaudeArms) -> JsonValue:
    """Whether the `empty` subagent got no tools, or the same tools as `inherit`."""
    empty = _offered("empty", arms)
    verdict: JsonValue
    if isinstance(empty, str):
        verdict = empty
    elif empty == _NO_TOOLS:
        verdict = "no-tools"
    elif empty == _offered("inherit", arms):
        verdict = "same-as-inherit"
    else:
        verdict = empty
    return {"empty": verdict}


def _offers_read(request: ClaudeRequest) -> bool:
    return any(t.name == "Read" for t in request.tool_list)


def _tool_names(requests: list[ClaudeRequest]) -> set[str]:
    return {t.name for r in requests for t in r.tool_list}


def _inherit_gets_builtins(arms: ClaudeArms) -> bool:
    inherit = governed_requests(arms["inherit"].requests, _MARKER)
    return bool(inherit) and all(_offers_read(r) for r in inherit)


def _listed_tools_restrict(arms: ClaudeArms) -> bool:
    read = governed_requests(arms["read"].requests, _MARKER)
    inherit = governed_requests(arms["inherit"].requests, _MARKER)
    return (
        bool(read)
        and all(_offers_read(r) for r in read)
        and len(_tool_names(read)) < len(_tool_names(inherit))
    )


# An unrestricted subagent receives the built-ins, so an empty tool set elsewhere describes
# a restriction rather than a subagent that never got tools.
_INHERIT_GETS_BUILTINS = CustomCheck(
    "inherit_gets_builtins",
    _inherit_gets_builtins,
    (
        "arm inherit: a governed request offered no Read in tools[].",
        "an unrestricted subagent did not receive built-ins, so an empty tool set "
        "elsewhere describes nothing.",
        _RUN_IS_SAFE,
    ),
)

# A listed `tools` field restricts the subagent: `read` offers Read, and fewer tools than
# `inherit`. Without this, `empty`'s value could not be told from an ignored field.
_LISTED_TOOLS_RESTRICT = CustomCheck(
    "listed_tools_restrict",
    _listed_tools_restrict,
    (
        "arm read: the governed requests did not offer Read with fewer tools than arm inherit.",
        "a listed tools field did not restrict the subagent, so the value for empty "
        "cannot be read.",
        _RUN_IS_SAFE,
    ),
)

PROBE = ClaudeOtelProbe(
    model="claude-sonnet-5-5",
    budget_usd="0.50",
    marker=_MARKER,
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
    assertion=_assertion,
    custom_checks=(_INHERIT_GETS_BUILTINS, _LISTED_TOOLS_RESTRICT),
    variants=("assertion", "discriminator"),
)
