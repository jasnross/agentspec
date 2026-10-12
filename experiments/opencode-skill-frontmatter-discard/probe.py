"""Does OpenCode discard `model`, `variant`, and `tools` from skill frontmatter?

Fully script-driven: no human step, no network, no credentials.
"""

from pathlib import Path

from probe_harness import jq_cli
from probe_harness.families.base import GateFailure, ProbeFunctions
from probe_harness.families.opencode_debug import OpenCodeDebugProbe

_FIXTURE_SKILL = "agentspec-probe-discard"


def _judge(ws: Path) -> None:
    """Refuse a view in which the fixture skill did not resolve exactly once.

    This oracle enumerates every resolvable skill rather than addressing one by name, so a
    fixture it never discovered is not an error: the run exits 0 with a valid view of all
    the *other* skills, the projection resolves to `null`, and the recorder writes
    `refuted`, the strong "the provider changed" signal, permanently.
    """
    view = ws / "view.json"
    matches = jq_cli.evaluate(
        "[.[] | select(.name == $name)] | length", view, args={"name": _FIXTURE_SKILL}
    ).strip()
    if matches == "1":
        return
    # The names, not just a count: the likeliest cause is a change in how OpenCode discovers
    # skills (`.opencode/skill/` versus `.opencode/skills/` is itself unmeasured), and only
    # the names reveal that.
    names = jq_cli.evaluate(".[].name", view, raw=True).splitlines()
    raise GateFailure(
        f"probe: the fixture resolved {matches} times, expected exactly 1. Resolved skills:",
        *(f"probe:   {name}" for name in names),
        "probe: this is an apparatus failure, not a refutation; no record written.",
    )


PROBE = OpenCodeDebugProbe(
    argv=("skill", "--pure"),
    variants=("assertion", "discriminator"),
    functions=ProbeFunctions(judge=_judge),
)
