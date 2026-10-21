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
from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe
from probe_harness.gates.opencode import APPARATUS_FAILURE, GOVERNED_JQ

# The stamps prove both servers connected; this proves all four tools reach a request when
# nothing filters them, so the glob arm cannot read as under-matching by default.
_BASELINE_OFFERS_ALL = CustomCheck(
    expression=(
        f"[.baseline[] | select(any({GOVERNED_JQ})) | .tools[]?.function.name] as $n"
        ' | all("fx_alpha", "fx_beta", "fx_extra_alpha", "fx_extra_beta";'
        " . as $t | $n | index($t))"
    ),
    failure=(
        "arm baseline: the servers connected but fx_alpha, fx_beta, fx_extra_alpha, and "
        "fx_extra_beta did not all reach the request.",
        APPARATUS_FAILURE,
    ),
)

PROBE = OpenCodeFakeProbe(
    marker="AGENTSPEC-PROBE-MARKER-OCGLOB2",
    arms=(
        OpenCodeArm("baseline", "probe-baseline"),
        OpenCodeArm("server_glob", "probe-server-glob"),
    ),
    fx_servers=("fx", "fx_extra"),
    custom_checks=(_BASELINE_OFFERS_ALL,),
    variants=("assertion", "discriminator"),
)
