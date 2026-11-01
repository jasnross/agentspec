"""The isolated layout and environment `opencode_env.py` gives OpenCode.

Observed through the debug family, whose stub `opencode` reports the `$HOME` and the
environment it was run with.
"""

import json
from pathlib import Path

from probe_harness.families.opencode_env import ancestor_instruction_files

from tests.conftest import (
    AGENT_VIEW,
    Cli,
    copy_package,
    oracle_file,
)


def test_an_isolated_definition_runs_the_oracle_with_home_inside_the_workspace(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    # `opencode-agent-variant` runs isolated.
    copy_package(experiments_dir, tree, "opencode-agent-variant")
    home_file = tmp_path / "home.txt"
    result = cli(
        "run",
        "opencode-agent-variant",
        env={
            "STUB_ORACLE": oracle_file(tmp_path, json.dumps(AGENT_VIEW)),
            "STUB_HOME_FILE": str(home_file),
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    home = Path(home_file.read_text(encoding="utf-8"))
    assert home.name == "home"
    ws = home.parent
    assert ws.parent.name == "opencode-agent-variant"
    assert (ws / "project" / ".opencode").is_dir()
    assert (ws / "xdg" / "config").is_dir()


def test_an_isolated_oracle_sees_no_operator_config_and_its_own_directories(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    # `opencode-agent-variant` runs isolated.
    copy_package(experiments_dir, tree, "opencode-agent-variant")
    env_file = tmp_path / "env.txt"
    operator = {
        name: "operator"
        for name in (
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_DIR",
            "OPENCODE_CONFIG_CONTENT",
            "OPENCODE_PERMISSION",
            "OPENCODE_DISABLE_PROJECT_CONFIG",
        )
    }
    result = cli(
        "run",
        "opencode-agent-variant",
        env={
            **operator,
            "STUB_ORACLE": oracle_file(tmp_path, json.dumps(AGENT_VIEW)),
            "STUB_ENV_FILE": str(env_file),
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    seen = dict(
        line.split("=", 1)
        for line in env_file.read_text(encoding="utf-8").splitlines()
        if "=" in line
    )
    assert not set(operator) & set(seen)
    ws = Path(seen["HOME"]).parent
    for kind in ("config", "cache", "data", "state"):
        assert seen[f"XDG_{kind.upper()}_HOME"] == str(ws / "xdg" / kind)
    assert seen["OPENCODE_DISABLE_AUTOUPDATE"] == "1"
    assert seen["OPENCODE_DISABLE_MODELS_FETCH"] == "1"
    assert seen["OPENCODE_DISABLE_EXTERNAL_SKILLS"] == "1"


def test_ancestor_instruction_files_finds_a_file_two_levels_up(tmp_path: Path) -> None:
    (tmp_path / "AGENTS.md").write_text("x\n", encoding="utf-8")
    root = tmp_path / "state" / "probe-workspaces"
    root.mkdir(parents=True)
    assert ancestor_instruction_files(root) == [tmp_path / "AGENTS.md"]


def test_ancestor_instruction_files_finds_nothing_in_a_clean_tree(tmp_path: Path) -> None:
    # The root need not exist: the check runs before any workspace is made.
    assert ancestor_instruction_files(tmp_path / "state" / "probe-workspaces") == []
