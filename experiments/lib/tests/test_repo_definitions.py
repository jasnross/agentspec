"""Every committed package's definition, loaded from the tree.

Each loads as its family and keeps its namespace private. Its declared fixture variants
put `assertion` first; `test_repo_fixture_reachability.py` checks their trees. The manual
packages load as manual probes, and `cursor-hook-command-argv` refuses a workspace root its
hook commands could not carry.
"""

import ast
import json
import re
from pathlib import Path

import pytest
from probe_harness.families import DEFINITION_TYPES
from probe_harness.families.claude_otel import ClaudeOtelProbe
from probe_harness.families.manual import ManualProbe
from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.families.opencode_fake import OpenCodeFakeProbe
from probe_harness.loader import load_definition

from tests.conftest import EXPERIMENTS_DIR, Cli

# The only public names a definition may define: everything else is private.
PUBLIC_NAMES = {"PROBE", "FIXTURE_PATHS"}


DEBUG_PACKAGES = {
    "opencode-agent-variant",
    "opencode-command-variant",
    "opencode-skill-frontmatter-discard",
}


def _packages() -> list[Path]:
    return sorted(manifest.parent for manifest in EXPERIMENTS_DIR.glob("*/probe.json"))


def _expected_family(package: Path) -> type:
    """The family a package's manifest and name imply."""
    driver = json.loads((package / "probe.json").read_text(encoding="utf-8"))["driver"]
    if driver == "manual":
        return ManualProbe
    if driver == "billed":
        return ClaudeOtelProbe
    return OpenCodeDebugProbe if package.name in DEBUG_PACKAGES else OpenCodeFakeProbe


def test_the_tree_holds_all_twenty_seven_packages() -> None:
    assert len(_packages()) == 27


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_every_package_loads_as_its_family(package: Path) -> None:
    definition = load_definition(package, DEFINITION_TYPES)
    assert isinstance(definition, _expected_family(package))


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_no_definition_defines_a_public_name_beyond_probe_and_fixture_paths(
    package: Path,
) -> None:
    # The rule covers names a definition defines, not the names it imports.
    tree = ast.parse((package / "probe.py").read_text(encoding="utf-8"))
    defined: set[str] = set()
    for node in tree.body:
        if isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef | ast.ClassDef):
            defined.add(node.name)
        elif isinstance(node, ast.Assign):
            defined |= {t.id for t in node.targets if isinstance(t, ast.Name)}
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            defined.add(node.target.id)
    public = {name for name in defined if not name.startswith("_")}
    assert public <= PUBLIC_NAMES, f"{package.name} defines {sorted(public - PUBLIC_NAMES)}"
    assert "PROBE" in public


# Every character `cursor-hook-command-argv` refuses in the workspace root: whitespace,
# quotes, and every character a shell acts on.
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
    # The runner prints the instructions between blank lines, as the bash runners did.
    assert definition.instructions.startswith("\n")
    assert definition.instructions.endswith("\n")


@pytest.mark.parametrize("character", list(SHELL_SIGNIFICANT), ids=repr)
def test_hook_command_argv_refuses_a_shell_significant_workspace_root(
    cli: Cli, tmp_path: Path, character: str
) -> None:
    # Refused before any workspace exists, so the directory itself need not exist.
    state = f"{tmp_path}/a{character}b"
    root = f"{state}/agentspec/probe-workspaces"
    result = cli(
        "run",
        "cursor-hook-command-argv",
        env={"XDG_STATE_HOME": state, "PROBE_TIMEOUT_SECONDS": "1"},
    )
    assert result.returncode == 1
    assert result.stderr == (
        "probe run: the probe workspace root contains whitespace or a shell metacharacter "
        f"({root});\n"
        "probe run: the command strings this probe registers would not measure what they mean to.\n"
    )
    assert "Workspace ready" not in result.output


def test_hook_command_argv_accepts_a_plain_workspace_root(cli: Cli, tmp_path: Path) -> None:
    result = cli(
        "run",
        "cursor-hook-command-argv",
        env={
            "XDG_STATE_HOME": str(tmp_path / "plain"),
            "PROBE_TIMEOUT_SECONDS": "1",
            "PROBE_DRY_RUN": "1",
        },
    )
    assert "workspace root contains" not in result.output
    assert "Workspace ready" in result.output


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_every_definition_declares_assertion_first(package: Path) -> None:
    definition = load_definition(package, DEFINITION_TYPES)
    assert definition.variants[0] == "assertion"
    assert len(set(definition.variants)) == len(definition.variants)


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_every_marker_is_letters_digits_and_hyphens(package: Path) -> None:
    # The gates find a marker in a request's string values. For such a marker that agrees
    # with finding it in the field's JSON text, which is what custom checks' jq does.
    definition = load_definition(package, DEFINITION_TYPES)
    if isinstance(definition, ClaudeOtelProbe | OpenCodeFakeProbe):
        assert re.fullmatch(r"[A-Za-z0-9-]+", definition.marker), definition.marker
