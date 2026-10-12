"""Definitions that could only fail later are refused before any workspace exists.

For the billed Claude family each of these would otherwise surface only after every arm had
spent a model call, so each test also asserts that no workspace was created.
"""

import json
import os
import textwrap
from pathlib import Path
from typing import Any

import pytest

from tests.conftest import Cli

MANIFEST: dict[str, Any] = {
    "schema_version": 1,
    "provider": "claude",
    "driver": "billed",
    "depth": None,
    "question": "q",
    "version_source": {"kind": "none"},
    "assertion": {"projection": ".", "expected": 1},
}

CLAUDE = textwrap.dedent("""\
    from probe_harness.families.claude_otel import (
        ClaudeArm, ClaudeOtelProbe, DelegationFork, Model, ToolAnswered,
    )

    PROBE = ClaudeOtelProbe(
        model="m", budget_usd="0.10", marker="M",
        arms=ARMS,
        gates=GATES,
        variants=VARIANTS,
    )
""")


def _put(tree: Path, probe_py: str, **manifest: Any) -> None:
    package = tree / "pkg"
    package.mkdir()
    (package / "probe.json").write_text(json.dumps({**MANIFEST, **manifest}), encoding="utf-8")
    (package / "probe.py").write_text(probe_py, encoding="utf-8")


def _claude(arms: str, gates: str = "()", variants: str = '("assertion",)') -> str:
    return CLAUDE.replace("ARMS", arms).replace("GATES", gates).replace("VARIANTS", variants)


ONE_ARM = '(ClaudeArm("a", "tree", "p"),)'


@pytest.fixture(autouse=True)
def _providers(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Stand-ins for the provider CLIs, so the families' tool checks pass on any machine."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    for tool in ("claude", "opencode"):
        stand_in = bin_dir / tool
        stand_in.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        stand_in.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")


@pytest.mark.parametrize(
    ("probe_py", "manifest", "message"),
    [
        pytest.param(
            _claude('(ClaudeArm("a", "tree", "p"), ClaudeArm("a", "other", "p"))'),
            {},
            "probe: pkg's definition declares arm a more than once",
            id="duplicate-arm",
        ),
        pytest.param(
            _claude(ONE_ARM, '(Model(), ToolAnswered("b", "t"))'),
            {},
            "probe: pkg's definition gates arm b (ToolAnswered), but declares no such arm",
            id="undeclared-gate-arm",
        ),
        pytest.param(
            _claude(ONE_ARM, '(DelegationFork("z"),)'),
            {},
            "probe: pkg's definition gates arm z (DelegationFork), but declares no such arm",
            id="undeclared-fork-arm",
        ),
        pytest.param(
            _claude(ONE_ARM, variants='("assertion", "discriminator")'),
            {},
            "probe: pkg declares variants assertion, discriminator, but no arm's fixture tree "
            "contains {variant}",
            id="variant-without-tree",
        ),
        pytest.param(
            _claude(ONE_ARM),
            {"driver": "manual", "wait_for": "true"},
            "probe: pkg is a ClaudeOtelProbe, which records without a capture, but its manifest "
            'declares driver "manual"',
            id="claude-over-manual",
        ),
        pytest.param(
            textwrap.dedent("""\
                from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe

                PROBE = OpenCodeFakeProbe(
                    marker="M",
                    arms=(OpenCodeArm("a", "agent"),),
                    variants=("assertion", "discriminator"),
                )
            """),
            {"provider": "opencode", "driver": "unattended"},
            "probe: pkg is an OpenCodeFakeProbe, whose only fixture variant is assertion",
            id="fake-with-variants",
        ),
        pytest.param(
            textwrap.dedent("""\
                from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe

                PROBE = OpenCodeFakeProbe(
                    marker="M", arms=(OpenCodeArm("a", "x"), OpenCodeArm("a", "y"))
                )
            """),
            {"provider": "opencode", "driver": "unattended"},
            "probe: pkg's definition declares arm a more than once",
            id="fake-duplicate-arm",
        ),
    ],
)
def test_an_inconsistent_definition_is_refused_before_a_workspace_exists(
    cli: Cli,
    tree: Path,
    _isolated_tmpdir: Path,
    probe_py: str,
    manifest: dict[str, Any],
    message: str,
) -> None:
    _put(tree, probe_py, **manifest)
    result = cli("run", "pkg")
    assert result.returncode == 1
    assert message in result.stderr
    assert list(_isolated_tmpdir.glob("probe-pkg.*")) == []
