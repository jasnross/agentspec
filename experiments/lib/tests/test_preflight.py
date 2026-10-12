"""Preflight refuses before a workspace exists, so nothing knowable early surfaces late."""

import json
import os
import textwrap
from pathlib import Path
from typing import Any

from tests.conftest import STUB_PROBE_PY, Cli

# A stub whose definition declares a second, non-`assertion` variant.
VARIANT_PROBE_PY = textwrap.dedent("""\
    import sys
    from pathlib import Path
    from probe_harness.families.base import CustomProbe, JudgeResult

    def _act(ws: Path) -> None:
        print("ran {name}")
        sys.exit(0)

    PROBE = CustomProbe(required_tools=(), arrange=lambda ws, variant: None, act=_act,
                        judge=lambda ws: JudgeResult(view=ws / "view.json", selection=None),
                        variants=("assertion", "discriminator"))
""")


def _put_custom(tree: Path, name: str, *, projection: str = ".", probe_py: str = "") -> Path:
    package = tree / name
    package.mkdir()
    manifest: dict[str, Any] = {
        "schema_version": 1,
        "provider": "opencode",
        "driver": "unattended",
        "depth": None,
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"projection": projection, "expected": 1},
    }
    (package / "probe.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    source = probe_py or STUB_PROBE_PY
    (package / "probe.py").write_text(source.format(name=name, code=0), encoding="utf-8")
    return package


def _workspaces(name: str) -> list[Path]:
    return sorted(Path(os.environ["TMPDIR"]).glob(f"probe-{name}.*"))


def test_a_non_compiling_projection_exits_1_before_any_workspace(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub", projection=".[")

    result = cli("run", "stub")

    assert result.returncode == 1, result.output
    assert "projection does not compile" in result.output
    assert "ran stub" not in result.output
    assert _workspaces("stub") == []


def test_probe_dry_run_outside_0_and_1_exits_2(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub")

    result = cli("run", "stub", env={"PROBE_DRY_RUN": "2"})

    assert result.returncode == 2, result.output
    assert "ran stub" not in result.output
    assert _workspaces("stub") == []


def test_an_undeclared_fixture_variant_exits_2(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub")

    result = cli("run", "stub", env={"PROBE_FIXTURE": "discriminator"})

    assert result.returncode == 2, result.output
    assert "ran stub" not in result.output
    assert _workspaces("stub") == []


def test_a_declared_non_assertion_variant_is_dry_run_only(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub", probe_py=VARIANT_PROBE_PY)

    result = cli("run", "stub", env={"PROBE_FIXTURE": "discriminator"})

    assert result.returncode == 2, result.output
    assert "the discriminator fixture is dry-run only; set PROBE_DRY_RUN=1" in result.output
    assert "ran stub" not in result.output
    assert _workspaces("stub") == []


def test_a_zero_timeout_exits_2(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub")

    result = cli("run", "stub", env={"PROBE_TIMEOUT_SECONDS": "0"})

    assert result.returncode == 2, result.output
    assert "ran stub" not in result.output
    assert _workspaces("stub") == []


def test_an_extra_argument_exits_2(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub")

    result = cli("run", "stub", "extra")

    assert result.returncode == 2, result.output
    assert "unexpected argument:" in result.output
    assert "ran stub" not in result.output
