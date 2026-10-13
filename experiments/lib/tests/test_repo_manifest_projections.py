"""The committed Claude manifests' projections, run by `probe record` over fabricated views.

A projection decides the recorded verdict, so each is checked against a view that would
tempt it into confirming what the run never measured.
"""

import json
import re
from pathlib import Path

import pytest

from tests.conftest import (
    EXPERIMENTS_DIR,
    Cli,
    Result,
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
    EXPERIMENTS_DIR / "claude-agent-effort/fixtures/assertion/.claude/agents/probe-effort.md"
)


MARKER_SKILL = fixture_marker(
    EXPERIMENTS_DIR / "claude-skill-effort/fixtures/assertion/.claude/skills/probe-effort/SKILL.md"
)


MARKER_MCP_PATH = (
    EXPERIMENTS_DIR / "claude-subagent-mcp-tools/fixtures/inherit/.claude/agents/"
    "probe-mcp-inherit.md"
)


def record_dry_run(cli: Cli, manifest: Path, view: Path) -> Result:
    """`probe record --dry-run` against a committed manifest, which must write nothing."""
    results = manifest.parent / "results"
    before = sorted(results.iterdir()) if results.is_dir() else []
    result = cli("record", "--manifest", manifest, "--view", view, "--dry-run")
    after = sorted(results.iterdir()) if results.is_dir() else []
    assert before == after, "a dry run wrote into a committed results directory"
    return result


def test_the_committed_manifest_s_projection_does_not_confirm_an_unmeasured_arm(
    tmp_path: Path, cli: Cli
) -> None:
    # The gates make a *runner-produced* record safe, but `probe record --dry-run --view
    # <saved-view>` runs the projection with no gates at all — and that is the workflow
    # both READMEs point authors at for iterating a candidate expression. A projection
    # whose pass value can be reached by an arm that was never measured is therefore
    # reachable in normal use.
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

    result = record_dry_run(cli, manifest, view)
    assert result.returncode == 0
    assert '"status": "confirmed"' not in result.output


def test_the_committed_skill_manifest_s_projection_does_not_confirm_an_unmeasured_arm(
    tmp_path: Path, cli: Cli
) -> None:
    # The sibling of the check above, against the skill package's own manifest and its own
    # governed field. The gates make a *runner-produced* record safe, but `probe record
    # --dry-run --view <saved-view>` runs the projection with no gates at all — and that is
    # the workflow both READMEs point authors at.
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

    result = record_dry_run(cli, manifest, view)
    assert result.returncode == 0
    assert '"status": "confirmed"' not in result.output


def test_the_committed_skill_manifest_names_an_arm_that_captured_no_governed_request(
    tmp_path: Path, cli: Cli
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

    result = record_dry_run(cli, manifest, view)
    assert result.returncode == 0
    assert '"status": "confirmed"' not in result.output
    assert "arm-had-no-governed-request" in result.output


def test_the_committed_mcp_manifest_s_projection_reads_both_direct_and_deferred_tools(
    tmp_path: Path, cli: Cli
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

    result = record_dry_run(cli, manifest, view)
    assert result.returncode == 0
    observed = json.loads(result.stdout)["assertion"]["observed"]
    assert compact(observed["inherit"]) == (
        '{"direct":[],"deferred":["mcp__fx__alpha"],"tool_search":true}'
    )
    assert compact(observed["read"]) == (
        '{"direct":["mcp__fx__beta"],"deferred":[],"tool_search":false}'
    )


def test_the_committed_mcp_manifest_s_projection_does_not_confirm_an_unmeasured_arm(
    tmp_path: Path, cli: Cli
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

    result = record_dry_run(cli, manifest, view)
    assert result.returncode == 0
    assert '"status": "refuted"' in result.output
    assert json.loads(result.stdout)["assertion"]["observed"]["read"] == (
        "arm-had-no-governed-request"
    )
