"""Does OpenCode discard `model`, `variant`, and `tools` from skill frontmatter?

Fully script-driven: no human step, no network, no credentials.
"""

from probe_harness.families.base import GateFailure, ProbeFunctions
from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.models.opencode_debug import OpenCodeSkills

_FIXTURE_SKILL = "agentspec-probe-discard"


def _judge(skills: OpenCodeSkills) -> None:
    """Refuse a view in which the fixture skill did not resolve exactly once.

    This oracle enumerates every resolvable skill rather than addressing one by name, so a
    fixture it never discovered is not an error: the run exits 0 with a valid view of all
    the *other* skills, the projection resolves to `null`, and the recorder writes
    `refuted`, the strong "the provider changed" signal, permanently.
    """
    matches = sum(1 for skill in skills.root if skill.name == _FIXTURE_SKILL)
    if matches == 1:
        return
    # The names, not just a count: the likeliest cause is a change in how OpenCode discovers
    # skills (`.opencode/skill/` versus `.opencode/skills/` is itself unmeasured), and only
    # the names reveal that.
    raise GateFailure(
        f"the fixture resolved {matches} times, expected exactly 1. Resolved skills:",
        *(f"  {skill.name}" for skill in skills.root),
        "this is an apparatus failure, not a refutation; no record written.",
    )


PROBE = OpenCodeDebugProbe(
    argv=("skill", "--pure"),
    output=OpenCodeSkills,
    variants=("assertion", "discriminator"),
    functions=ProbeFunctions(judge=_judge),
)
