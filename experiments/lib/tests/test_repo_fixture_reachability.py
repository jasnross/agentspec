"""Every fixture directory a package commits is reachable through its definition.

A fixture tree no code path reaches is evidence nobody can reproduce, and a discriminator
tree a run could record would commit a false `refuted`. So each entry directly under a
package's `fixtures/`, directory or file, must be a declared root: a manual probe's whole
tree, a debug probe's variants, a fake-provider probe's `project`, a Claude probe's arm
trees (with every variant substituted), MCP config templates and plugin source, plus any
name a definition lists in its module-level `FIXTURE_PATHS`, each of which must exist.
"""

import shutil
import sys
from pathlib import Path
from typing import assert_never

import pytest
from probe_harness.families import DEFINITION_TYPES, ProbeDefinition
from probe_harness.families.base import CustomProbe
from probe_harness.families.claude_otel import VARIANT, ClaudeOtelProbe, McpConfig, PluginDir
from probe_harness.families.manual import ManualProbe
from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.families.opencode_fake import OpenCodeFakeProbe
from probe_harness.loader import load_definition

from tests.conftest import (
    EXPERIMENTS_DIR,
)


def _packages() -> list[Path]:
    return sorted(manifest.parent for manifest in EXPERIMENTS_DIR.glob("*/probe.json"))


def _declared_roots(package: Path, definition: ProbeDefinition) -> set[str] | None:
    """The fixture roots a definition reaches, or None when it reaches the whole tree."""
    module = sys.modules[f"probe_definition_{package.name.replace('-', '_')}"]
    extra = set(getattr(module, "FIXTURE_PATHS", ()))
    match definition:
        case ManualProbe():
            return None
        case OpenCodeDebugProbe():
            return set(definition.variants) | extra
        case OpenCodeFakeProbe():
            return {"project"} | extra
        case ClaudeOtelProbe():
            roots = {
                arm.tree.replace(VARIANT, variant)
                for arm in definition.arms
                for variant in definition.variants
            }
            roots |= {
                arm.server.source for arm in definition.arms if isinstance(arm.server, PluginDir)
            }
            roots |= {
                arm.server.template for arm in definition.arms if isinstance(arm.server, McpConfig)
            }
            return roots | extra
        case CustomProbe():
            return extra
        case _:
            assert_never(definition)


def _unreachable(package: Path) -> set[str]:
    definition = load_definition(package, DEFINITION_TYPES)
    roots = _declared_roots(package, definition)
    if roots is None:
        return set()
    return {p.name for p in (package / "fixtures").iterdir()} - roots


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


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_every_declared_fixture_path_exists(package: Path) -> None:
    load_definition(package, DEFINITION_TYPES)
    module = sys.modules[f"probe_definition_{package.name.replace('-', '_')}"]
    for name in getattr(module, "FIXTURE_PATHS", ()):
        assert (package / "fixtures" / name).exists(), f"{package.name}: no fixtures/{name}"
