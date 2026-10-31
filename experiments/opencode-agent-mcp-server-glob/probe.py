"""Under an OpenCode agent `permission` map that leads with `"*": "deny"`,
which tools does a `fx_*` allow offer when servers `fx` and `fx_extra` are
both connected?

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

_MARKER = "AGENTSPEC-PROBE-MARKER-OCGLOB2"
_ALL_FX = {"fx_alpha", "fx_beta", "fx_extra_alpha", "fx_extra_beta"}


def _assertion(arms: OpenCodeArms) -> JsonValue:
    """The glob arm's tool set: one sorted list when its governed requests agree."""
    return {"server_glob": toolset(arms["server_glob"].requests, _MARKER)}


def _baseline_offers_all(arms: OpenCodeArms) -> bool:
    return governed_tool_names(arms["baseline"].requests, _MARKER).issuperset(_ALL_FX)


# The stamps prove both servers connected; this proves all four tools reach a request when
# nothing filters them, so the glob arm cannot read as under-matching by default.
_BASELINE_OFFERS_ALL = CustomCheck(
    "baseline_offers_all",
    _baseline_offers_all,
    (
        "arm baseline: the servers connected but fx_alpha, fx_beta, fx_extra_alpha, and "
        "fx_extra_beta did not all reach the request.",
        APPARATUS_FAILURE,
    ),
)

PROBE = OpenCodeFakeProbe(
    marker=_MARKER,
    arms=(
        OpenCodeArm("baseline", "probe-baseline"),
        OpenCodeArm("server_glob", "probe-server-glob"),
    ),
    fx_servers=("fx", "fx_extra"),
    assertion=_assertion,
    custom_checks=(_BASELINE_OFFERS_ALL,),
    variants=("assertion", "discriminator"),
)
