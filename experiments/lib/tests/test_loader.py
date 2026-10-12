"""A package whose definition cannot be loaded is refused by name."""

import json
from pathlib import Path

from tests.conftest import Cli


def _put_manifest(tree: Path, name: str) -> Path:
    package = tree / name
    package.mkdir()
    manifest = {
        "schema_version": 1,
        "provider": "opencode",
        "driver": "unattended",
        "depth": None,
        "question": "q",
        "version_source": {"kind": "none"},
        "assertion": {"projection": ".", "expected": 1},
    }
    (package / "probe.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return package


def test_a_package_without_probe_py_is_refused(cli: Cli, tree: Path) -> None:
    _put_manifest(tree, "bare")

    result = cli("run", "bare")

    assert result.returncode == 1, result.output
    assert "has no probe.py" in result.output


def test_a_probe_py_without_probe_is_refused(cli: Cli, tree: Path) -> None:
    package = _put_manifest(tree, "undefined")
    (package / "probe.py").write_text("DEFINITION = None\n", encoding="utf-8")

    result = cli("run", "undefined")

    assert result.returncode == 1, result.output
    assert "defines no PROBE" in result.output
