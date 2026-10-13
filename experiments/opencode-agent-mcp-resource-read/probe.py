"""Under an OpenCode agent map that leads with `"*": "deny"` and sets `read` to
`{"*": "allow", "mcp:*": "deny"}`, which tools is the agent offered, and does
each MCP resource call fail on the rule while a project-file read still
succeeds?

Fully script-driven: no human step, no credentials, and no model quota.
OpenCode talks to a fake OpenAI-compatible provider on loopback, which scripts
one tool call per arm and writes every request body to disk. The tool result
OpenCode sends back in the follow-up request is the oracle, so the measurement
is at `outbound-request` depth.
"""

import json
from pathlib import Path

from probe_harness.families.base import GateFailure
from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe, ScriptedCall

_RESOURCE_CALL = ScriptedCall(
    "read_mcp_resource", '{"server":"fx","uri":"fx://AGENTSPEC-RESOURCE-OCRES3"}'
)
_LIST_RESOURCES = ScriptedCall("list_mcp_resources", '{"server":"fx"}')
_LIST_TEMPLATES = ScriptedCall("list_mcp_resource_templates", '{"server":"fx"}')


def _arrange_arm(package: Path, arm_dir: Path, arm: OpenCodeArm) -> ScriptedCall | None:
    """Refuse a missing read target, and script the file read for the arm that makes one.

    A missing file would fail the `read` arm's call for a reason that has nothing to do with
    permissions. The `read` arm's arguments are built here, from the physical path: on macOS
    `$TMPDIR` sits under `/var`, a symlink to `/private/var`, and OpenCode compares the path
    against its resolved working directory, so the unresolved spelling would read as outside
    the project and fail on `external_directory` instead.
    """
    notes = arm_dir / "project" / "notes.txt"
    if not notes.exists():
        raise GateFailure(f"arm {arm.name}: {notes} is missing, so a read of it could not succeed.")
    if arm.call is None or arm.call.tool != "read":
        return None
    path = f"{(arm_dir / 'project').resolve()}/notes.txt"
    return ScriptedCall(
        "read", json.dumps({"filePath": path}, separators=(",", ":"), ensure_ascii=False)
    )


PROBE = OpenCodeFakeProbe(
    marker="AGENTSPEC-PROBE-MARKER-OCRES3",
    arms=(
        OpenCodeArm("map_read_resource", "probe-pattern-map", _RESOURCE_CALL),
        OpenCodeArm("map_list_resources", "probe-pattern-map", _LIST_RESOURCES),
        OpenCodeArm("map_list_templates", "probe-pattern-map", _LIST_TEMPLATES),
        # The arguments are built per arm, from the resolved project path.
        OpenCodeArm("map_read_file", "probe-pattern-map", ScriptedCall("read", "")),
        OpenCodeArm("plain_read_resource", "probe-plain-read", _RESOURCE_CALL),
        OpenCodeArm("plain_list_resources", "probe-plain-read", _LIST_RESOURCES),
        OpenCodeArm("plain_list_templates", "probe-plain-read", _LIST_TEMPLATES),
    ),
    fx_servers=("fx",),
    tool_result_gate=True,
    arrange_arm=_arrange_arm,
)
