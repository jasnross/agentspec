"""Which tools does OpenCode send the model when an agent's `permission` map
leads with `"*": "deny"` and then allows built-ins, a lone `edit`, or one MCP
tool — and when the deny-all comes after the allow instead?

Fully script-driven: no human step, no credentials, and no model quota.
OpenCode talks to a fake OpenAI-compatible provider on loopback, which writes
every request body to disk. That dump is the oracle, so the measurement is at
`outbound-request` depth rather than the `resolved-config` depth of
`opencode debug agent`.
"""

from probe_harness.families.base import CustomCheck
from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeArms, OpenCodeFakeProbe
from probe_harness.gates.opencode import APPARATUS_FAILURE, governed_tool_names, toolset
from probe_harness.json_values import JsonValue

_MARKER = "AGENTSPEC-PROBE-MARKER-OCPERM5"


def _assertion(arms: OpenCodeArms) -> JsonValue:
    """Each measured arm's tool set: one sorted list when its governed requests agree."""
    return {
        arm: toolset(arms[arm].requests, _MARKER)
        for arm in (
            "read_edit_bash",
            "edit_only",
            "mcp_one",
            "reversed",
        )
    }


def _baseline_offers_fx(arms: OpenCodeArms) -> bool:
    offered = governed_tool_names(arms["baseline"].requests, _MARKER)
    return offered.issuperset(("fx_alpha", "fx_beta"))


# The stamp proves the server connected; this proves its tools reach a request when nothing
# filters them.
_BASELINE_OFFERS_FX = CustomCheck(
    "baseline_offers_fx",
    _baseline_offers_fx,
    (
        "arm baseline: the fx server connected but fx_alpha and fx_beta never reached the request.",
        APPARATUS_FAILURE,
    ),
)

PROBE = OpenCodeFakeProbe(
    marker=_MARKER,
    arms=(
        OpenCodeArm("baseline", "probe-baseline"),
        OpenCodeArm("read_edit_bash", "probe-read-edit-bash"),
        OpenCodeArm("edit_only", "probe-edit-only"),
        OpenCodeArm("mcp_one", "probe-mcp-one"),
        OpenCodeArm("reversed", "probe-reversed"),
    ),
    fx_servers=("fx",),
    assertion=_assertion,
    custom_checks=(_BASELINE_OFFERS_FX,),
    variants=("assertion", "discriminator"),
)
