"""Does OpenCode discard `model`, `variant`, and `tools` from skill frontmatter?

Fully script-driven: no human step, no network, no credentials.
"""

from pathlib import Path

from probe_harness import json_values
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
    view = json_values.loads((ws / "view.json").read_text(encoding="utf-8"))
    # Iterated as the projection iterates it: an array's elements, or an object's values.
    entries = (
        view if isinstance(view, list) else list(view.values()) if isinstance(view, dict) else []
    )
    skills = [skill for skill in entries if isinstance(skill, dict)]
    matches = sum(1 for skill in skills if skill.get("name") == _FIXTURE_SKILL)
    if matches == 1:
        return
    # The names, not just a count: the likeliest cause is a change in how OpenCode discovers
    # skills (`.opencode/skill/` versus `.opencode/skills/` is itself unmeasured), and only
    # the names reveal that.
    raise GateFailure(
        f"the fixture resolved {matches} times, expected exactly 1. Resolved skills:",
        *(f"  {json_values.to_text(skill.get('name'))}" for skill in skills),
        "this is an apparatus failure, not a refutation; no record written.",
    )


PROBE = OpenCodeDebugProbe(
    argv=("skill", "--pure"),
    variants=("assertion", "discriminator"),
    functions=ProbeFunctions(judge=_judge),
)
