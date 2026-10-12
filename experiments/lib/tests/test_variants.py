"""Declared fixture variants: `assertion` comes first, and every declared tree exists."""

from pathlib import Path

import pytest
from probe_harness.families import DEFINITION_TYPES
from probe_harness.families.claude_otel import VARIANT, ClaudeOtelProbe
from probe_harness.families.opencode_debug import OpenCodeDebugProbe
from probe_harness.loader import load_definition

from tests.conftest import EXPERIMENTS_DIR


def _packages() -> list[Path]:
    return sorted(manifest.parent for manifest in EXPERIMENTS_DIR.glob("*/probe.json"))


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_every_definition_declares_assertion_first(package: Path) -> None:
    definition = load_definition(package, DEFINITION_TYPES)
    assert definition.variants[0] == "assertion"
    assert len(set(definition.variants)) == len(definition.variants)


@pytest.mark.parametrize("package", _packages(), ids=lambda p: p.name)
def test_every_declared_variant_tree_exists(package: Path) -> None:
    # Only the families that select a tree by variant have one per variant.
    definition = load_definition(package, DEFINITION_TYPES)
    fixtures = package / "fixtures"
    if isinstance(definition, OpenCodeDebugProbe):
        for variant in definition.variants:
            assert (fixtures / variant).is_dir(), f"{package.name}: no fixtures/{variant}"
    if isinstance(definition, ClaudeOtelProbe):
        for arm in definition.arms:
            for variant in definition.variants:
                tree = arm.tree.replace(VARIANT, variant)
                assert (fixtures / tree).is_dir(), f"{package.name}: no fixtures/{tree}"
