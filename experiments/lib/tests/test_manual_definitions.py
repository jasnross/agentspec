"""The manual packages' definitions, loaded from the committed tree."""

import json
from pathlib import Path

import pytest
from probe_harness.families import DEFINITION_TYPES
from probe_harness.families.manual import ManualProbe
from probe_harness.loader import load_definition

from tests.conftest import EXPERIMENTS_DIR, Cli

# Every character `cursor-hook-command-argv` refuses in `$TMPDIR`: whitespace, quotes, and
# every character a shell acts on.
SHELL_SIGNIFICANT = " \t\n'\"$`;&|()<>*?!\\"


def _manual_packages() -> list[str]:
    return sorted(
        manifest.parent.name
        for manifest in EXPERIMENTS_DIR.glob("*/probe.json")
        if json.loads(manifest.read_text(encoding="utf-8")).get("driver") == "manual"
    )


def test_the_tree_holds_manual_packages() -> None:
    # Guards the parametrized test below against collecting nothing.
    assert _manual_packages()


@pytest.mark.parametrize("name", _manual_packages())
def test_every_manual_package_loads_as_a_manual_probe(name: str) -> None:
    definition = load_definition(EXPERIMENTS_DIR / name, DEFINITION_TYPES)
    assert isinstance(definition, ManualProbe)
    assert definition.variants == ("assertion",)
    # The runner prints the instructions between blank lines, as the bash runners did.
    assert definition.instructions.startswith("\n")
    assert definition.instructions.endswith("\n")


@pytest.mark.parametrize("character", list(SHELL_SIGNIFICANT), ids=repr)
def test_hook_command_argv_refuses_a_shell_significant_tmpdir(
    cli: Cli, tmp_path: Path, character: str
) -> None:
    # Refused before any workspace exists, so the directory itself need not exist.
    tmpdir = f"{tmp_path}/a{character}b"
    result = cli(
        "run",
        "cursor-hook-command-argv",
        env={"TMPDIR": tmpdir, "PROBE_TIMEOUT_SECONDS": "1"},
    )
    assert result.returncode == 1
    assert result.stderr == (
        f"probe: TMPDIR contains whitespace or a shell metacharacter ({tmpdir});\n"
        "probe: the command strings this probe registers would not measure what they mean to.\n"
    )
    assert "Workspace ready" not in result.output


def test_hook_command_argv_accepts_a_plain_tmpdir(cli: Cli, tmp_path: Path) -> None:
    tmpdir = tmp_path / "plain"
    tmpdir.mkdir()
    result = cli(
        "run",
        "cursor-hook-command-argv",
        env={"TMPDIR": str(tmpdir), "PROBE_TIMEOUT_SECONDS": "1", "PROBE_DRY_RUN": "1"},
    )
    assert "TMPDIR contains" not in result.output
    assert "Workspace ready" in result.output
