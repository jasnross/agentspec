"""Is an OpenCode agent with no `permission` map offered every tool of a
connected MCP server, where an agent whose map is only `"*": "deny"` (with
`external_directory` and `doom_loop` restated) is offered none?

Fully script-driven: no human step, no credentials, and no model quota.
OpenCode talks to a fake OpenAI-compatible provider on loopback, which writes
every request body to disk. That dump is the oracle, so the measurement is at
`outbound-request` depth rather than the `resolved-config` depth of
`opencode debug agent`.
"""

from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe

PROBE = OpenCodeFakeProbe(
    marker="AGENTSPEC-PROBE-MARKER-OCMCPU1",
    arms=(
        OpenCodeArm("no_map", "probe-no-map"),
        OpenCodeArm("deny_all", "probe-deny-all"),
    ),
    fx_servers=("fx",),
)
