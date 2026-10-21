"""The source map: which files it covers, how it keys them, and the ids it records."""

import subprocess
from pathlib import Path

from probe_harness.source import source_map

from tests.conftest import EXPERIMENTS_DIR


def _blob(path: Path) -> str:
    completed = subprocess.run(
        ["git", "hash-object", str(path)], capture_output=True, text=True, check=True
    )
    return completed.stdout.strip()


def test_a_package_outside_any_repository_is_keyed_under_its_experiments_directory(
    tmp_path: Path,
) -> None:
    package = tmp_path / "experiments" / "pkg"
    (package / "fixtures" / "assertion").mkdir(parents=True)
    (package / "probe.py").write_text("PROBE = None\n", encoding="utf-8")
    (package / "probe.json").write_text("{}\n", encoding="utf-8")
    fixture = package / "fixtures" / "assertion" / "config.json"
    fixture.write_text("{}\n", encoding="utf-8")
    (package / "fixtures" / "__pycache__").mkdir()
    (package / "fixtures" / "__pycache__" / "x.pyc").write_bytes(b"\0")

    source = source_map(package)

    assert source["experiments/pkg/probe.py"] == _blob(package / "probe.py")
    assert source["experiments/pkg/probe.json"] == _blob(package / "probe.json")
    assert source["experiments/pkg/fixtures/assertion/config.json"] == _blob(fixture)
    assert not any("__pycache__" in key for key in source)
    assert "experiments/probe" in source
    assert list(source) == sorted(source)


def test_a_committed_package_is_keyed_by_repository_path_with_working_content_ids() -> None:
    package = EXPERIMENTS_DIR / "opencode-agent-variant"

    source = source_map(package)

    package_keys = [key for key in source if key.startswith("experiments/opencode-agent-variant/")]
    assert "experiments/opencode-agent-variant/probe.py" in package_keys
    assert any("/fixtures/" in key for key in package_keys)
    assert all(key.startswith("experiments/") for key in source)
    assert not any("__pycache__" in key for key in source)
    root = EXPERIMENTS_DIR.parent
    for key in (*package_keys, "experiments/lib/probe_harness/source.py", "experiments/uv.lock"):
        assert source[key] == _blob(root / key)
