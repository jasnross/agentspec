"""Every committed package's definition loads as its family, and keeps its namespace private."""

import ast
import json
from pathlib import Path

import pytest
from probe_harness.families import DEFINITION_TYPES
from probe_harness.families.claude_otel import ClaudeOtelProbe
from probe_harness.families.manual import ManualProbe
from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.families.opencode_fake import OpenCodeFakeProbe
from probe_harness.loader import load_definition

from tests.conftest import EXPERIMENTS_DIR

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
