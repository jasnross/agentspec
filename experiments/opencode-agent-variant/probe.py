"""Does OpenCode read a top-level `variant:` key in agent frontmatter?

Fully script-driven: no human step, no network, no credentials. `opencode debug agent
<name>` is name-addressed and exits nonzero when the agent is missing, so this probe needs
no resolution check of its own.
"""

from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.families.opencode_env import OpenCodeIsolation
from probe_harness.json_values import JsonValue
from probe_harness.models.opencode_debug import OpenCodeAgent
from pydantic import MISSING


def _assertion(agent: OpenCodeAgent) -> JsonValue:
    """The agent's resolved model and variant; `None` for a variant it did not resolve.

    The model is compared as the two keys `OpenCodeAgentModel` declares, so a key OpenCode
    adds to it later changes neither the outcome nor the record.
    """
    return {
        "model": {"providerID": agent.model.providerID, "modelID": agent.model.modelID},
        "variant": None if agent.variant is MISSING else agent.variant,
    }


PROBE = OpenCodeDebugProbe(
    argv=("agent", "probe"),
    output=OpenCodeAgent,
    variants=("assertion", "discriminator"),
    isolation=OpenCodeIsolation(),
    assertion=_assertion,
)
