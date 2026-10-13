"""Does OpenCode read a top-level `variant:` key in command frontmatter?

Fully script-driven: no human step, no network, no credentials.
"""

from pathlib import Path

from probe_harness import json_values
from probe_harness.families.base import GateFailure, ProbeFunctions
from probe_harness.families.opencode_debug import OpenCodeDebugProbe

_FIXTURE_COMMAND = "agentspec-probe-variant"


def _judge(ws: Path) -> None:
    """Refuse a view in which the fixture command never resolved.

    This oracle resolves the whole config rather than addressing one command by name, so a
    fixture it never discovered is not an error: the run exits 0 with a valid view, the
    projection yields `{"model":null,"variant":null}`, and the recorder writes `refuted`,
    the strong "the provider changed" signal, permanently. The discriminator fixture
    refutes on a *present* command resolving `variant` to `null`, which is a different
    observation.
    """
    view = json_values.loads((ws / "view.json").read_text(encoding="utf-8"))
    # A config with no commands at all omits the key, and is still this failure.
    commands = (view.get("command") if isinstance(view, dict) else None) or {}
    if not isinstance(commands, dict):
        commands = {}
    if _FIXTURE_COMMAND in commands:
        return
    # The names, not just a count: the likeliest cause is a change in how OpenCode
    # discovers or namespaces commands, and only the names reveal that.
    raise GateFailure(
        f"the fixture command did not resolve. OpenCode resolved {len(commands)} commands:",
        *(f"  {name}" for name in sorted(commands)),
        "this is an apparatus failure, not a refutation; no record written.",
    )


PROBE = OpenCodeDebugProbe(
    argv=("config", "--pure"),
    variants=("assertion", "discriminator"),
    functions=ProbeFunctions(judge=_judge),
)
