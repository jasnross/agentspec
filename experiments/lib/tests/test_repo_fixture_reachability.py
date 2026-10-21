"""Every fixture directory a package commits is reachable through its definition.

A fixture tree no code path reaches is evidence nobody can reproduce, and a discriminator
tree a run could record would commit a false `refuted`. So each entry directly under a
package's `fixtures/`, directory or file, must be a declared variant, or `capture` for a
manual probe. Inside a Claude probe's variant tree, which the family reads piecemeal rather
than copying whole, each entry must be an arm tree, an MCP config template, a plugin source,
or a name the definition lists in its module-level `FIXTURE_PATHS`; every such name must
exist in every variant tree.
"""

import shutil
import sys
from pathlib import Path

import pytest
from probe_harness.families import DEFINITION_TYPES, ProbeDefinition
from probe_harness.families.claude_otel import ClaudeOtelProbe, McpConfig, PluginDir
from probe_harness.families.manual import CAPTURE_DIR, ManualProbe
from probe_harness.loader import load_definition

from tests.conftest import (
    EXPERIMENTS_DIR,
)


def _packages() -> list[Path]:
    return sorted(manifest.parent for manifest in EXPERIMENTS_DIR.glob("*/probe.json"))


def _fixture_paths(package: Path) -> set[str]:
    module = sys.modules[f"probe_definition_{package.name.replace('-', '_')}"]
    return set(getattr(module, "FIXTURE_PATHS", ()))


def _top_level_roots(definition: ProbeDefinition) -> set[str]:
    roots = set(definition.variants)
    if isinstance(definition, ManualProbe):
        roots.add(CAPTURE_DIR)
    return roots


def _claude_variant_roots(package: Path, definition: ClaudeOtelProbe) -> set[str]:
    """The entries a Claude probe reads from inside each of its variant trees."""
    roots = {arm.tree for arm in definition.arms}
    roots |= {arm.server.source for arm in definition.arms if isinstance(arm.server, PluginDir)}
    roots |= {arm.server.template for arm in definition.arms if isinstance(arm.server, McpConfig)}
    return roots | _fixture_paths(package)


def _unreachable(package: Path) -> set[str]:
    """Every unreachable fixture entry, as a path relative to `fixtures/`."""
    definition = load_definition(package, DEFINITION_TYPES)
    fixtures = package / "fixtures"
    unreachable = {p.name for p in fixtures.iterdir()} - _top_level_roots(definition)
    if isinstance(definition, ClaudeOtelProbe):
        roots = _claude_variant_roots(package, definition)
        for variant in definition.variants:
            tree = fixtures / variant
            if tree.is_dir():
                entries = {p.name for p in tree.iterdir()}
                unreachable |= {f"{variant}/{name}" for name in entries - roots}
    return unreachable


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_every_fixture_directory_is_reachable(package: Path) -> None:
    unreachable = _unreachable(package)
    assert not unreachable, f"{package.name} commits unreachable fixtures: {sorted(unreachable)}"


def test_the_check_catches_an_orphaned_fixture(tmp_path: Path) -> None:
    package = tmp_path / "orphaned"
    shutil.copytree(EXPERIMENTS_DIR / "opencode-agent-variant", package)
    (package / "fixtures" / "orphan").mkdir()
    (package / "fixtures" / "stray.json").write_text("{}", encoding="utf-8")
    assert _unreachable(package) == {"orphan", "stray.json"}


def test_the_check_catches_an_orphan_inside_a_claude_variant_tree(tmp_path: Path) -> None:
    package = tmp_path / "orphaned_claude"
    shutil.copytree(EXPERIMENTS_DIR / "claude-subagent-mcp-tools", package)
    (package / "fixtures" / "orphan").mkdir()
    (package / "fixtures" / "assertion" / "orphan").mkdir()
    assert _unreachable(package) == {"orphan", "assertion/orphan"}


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_every_declared_variant_directory_exists(package: Path) -> None:
    definition = load_definition(package, DEFINITION_TYPES)
    for variant in definition.variants:
        assert (package / "fixtures" / variant).is_dir(), f"{package.name}: no fixtures/{variant}"


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_every_claude_arm_input_exists_under_every_variant(package: Path) -> None:
    definition = load_definition(package, DEFINITION_TYPES)
    if not isinstance(definition, ClaudeOtelProbe):
        return
    for variant in definition.variants:
        fixtures = package / "fixtures" / variant
        for arm in definition.arms:
            assert (fixtures / arm.tree).is_dir(), f"{package.name}: no {variant}/{arm.tree}"
            if isinstance(arm.server, McpConfig):
                template = arm.server.template
                assert (fixtures / template).is_file(), f"{package.name}: no {variant}/{template}"
            if isinstance(arm.server, PluginDir):
                source = arm.server.source
                assert (fixtures / source / ".mcp.json").is_file(), (
                    f"{package.name}: no {variant}/{source}/.mcp.json"
                )


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_every_declared_fixture_path_exists(package: Path) -> None:
    definition = load_definition(package, DEFINITION_TYPES)
    for variant in definition.variants:
        for name in _fixture_paths(package):
            path = package / "fixtures" / variant / name
            assert path.exists(), f"{package.name}: no fixtures/{variant}/{name}"
