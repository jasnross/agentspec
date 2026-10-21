"""The committed Claude manifests' projections, evaluated over fabricated views.

A projection decides the recorded verdict, so each is checked against a degenerate view —
one that would tempt it into confirming what the run never measured. The gates keep a real
run from producing such a view; these tests pin what the projection itself outputs if one
ever reaches it.
"""

import re
from pathlib import Path

import pytest
from probe_harness import jq_cli, json_values
from probe_harness.contract import ProjectionAssertion, parse_manifest
from probe_harness.json_values import JsonValue

from tests.conftest import (
    EXPERIMENTS_DIR,
    compact,
    deferred_request,
    direct_request,
    write_view,
)


def fixture_marker(path: Path) -> str:
    """The first line of <path> that is exactly a probe marker, or "" when there is none.

    Read out of the package's fixture rather than restated, so the two cannot drift.
    """
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return ""
    for line in text.split("\n"):
        if re.fullmatch(r"AGENTSPEC-PROBE-MARKER-[A-Z0-9]*", line):
            return line
    return ""


MARKER_AGENT = fixture_marker(
    EXPERIMENTS_DIR
    / "claude-agent-effort/fixtures/assertion/project/.claude/agents/probe-effort.md"
)


MARKER_SKILL = fixture_marker(
    EXPERIMENTS_DIR
    / "claude-skill-effort/fixtures/assertion/project/.claude/skills/probe-effort/SKILL.md"
)


MARKER_MCP_PATH = (
    EXPERIMENTS_DIR / "claude-subagent-mcp-tools/fixtures/assertion/inherit/.claude/agents/"
    "probe-mcp-inherit.md"
)


def project(manifest_path: Path, view: Path) -> tuple[JsonValue, bool]:
    """The committed manifest's projection over <view>, and whether it equals `expected`."""
    manifest = parse_manifest(manifest_path)
    assert isinstance(manifest.assertion, ProjectionAssertion)
    output = jq_cli.evaluate(manifest.assertion.projection, view, compact=True)
    values = json_values.load_stream(output)
    # As the recorder reads it: no value observes `null`, several are an error.
    assert len(values) <= 1, f"the projection yielded several values: {values}"
    observed = values[0] if values else None
    return observed, json_values.equal(observed, manifest.assertion.expected)


def test_the_committed_manifest_s_projection_does_not_confirm_an_unmeasured_arm(
    tmp_path: Path,
) -> None:
    # The gates keep a real run from producing an arm that was never measured, but the
    # projection must not depend on them: a pass value an unmeasured arm can reach is a
    # projection bug that only the gates are hiding.
    #
    # The degenerate shape: an arm with zero governed requests and an empty control set.
    # `unique` yields `[]` for both, and an unguarded `. == $u` collapses `[] == []` to the
    # same-as-ungoverned pass value.
    #
    # `rel` closes that on its **control-set** branch, not on the arm-empty one: once
    # `($u | length) != 1` has been ruled out, a single-element `$u` can never equal an
    # empty arm, so `arm-had-no-governed-request` is legibility rather than the guard. This
    # view reaches the control-set branch — see the skill package's sibling for one that
    # reaches the other.
    manifest = EXPERIMENTS_DIR / "claude-agent-effort/probe.json"
    if not manifest.is_file():
        pytest.skip("claude-agent-effort is not present")

    view = write_view(
        tmp_path / "degenerate.json",
        {
            "session_agent": [{"system": "plain"}],
            "delegated": [{"system": MARKER_AGENT, "output_config": {"effort": "low"}}],
        },
    )

    _, confirmed = project(manifest, view)
    assert not confirmed


def test_the_committed_skill_manifest_s_projection_does_not_confirm_an_unmeasured_arm(
    tmp_path: Path,
) -> None:
    # The sibling of the check above, against the skill package's own manifest and its own
    # governed field. Here too the projection must not lean on the gates to keep an
    # unmeasured arm from confirming.
    #
    # Which branch of `rel` does the work here is worth stating, because it is not the one
    # the shape suggests: the pass value `. == $u` is only reachable once
    # `($u | length) != 1` has been ruled out, so a single-element `$u` can never equal an
    # empty arm. The **control-set** branch is what makes an unmeasured arm unconfirmable;
    # `arm-had-no-governed-request` is legibility, and the next test is what pins it.
    manifest = EXPERIMENTS_DIR / "claude-skill-effort/probe.json"
    if not manifest.is_file():
        pytest.skip("claude-skill-effort is not present")
    # Guarded, not assumed: a moved fixture would leave the marker empty, the fabricated
    # view unmarked, and this test passing for the wrong reason.
    assert MARKER_SKILL

    governed = {
        "messages": [{"role": "user", "content": MARKER_SKILL}],
        "output_config": {"effort": "low"},
    }
    view = write_view(
        tmp_path / "degenerate-skill.json",
        {"inline": [{"system": "plain"}], "slash_entry": [governed], "fork": [governed]},
    )

    _, confirmed = project(manifest, view)
    assert not confirmed


def test_the_committed_skill_manifest_names_an_arm_that_captured_no_governed_request(
    tmp_path: Path,
) -> None:
    # The case the test above cannot reach: a control set that *is* single-valued, so `rel`
    # gets past its first branch, with one arm holding nothing governed. Deleting
    # `arm-had-no-governed-request` from the projection renders that arm as a bare `[]`
    # instead — still not confirmed, but no longer saying why. A reader iterating a
    # candidate expression against a saved view is the audience for the difference.
    manifest = EXPERIMENTS_DIR / "claude-skill-effort/probe.json"
    if not manifest.is_file():
        pytest.skip("claude-skill-effort is not present")
    assert MARKER_SKILL

    governed = {
        "messages": [{"role": "user", "content": MARKER_SKILL}],
        "output_config": {"effort": "low"},
    }
    view = write_view(
        tmp_path / "unmeasured-arm.json",
        {
            "inline": [
                {
                    "messages": [{"role": "user", "content": "plain"}],
                    "output_config": {"effort": "medium"},
                }
            ],
            "slash_entry": [governed],
            "fork": [governed],
        },
    )

    observed, confirmed = project(manifest, view)
    assert not confirmed
    assert "arm-had-no-governed-request" in compact(observed)


def test_the_committed_mcp_manifest_s_projection_reads_both_direct_and_deferred_tools(
    tmp_path: Path,
) -> None:
    # The two places an MCP tool can reach a request: deferred, named in a text block, or
    # loaded directly into `tools[]`. A projection reading only one would report the other
    # as an absent grant.
    manifest = EXPERIMENTS_DIR / "claude-subagent-mcp-tools/probe.json"
    if not manifest.is_file():
        pytest.skip("claude-subagent-mcp-tools is not present")
    marker = fixture_marker(MARKER_MCP_PATH)
    assert marker

    deferred = deferred_request("mcp__fx__alpha", marker=marker)
    direct = direct_request("mcp__fx__beta", marker=marker)
    view = write_view(
        tmp_path / "view.json",
        {
            "inherit": [deferred],
            "read": [direct],
            "exact": [deferred],
            "exact_search": [deferred],
            "server": [deferred],
            "server_glob": [deferred],
        },
    )

    observed, _ = project(manifest, view)
    assert isinstance(observed, dict)
    assert compact(observed["inherit"]) == (
        '{"direct":[],"deferred":["mcp__fx__alpha"],"tool_search":true}'
    )
    assert compact(observed["read"]) == (
        '{"direct":["mcp__fx__beta"],"deferred":[],"tool_search":false}'
    )


def test_the_committed_mcp_manifest_s_projection_does_not_confirm_an_unmeasured_arm(
    tmp_path: Path,
) -> None:
    # `read` expects no MCP tools and no ToolSearch — exactly what an arm with no governed
    # request would project to if the projection reduced it the same way. The sentinel
    # keeps an unmeasured `read` from confirming.
    manifest = EXPERIMENTS_DIR / "claude-subagent-mcp-tools/probe.json"
    if not manifest.is_file():
        pytest.skip("claude-subagent-mcp-tools is not present")
    marker = fixture_marker(MARKER_MCP_PATH)
    assert marker

    both = deferred_request("mcp__fx__alpha", "mcp__fx__beta", marker=marker)
    alpha = deferred_request("mcp__fx__alpha", marker=marker)
    view = write_view(
        tmp_path / "view.json",
        {
            "inherit": [both],
            "read": [{"model": "pinned-model", "system": "main thread", "tools": []}],
            "exact": [alpha],
            "exact_search": [alpha],
            "server": [both],
            "server_glob": [both],
        },
    )

    observed, confirmed = project(manifest, view)
    assert not confirmed
    assert isinstance(observed, dict)
    assert observed["read"] == "arm-had-no-governed-request"
