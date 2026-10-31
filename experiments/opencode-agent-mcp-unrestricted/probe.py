"""Is an OpenCode agent with no `permission` map offered every tool of a
connected MCP server, where an agent whose map is only `"*": "deny"` (with
`external_directory` and `doom_loop` restated) is offered none?

Fully script-driven: no human step, no credentials, and no model quota.
OpenCode talks to a fake OpenAI-compatible provider on loopback, which writes
every request body to disk. That dump is the oracle, so the measurement is at
`outbound-request` depth rather than the `resolved-config` depth of
`opencode debug agent`.
"""

from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeArms, OpenCodeFakeProbe
from probe_harness.gates.opencode import toolset
from probe_harness.json_values import JsonValue

_MARKER = "AGENTSPEC-PROBE-MARKER-OCMCPU1"


def _assertion(arms: OpenCodeArms) -> JsonValue:
    """Each arm's fx tool set: one sorted list when its governed requests agree."""
    return {
        arm: toolset(arms[arm].requests, _MARKER, keep=lambda name: name.startswith("fx_"))
        for arm in ("no_map", "deny_all")
    }


PROBE = OpenCodeFakeProbe(
    marker=_MARKER,
    arms=(
        OpenCodeArm("no_map", "probe-no-map"),
        OpenCodeArm("deny_all", "probe-deny-all"),
    ),
    fx_servers=("fx",),
    assertion=_assertion,
    variants=("assertion", "discriminator"),
)
