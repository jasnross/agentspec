"""Does OpenCode read a top-level `variant:` key in agent frontmatter?

Fully script-driven: no human step, no network, no credentials. `opencode debug agent
<name>` is name-addressed and exits nonzero when the agent is missing, so this probe needs
no resolution check of its own.
"""

from probe_harness.families.opencode_debug import OpenCodeDebugProbe

PROBE = OpenCodeDebugProbe(argv=("agent", "probe"), variants=("assertion", "discriminator"))
