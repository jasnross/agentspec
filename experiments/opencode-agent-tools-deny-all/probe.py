"""Which tools does OpenCode send the model when an agent's `tools` map denies
everything with `"*": false` and allows one MCP tool — before it, and after?

Fully script-driven: no human step, no credentials, and no model quota.
OpenCode talks to a fake OpenAI-compatible provider on loopback, which writes
every request body to disk. That dump is the oracle, so the measurement is at
`outbound-request` depth rather than the `resolved-config` depth of
`opencode debug agent`.
"""

from probe_harness.families.base import CustomCheck
from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe
from probe_harness.gates.opencode import APPARATUS_FAILURE, GOVERNED_JQ

# The stamp proves the server connected; this proves its tools reach a request when nothing
# filters them.
_BASELINE_OFFERS_FX = CustomCheck(
    expression=(
        f"[.baseline[] | select(any({GOVERNED_JQ})) | .tools[]?.function.name] as $n"
        ' | ($n | index("fx_alpha")) and ($n | index("fx_beta"))'
    ),
    failure=(
        "arm baseline: the fx server connected but fx_alpha and fx_beta never reached the request.",
        APPARATUS_FAILURE,
    ),
)

PROBE = OpenCodeFakeProbe(
    marker="AGENTSPEC-PROBE-MARKER-OCDENY7",
    arms=(
        OpenCodeArm("baseline", "probe-baseline"),
        OpenCodeArm("allow_one", "probe-allow-one"),
        OpenCodeArm("reversed", "probe-reversed"),
    ),
    fx_servers=("fx",),
    custom_checks=(_BASELINE_OFFERS_FX,),
)
