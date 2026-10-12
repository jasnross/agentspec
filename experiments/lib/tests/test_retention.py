"""Workspace retention and the failure paths around it, beyond what the bats suite pinned."""

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

from probe_harness import jq_cli

from tests.conftest import Cli

KEPT = "probe: the workspace has been kept for inspection:"


def _manifest(**overrides: Any) -> dict[str, Any]:
    manifest: dict[str, Any] = {
        "schema_version": 1,
        "provider": "opencode",
        "driver": "unattended",
        "depth": None,
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"projection": ".", "expected": 1},
    }
    manifest.update(overrides)
    return manifest


def test_a_runner_exiting_nonzero_after_the_workspace_exists_keeps_it(
    cli: Cli, put_package: Callable[..., Path], tree: Path
) -> None:
    put_package("alpha", "unattended", 3)
    result = cli("run", "alpha")
    assert result.returncode == 3
    assert KEPT in result.stderr
    kept = result.stderr.split(KEPT, 1)[1].strip()
    assert Path(kept).is_dir()


def test_a_runner_exiting_zero_prints_no_kept_line(
    cli: Cli, put_package: Callable[..., Path], tree: Path
) -> None:
    put_package("alpha", "unattended", 0)
    result = cli("run", "alpha")
    assert result.returncode == 0
    assert KEPT not in result.output


def test_replay_refuses_a_selection_for_a_projection_manifest(
    cli: Cli, put_package: Callable[..., Path], tree: Path, tmp_path: Path
) -> None:
    put_package("alpha", "unattended", 0)
    ws = tmp_path / "ws"
    ws.mkdir()
    result = cli("replay", "alpha", ws, "--selection", "x")
    assert result.returncode == 2
    assert "declares a projection; --selection does not apply" in result.stderr


def test_a_non_object_version_source_fails_with_jq_s_diagnostic(cli: Cli, tmp_path: Path) -> None:
    package = tmp_path / "pkg"
    package.mkdir()
    manifest = package / "probe.json"
    manifest.write_text(json.dumps(_manifest(version_source="command")), encoding="utf-8")
    view = tmp_path / "view.json"
    view.write_text("1\n", encoding="utf-8")
    result = cli("record", "--manifest", manifest, "--view", view, "--dry-run")
    assert result.returncode == 5
    assert "Cannot index string" in result.stderr
    assert result.stdout == ""


def test_compile_check_accepts_an_expression_ending_in_a_comment() -> None:
    assert jq_cli.compile_error(". # trailing comment") is None


def test_a_relative_manifest_path_records_under_the_logical_working_directory(
    cli: Cli, tmp_path: Path
) -> None:
    real = tmp_path / "real"
    (real / "pkg").mkdir(parents=True)
    (real / "pkg" / "probe.json").write_text(json.dumps(_manifest()), encoding="utf-8")
    (real / "view.json").write_text("1\n", encoding="utf-8")
    link = tmp_path / "link"
    link.symlink_to(real)
    result = cli(
        "record",
        "--manifest",
        "pkg/probe.json",
        "--view",
        "view.json",
        cwd=link,
        env={"PWD": str(link)},
    )
    assert result.returncode == 0
    assert result.stdout.startswith(f"{link}/pkg/results/")


VARIANT_ECHO_PY = """\
from pathlib import Path
from probe_harness.families.base import CustomProbe, JudgeResult

def _arrange(ws: Path, variant: str) -> None:
    print(f"arranged {variant}")

def _act(ws: Path) -> None:
    raise SystemExit(0)

PROBE = CustomProbe(required_tools=(), arrange=_arrange, act=_act,
                    judge=lambda ws: JudgeResult(view=ws / "view.json", selection=None),
                    variants=("assertion", "discriminator"))
"""

MANUAL_WITH_VARIANT_PY = """\
from probe_harness.families.manual import ManualProbe

PROBE = ManualProbe(instructions="do it", variants=("assertion", "discriminator"))
"""


def _put(tree: Path, name: str, probe_py: str, **manifest: Any) -> Path:
    package = tree / name
    package.mkdir()
    (package / "probe.json").write_text(json.dumps(_manifest(**manifest)), encoding="utf-8")
    (package / "probe.py").write_text(probe_py, encoding="utf-8")
    return package


def _workspaces(tmpdir: Path, name: str) -> list[Path]:
    return list(tmpdir.glob(f"probe-{name}.*"))


def test_a_custom_probe_s_arrange_receives_the_selected_variant(cli: Cli, tree: Path) -> None:
    _put(tree, "alpha", VARIANT_ECHO_PY)
    result = cli("run", "alpha", env={"PROBE_FIXTURE": "discriminator", "PROBE_DRY_RUN": "1"})
    assert result.returncode == 0
    assert "arranged discriminator" in result.stdout


def test_a_custom_probe_over_a_manual_manifest_is_refused_before_a_workspace_exists(
    cli: Cli, tree: Path, _isolated_tmpdir: Path
) -> None:
    _put(tree, "alpha", VARIANT_ECHO_PY, driver="manual", wait_for="length > 0")
    result = cli("run", "alpha")
    assert result.returncode == 1
    assert 'records without a capture, but its manifest declares driver "manual"' in result.stderr
    assert "arranged" not in result.stdout
    assert _workspaces(_isolated_tmpdir, "alpha") == []


def test_a_custom_probe_over_a_capture_version_source_is_refused(
    cli: Cli, tree: Path, _isolated_tmpdir: Path
) -> None:
    _put(tree, "alpha", VARIANT_ECHO_PY, version_source={"kind": "capture", "jq": "first"})
    result = cli("run", "alpha")
    assert result.returncode == 1
    assert "declares a capture version source" in result.stderr
    assert _workspaces(_isolated_tmpdir, "alpha") == []


def test_a_manual_probe_declaring_a_second_variant_is_refused(
    cli: Cli, tree: Path, _isolated_tmpdir: Path
) -> None:
    _put(tree, "alpha", MANUAL_WITH_VARIANT_PY, driver="manual", wait_for="length > 0")
    capture = tree / "alpha" / "fixtures" / "capture"
    capture.mkdir(parents=True)
    (capture / "dump-hook.sh").write_text("#!/bin/sh\n", encoding="utf-8")
    result = cli("run", "alpha")
    assert result.returncode == 1
    assert "whose only fixture variant is assertion" in result.stderr
    assert "Workspace ready" not in result.output
    assert _workspaces(_isolated_tmpdir, "alpha") == []
