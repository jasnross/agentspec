"""Does `onFailure: block` stop a tool call when its PreToolUse hook exits 1, where the same
hook without the field lets the call run?

Two arms, each asking the model for one Bash command behind a `PreToolUse` hook that exits 1.
The `block` arm's hook sets `onFailure: block`, and the `unset` arm's leaves the field out.
Every arm spends a billed model call, which is why this package declares `driver: billed` and
`just probe-run` withholds it without `--billed`.
"""

from probe_harness.families.claude_otel import (
    ClaudeArm,
    ClaudeArms,
    ClaudeOtelProbe,
    MarkerField,
    Model,
    ToolAnswered,
)
from probe_harness.gates.claude_otel import UNRECOGNIZED
from probe_harness.json_values import JsonValue
from probe_harness.models.claude_request import ToolResultBlock

_MARKER = "AGENTSPEC-PROBE-MARKER-CCHOOKFAIL3"
# What the command prints. Lowercased, so a result holding it is a command that ran, where the
# marker itself sits in every request's messages.
_OUTPUT = "agentspec-probe-marker-cchookfail3"
_PROMPT = (
    f"Run this exact command with the Bash tool, then reply with what it printed: "
    f"echo {_MARKER} | tr 'A-Z' 'a-z'"
)
_ARGS = ("--allowedTools", "Bash", "--effort", "low")


def _outcome(results: list[ToolResultBlock]) -> JsonValue:
    """`"ran"` when a result holds the command's output, `"blocked"` when every one is an error,
    else `{"unrecognized": [...]}`, each result's text, so an outcome neither names is visible.
    """
    if any(_OUTPUT in block.text and block.is_error is not True for block in results):
        return "ran"
    if results and all(block.is_error is True for block in results):
        return "blocked"
    texts: list[JsonValue] = [block.text for block in results]
    return {UNRECOGNIZED: texts}


def _assertion(arms: ClaudeArms) -> JsonValue:
    """How each arm's Bash call ended, read only from tool results."""
    return {
        name: _outcome(
            [
                block
                for request in arm.requests
                for message in request.messages
                if message.role == "user" and not isinstance(message.content, str)
                for block in message.content
                if isinstance(block, ToolResultBlock)
            ]
        )
        for name, arm in arms.items()
    }


PROBE = ClaudeOtelProbe(
    model="claude-sonnet-5-5",
    budget_usd="0.50",
    marker=_MARKER,
    arms=(
        ClaudeArm("block", "block", _PROMPT, _ARGS),
        ClaudeArm("unset", "unset", _PROMPT, _ARGS),
    ),
    gates=(Model(), ToolAnswered("block", "Bash"), ToolAnswered("unset", "Bash")),
    marker_field=MarkerField.MESSAGES,
    requires_responses=True,
    assertion=_assertion,
    variants=("assertion", "discriminator"),
)
