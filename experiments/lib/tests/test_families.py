"""The shared pipeline in `families/__init__.py`: retention, replay, and definition refusals.

A workspace is kept when a run fails after creating it. Definitions that could only fail
later are refused before any workspace exists: for the billed Claude family each would
otherwise surface only after every arm had spent a model call, so each such test also
asserts that no workspace was created.
"""

import json
import os
import textwrap
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from tests.conftest import (
    Cli,
    unattended_manifest,
    workspaces,
)

KEPT = "probe run: the workspace has been kept for inspection:"


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


RECORDING_PY = """\
from pathlib import Path
from probe_harness.families.base import CustomProbe, JudgeResult

def _act(ws: Path) -> None:
    (ws / "view.json").write_text("1\\n", encoding="utf-8")

PROBE = CustomProbe(required_tools=(), arrange=lambda ws, variant: None, act=_act,
                    judge=lambda ws: JudgeResult(view=ws / "view.json", selection=None))
"""


def test_a_record_lands_in_the_package_s_results_when_the_tree_is_reached_through_a_symlink(
    cli: Cli, tree: Path, tmp_path: Path
) -> None:
    _put_custom_package(tree, "alpha", RECORDING_PY)
    link = tmp_path / "link"
    link.symlink_to(tree)

    result = cli("run", "alpha", env={"PROBE_EXPERIMENTS_DIR": str(link)})
    assert result.returncode == 0, result.output
    records = list((tree / "alpha" / "results").glob("*.json"))
    assert len(records) == 1
    assert result.stdout == f"{link / 'alpha' / 'results' / records[0].name}\n"


def test_a_real_record_keeps_its_workspace_with_the_source_map(cli: Cli, tree: Path) -> None:
    _put_custom_package(tree, "alpha", RECORDING_PY)

    result = cli("run", "alpha")

    assert result.returncode == 0, result.output
    (ws,) = workspaces("alpha")
    assert f"probe run: the workspace is kept at {ws}\n" in result.stderr
    source = json.loads((ws / "source.json").read_text(encoding="utf-8"))
    assert "experiments/alpha/probe.py" in source
    assert "experiments/lib/probe_harness/source.py" in source


# The version stub prints the file named by `VERSION_FILE`; Act rewrites that file, as a
# provider updating during a run would.
UPGRADING_PY = """\
import os
from pathlib import Path
from probe_harness.families.base import CustomProbe, JudgeResult

def _act(ws: Path) -> None:
    Path(os.environ["VERSION_FILE"]).write_text("2.0.0\\n", encoding="utf-8")
    (ws / "view.json").write_text("1\\n", encoding="utf-8")

PROBE = CustomProbe(required_tools=(), arrange=lambda ws, variant: None, act=_act,
                    judge=lambda ws: JudgeResult(view=ws / "view.json", selection=None))
"""


def _version_stub(tmp_path: Path, version: str) -> tuple[Path, Path]:
    """A version command printing the content of a file holding <version>: `(stub, file)`."""
    version_file = tmp_path / "version.txt"
    version_file.write_text(f"{version}\n" if version else "", encoding="utf-8")
    stub = tmp_path / "version-stub"
    stub.write_text(f'#!/bin/sh\ncat "{version_file}"\n', encoding="utf-8")
    stub.chmod(0o755)
    return stub, version_file


def test_a_record_carries_the_version_read_before_act_not_after(
    cli: Cli, tree: Path, tmp_path: Path
) -> None:
    stub, version_file = _version_stub(tmp_path, "1.0.0")
    package = _put_custom_package(
        tree, "alpha", UPGRADING_PY, version_source={"kind": "command", "command": str(stub)}
    )

    result = cli("run", "alpha", env={"VERSION_FILE": str(version_file)})

    assert result.returncode == 0, result.output
    assert version_file.read_text(encoding="utf-8") == "2.0.0\n"
    (record,) = (package / "results").glob("*.json")
    assert json.loads(record.read_text(encoding="utf-8"))["tool_version"] == "1.0.0"
    (ws,) = workspaces("alpha")
    assert (ws / "tool_version").read_text(encoding="utf-8") == "1.0.0\n"


def test_a_version_command_printing_nothing_refuses_the_run_before_act(
    cli: Cli, tree: Path, tmp_path: Path
) -> None:
    stub, version_file = _version_stub(tmp_path, "")
    package = _put_custom_package(
        tree, "alpha", UPGRADING_PY, version_source={"kind": "command", "command": str(stub)}
    )

    result = cli("run", "alpha", env={"VERSION_FILE": str(version_file)})

    assert result.returncode == 1, result.output
    assert f"probe run: the version command printed nothing within 10s: {stub}\n" in result.stderr
    assert version_file.read_text(encoding="utf-8") == ""  # Act never ran
    assert not (package / "results").exists()
    (ws,) = workspaces("alpha")
    assert not (ws / "tool_version").exists()
    assert f"probe run: the workspace has been kept for inspection: {ws}\n" in result.stderr


def test_a_dry_run_names_the_workspace_it_keeps(cli: Cli, tree: Path) -> None:
    _put_custom_package(tree, "alpha", RECORDING_PY)

    result = cli("run", "alpha", env={"PROBE_DRY_RUN": "1"})

    assert result.returncode == 0, result.output
    (ws,) = workspaces("alpha")
    assert f"probe run: the workspace is kept at {ws}\n" in result.stderr


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


def _put_custom_package(tree: Path, name: str, probe_py: str, **manifest: Any) -> Path:
    package = tree / name
    package.mkdir()
    (package / "probe.json").write_text(
        json.dumps(unattended_manifest(**manifest)), encoding="utf-8"
    )
    (package / "probe.py").write_text(probe_py, encoding="utf-8")
    return package


def test_a_custom_probe_s_arrange_receives_the_selected_variant(cli: Cli, tree: Path) -> None:
    _put_custom_package(tree, "alpha", VARIANT_ECHO_PY)
    result = cli("run", "alpha", env={"PROBE_FIXTURE": "discriminator", "PROBE_DRY_RUN": "1"})
    assert result.returncode == 0
    assert "arranged discriminator" in result.stdout


def test_a_custom_probe_over_a_manual_manifest_is_refused_before_a_workspace_exists(
    cli: Cli, tree: Path
) -> None:
    _put_custom_package(tree, "alpha", VARIANT_ECHO_PY, driver="manual", wait_for="length > 0")
    result = cli("run", "alpha")
    assert result.returncode == 1
    assert 'records without a capture, but its manifest declares driver "manual"' in result.stderr
    assert "arranged" not in result.stdout
    assert workspaces("alpha") == []


def test_a_custom_probe_over_a_capture_version_source_is_refused(cli: Cli, tree: Path) -> None:
    _put_custom_package(
        tree, "alpha", VARIANT_ECHO_PY, version_source={"kind": "capture", "jq": "first"}
    )
    result = cli("run", "alpha")
    assert result.returncode == 1
    assert "declares a capture version source" in result.stderr
    assert workspaces("alpha") == []


def test_a_non_object_version_source_is_refused_before_a_workspace_exists(
    cli: Cli, tree: Path
) -> None:
    package = _put_custom_package(tree, "alpha", VARIANT_ECHO_PY, version_source="command")
    result = cli("run", "alpha")
    assert result.returncode == 1
    assert f"probe run: manifest is invalid: {package / 'probe.json'}\n" in result.stderr
    assert "probe run:   version_source: must be an object" in result.stderr
    assert "arranged" not in result.stdout
    assert workspaces("alpha") == []


def test_a_manual_probe_declaring_a_second_variant_is_refused(cli: Cli, tree: Path) -> None:
    _put_custom_package(
        tree, "alpha", MANUAL_WITH_VARIANT_PY, driver="manual", wait_for="length > 0"
    )
    capture = tree / "alpha" / "fixtures" / "capture"
    capture.mkdir(parents=True)
    (capture / "dump-hook.sh").write_text("#!/bin/sh\n", encoding="utf-8")
    result = cli("run", "alpha")
    assert result.returncode == 1
    assert "whose only fixture variant is assertion" in result.stderr
    assert "Workspace ready" not in result.output
    assert workspaces("alpha") == []


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


def _put_definition_package(tree: Path, probe_py: str, **manifest: Any) -> None:
    package = tree / "pkg"
    package.mkdir()
    (package / "probe.json").write_text(json.dumps({**MANIFEST, **manifest}), encoding="utf-8")
    (package / "probe.py").write_text(probe_py, encoding="utf-8")


def _claude(arms: str, gates: str = "()", variants: str = '("assertion",)') -> str:
    return CLAUDE.replace("ARMS", arms).replace("GATES", gates).replace("VARIANTS", variants)


ONE_ARM = '(ClaudeArm("a", "tree", "p"),)'


@pytest.fixture
def providers(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Stand-ins for the provider CLIs, so the families' tool checks pass on any machine."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    for tool in ("claude", "opencode"):
        stand_in = bin_dir / tool
        stand_in.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        stand_in.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")


@pytest.mark.usefixtures("providers")
@pytest.mark.parametrize(
    ("probe_py", "manifest", "message"),
    [
        pytest.param(
            _claude('(ClaudeArm("a", "tree", "p"), ClaudeArm("a", "other", "p"))'),
            {},
            "probe run: pkg's definition declares arm a more than once",
            id="duplicate-arm",
        ),
        pytest.param(
            _claude(ONE_ARM, '(Model(), ToolAnswered("b", "t"))'),
            {},
            "probe run: pkg's definition gates arm b (ToolAnswered), but declares no such arm",
            id="undeclared-gate-arm",
        ),
        pytest.param(
            _claude(ONE_ARM, '(DelegationFork("z"),)'),
            {},
            "probe run: pkg's definition gates arm z (DelegationFork), but declares no such arm",
            id="undeclared-fork-arm",
        ),
        pytest.param(
            _claude(ONE_ARM, variants='("assertion", "discriminator")'),
            {},
            "probe run: pkg declares variants assertion, discriminator, but no arm's fixture tree "
            "contains {variant}",
            id="variant-without-tree",
        ),
        pytest.param(
            _claude(ONE_ARM),
            {"driver": "manual", "wait_for": "true"},
            "probe run: pkg is a ClaudeOtelProbe, which records without a capture, but its "
            'manifest declares driver "manual"',
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
            "probe run: pkg is an OpenCodeFakeProbe, whose only fixture variant is assertion",
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
            "probe run: pkg's definition declares arm a more than once",
            id="fake-duplicate-arm",
        ),
    ],
)
def test_an_inconsistent_definition_is_refused_before_a_workspace_exists(
    cli: Cli,
    tree: Path,
    probe_py: str,
    manifest: dict[str, Any],
    message: str,
) -> None:
    _put_definition_package(tree, probe_py, **manifest)
    result = cli("run", "pkg")
    assert result.returncode == 1
    assert message in result.stderr
    assert workspaces("pkg") == []


def test_a_flag_given_with_a_package_exits_2(cli: Cli, put_package: Callable[..., Path]) -> None:
    put_package("alpha", "unattended", 0)
    result = cli("run", "alpha", "--billed")
    assert result.returncode == 2
    assert "probe run: error: unexpected argument: --billed; set PROBE_FIXTURE" in result.stderr
    assert "ran alpha" not in result.stdout


def test_replay_without_a_workspace_exits_2(cli: Cli, put_package: Callable[..., Path]) -> None:
    put_package("alpha", "unattended", 0)
    result = cli("replay", "alpha")
    assert result.returncode == 2
    assert "probe replay: error: the following arguments are required: workspace" in (result.stderr)
