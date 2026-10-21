"""Preflight refuses before a workspace exists, so nothing knowable early surfaces late."""

import json
import os
import shutil
import subprocess
import sys
import textwrap
import time
from pathlib import Path
from typing import Any

import pytest
from probe_harness import preflight
from probe_harness.workspace import (
    POLL_INTERVAL_SECONDS,
    wait_for_capture,
)

from tests.conftest import (
    EXPERIMENTS_DIR,
    STUB_PROBE_PY,
    Cli,
    make_capture_workspace,
    workspaces,
)

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


def _put_custom(
    tree: Path,
    name: str,
    *,
    projection: str = ".",
    probe_py: str = "",
    version_source: dict[str, Any] | None = None,
) -> Path:
    package = tree / name
    package.mkdir()
    manifest: dict[str, Any] = {
        "schema_version": 1,
        "provider": "opencode",
        "driver": "unattended",
        "depth": None,
        "question": "q",
        "version_source": version_source or {"kind": "none"},
        "assertion": {"projection": projection, "expected": 1},
    }
    (package / "probe.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    source = probe_py or STUB_PROBE_PY
    (package / "probe.py").write_text(source.format(name=name, code=0), encoding="utf-8")
    return package


def test_a_non_compiling_projection_exits_1_before_any_workspace(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub", projection=".[")

    result = cli("run", "stub")

    assert result.returncode == 1, result.output
    assert "projection does not compile" in result.output
    assert "ran stub" not in result.output
    assert workspaces("stub") == []


def test_probe_dry_run_outside_0_and_1_exits_2(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub")

    result = cli("run", "stub", env={"PROBE_DRY_RUN": "2"})

    assert result.returncode == 2, result.output
    assert "ran stub" not in result.output
    assert workspaces("stub") == []


def test_an_undeclared_fixture_variant_exits_2(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub")

    result = cli("run", "stub", env={"PROBE_FIXTURE": "discriminator"})

    assert result.returncode == 2, result.output
    assert "ran stub" not in result.output
    assert workspaces("stub") == []


def test_a_declared_non_assertion_variant_is_dry_run_only(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub", probe_py=VARIANT_PROBE_PY)

    result = cli("run", "stub", env={"PROBE_FIXTURE": "discriminator"})

    assert result.returncode == 2, result.output
    assert "the discriminator fixture is dry-run only; set PROBE_DRY_RUN=1" in result.output
    assert "ran stub" not in result.output
    assert workspaces("stub") == []


def test_a_zero_timeout_exits_2(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub")

    result = cli("run", "stub", env={"PROBE_TIMEOUT_SECONDS": "0"})

    assert result.returncode == 2, result.output
    assert "ran stub" not in result.output
    assert workspaces("stub") == []


def test_an_extra_argument_exits_2(cli: Cli, tree: Path) -> None:
    _put_custom(tree, "stub")

    result = cli("run", "stub", "extra")

    assert result.returncode == 2, result.output
    assert "unexpected argument:" in result.output
    assert "ran stub" not in result.output


def test_probe_timeout_seconds_overrides_the_caller_s_timeout(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ws = make_capture_workspace(tmp_path / "ws-override", '{"a":1}\n')
    monkeypatch.setenv("PROBE_TIMEOUT_SECONDS", "1")

    options = preflight.run_options(("assertion",), {})
    assert options.timeout == 1

    started = time.monotonic()
    assert wait_for_capture(ws, "any(.[]; .never == true)", options.timeout) is False
    assert time.monotonic() - started < options.timeout + POLL_INTERVAL_SECONDS


def test_a_version_command_whose_tool_is_not_on_path_is_refused_before_a_workspace(
    cli: Cli, tree: Path
) -> None:
    command = {"kind": "command", "command": "no-such-provider-cli --version"}
    _put_custom(tree, "stub", version_source=command)

    result = cli("run", "stub")

    assert result.returncode == 1, result.output
    assert "probe run: missing required tool(s): no-such-provider-cli\n" in result.stderr
    assert "ran stub" not in result.output
    assert workspaces("stub") == []


def test_a_missing_git_is_refused_before_a_workspace(tree: Path, tmp_path: Path) -> None:
    _put_custom(tree, "stub")
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    jq = shutil.which("jq")
    assert jq is not None
    (bin_dir / "jq").symlink_to(jq)

    completed = subprocess.run(
        [sys.executable, str(EXPERIMENTS_DIR / "probe"), "run", "stub"],
        env={**os.environ, "PATH": str(bin_dir)},
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        check=False,
    )

    assert completed.returncode == 1, completed.stderr
    assert "probe run: missing required tool(s): git\n" in completed.stderr
    assert "ran stub" not in completed.stdout
    assert workspaces("stub") == []
