"""Does OpenCode read a top-level `variant:` key in command frontmatter?

Fully script-driven: no human step, no network, no credentials.
"""

from probe_harness.families.base import GateFailure, ProbeFunctions
from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.families.opencode_env import OpenCodeIsolation
from probe_harness.json_values import JsonValue
from probe_harness.models.opencode_debug import OpenCodeConfig
from pydantic import MISSING

_FIXTURE_COMMAND = "agentspec-probe-variant"


def _assertion(config: OpenCodeConfig) -> JsonValue:
    """The fixture command's resolved model and variant; `None` for either it lacks."""
    commands = {} if config.command is MISSING else config.command
    command = commands.get(_FIXTURE_COMMAND)
    if command is None:
        return {"model": None, "variant": None}
    return {
        "model": None if command.model is MISSING else command.model,
        "variant": None if command.variant is MISSING else command.variant,
    }


def _judge(config: OpenCodeConfig) -> None:
    """Refuse a view in which the fixture command never resolved.

    This oracle resolves the whole config rather than addressing one command by name, so a
    fixture it never discovered is not an error: the run exits 0 with a valid view, the
    assertion yields `{"model":null,"variant":null}`, and the recorder writes `refuted`,
    the strong "the provider changed" signal, permanently. The discriminator fixture
    refutes on a *present* command resolving `variant` to `null`, which is a different
    observation.
    """
    # A config with no commands at all omits the key, and is still this failure.
    commands = {} if config.command is MISSING else config.command
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
    output=OpenCodeConfig,
    variants=("assertion", "discriminator"),
    isolation=OpenCodeIsolation(),
    assertion=_assertion,
    functions=ProbeFunctions(judge=_judge),
)
