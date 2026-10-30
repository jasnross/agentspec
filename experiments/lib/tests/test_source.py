"""The source map: which files it covers, how it keys them, and the ids it records."""

import subprocess
from pathlib import Path

import pytest
from probe_harness import source
from probe_harness.contract import RecordV2
from probe_harness.source import SourceMapError, clean_tree_refusal, materialize, source_map

from tests.conftest import EXPERIMENTS_DIR, GitRepo, fake_harness


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


@pytest.fixture
def committed(git_repo: GitRepo, monkeypatch: pytest.MonkeyPatch) -> Path:
    """A package and a stand-in harness, both committed in <git_repo>; returns the package."""
    experiments = fake_harness(git_repo.root / "experiments")
    monkeypatch.setattr(source, "HARNESS_ROOT", experiments)
    package = experiments / "pkg"
    (package / "fixtures" / "assertion").mkdir(parents=True)
    (package / "probe.json").write_text("{}\n", encoding="utf-8")
    (package / "probe.py").write_text("PROBE = None\n", encoding="utf-8")
    (package / "fixtures" / "assertion" / "config.json").write_text("{}\n", encoding="utf-8")
    git_repo.commit()
    return package


def test_the_gate_passes_a_clean_tree(committed: Path) -> None:
    assert clean_tree_refusal(source_map(committed), committed) is None


def test_the_gate_refuses_a_modified_covered_file_by_name(committed: Path) -> None:
    (committed / "probe.py").write_text("PROBE = 1\n", encoding="utf-8")
    refusal = clean_tree_refusal(source_map(committed), committed)
    assert refusal is not None
    assert "commit these files" in refusal[0]
    assert refusal[1:] == ("  modified: experiments/pkg/probe.py",)


def test_the_gate_refuses_an_untracked_covered_file_by_name(committed: Path) -> None:
    (committed / "fixtures" / "assertion" / "new.json").write_text("{}\n", encoding="utf-8")
    refusal = clean_tree_refusal(source_map(committed), committed)
    assert refusal is not None
    assert refusal[1:] == ("  uncommitted: experiments/pkg/fixtures/assertion/new.json",)


def test_the_gate_refuses_a_deleted_covered_file_by_name(committed: Path) -> None:
    (committed.parent / "lib" / "stubs" / "stub.py").unlink()
    refusal = clean_tree_refusal(source_map(committed), committed)
    assert refusal is not None
    assert refusal[1:] == ("  deleted: experiments/lib/stubs/stub.py",)


def test_the_gate_ignores_an_uncovered_file(committed: Path, git_repo: GitRepo) -> None:
    (git_repo.root / "README.md").write_text("changed\n", encoding="utf-8")
    (committed / "README.md").write_text("changed\n", encoding="utf-8")
    assert clean_tree_refusal(source_map(committed), committed) is None


def test_the_gate_names_at_most_ten_paths(committed: Path) -> None:
    for i in range(12):
        (committed / "fixtures" / "assertion" / f"{i:02}.json").write_text("{}\n")
    refusal = clean_tree_refusal(source_map(committed), committed)
    assert refusal is not None
    assert len(refusal) == 12
    assert refusal[-1] == "  … and 2 more"


def test_the_gate_refuses_a_harness_outside_any_work_tree(
    committed: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    outside = fake_harness(tmp_path / "loose" / "experiments")
    monkeypatch.setattr(source, "HARNESS_ROOT", outside)
    refusal = clean_tree_refusal(source_map(committed), committed)
    assert refusal == (
        f"{outside.parent} is not inside a git work tree, so its files have no commit",
    )


def test_the_gate_is_skipped_for_a_package_outside_any_work_tree(tmp_path: Path) -> None:
    package = tmp_path / "experiments" / "pkg"
    package.mkdir(parents=True)
    (package / "probe.py").write_text("PROBE = None\n", encoding="utf-8")
    assert clean_tree_refusal(source_map(package), package) is None


def _record(files: dict[str, str]) -> RecordV2:
    return RecordV2.model_validate(
        {
            "schema_version": 2,
            "provider": "opencode",
            "status": "confirmed",
            "depth": None,
            "date": "2026-10-30",
            "tool_version": None,
            "source": files,
            "assertion": {"expected": 1, "observed": 1},
        }
    )


def test_materialize_round_trips_a_committed_file(committed: Path, tmp_path: Path) -> None:
    record_path = committed / "results" / "r.json"
    record_path.parent.mkdir()
    out = tmp_path / "out"

    missing = materialize(_record(source_map(committed)), record_path, out, arranged=False)

    assert missing == []
    root = committed.parent.parent
    for relative in ("experiments/pkg/probe.py", "experiments/lib/probe_harness/cli.py"):
        assert (out / relative).read_bytes() == (root / relative).read_bytes()


def test_materialize_reports_a_missing_object_and_writes_the_rest(
    committed: Path, tmp_path: Path
) -> None:
    files = {
        "experiments/pkg/probe.py": _blob(committed / "probe.py"),
        "experiments/pkg/gone.py": "f" * 40,
    }
    out = tmp_path / "out"

    missing = materialize(_record(files), committed / "probe.json", out, arranged=False)

    assert missing == ["experiments/pkg/gone.py"]
    assert (out / "experiments/pkg/probe.py").read_text() == "PROBE = None\n"
    assert not (out / "experiments/pkg/gone.py").exists()


def test_materialize_writes_the_arranged_files_when_asked(committed: Path, tmp_path: Path) -> None:
    judged = {"experiments/pkg/probe.py": _blob(committed / "probe.py")}
    arranged = {"experiments/pkg/probe.json": _blob(committed / "probe.json")}
    record = _record(judged).model_copy(update={"arranged": arranged})
    out = tmp_path / "out"

    assert materialize(record, committed / "probe.json", out, arranged=True) == []
    assert (out / "experiments/pkg/probe.json").is_file()
    assert not (out / "experiments/pkg/probe.py").exists()


def test_materialize_refuses_to_overwrite_before_writing_anything(
    committed: Path, tmp_path: Path
) -> None:
    files = source_map(committed)
    out = tmp_path / "out"
    (out / "experiments/pkg").mkdir(parents=True)
    (out / "experiments/pkg/probe.py").write_text("mine\n", encoding="utf-8")

    with pytest.raises(FileExistsError):
        materialize(_record(files), committed / "probe.json", out, arranged=False)
    assert (out / "experiments/pkg/probe.py").read_text() == "mine\n"
    assert not (out / "experiments/pkg/probe.json").exists()


def test_materialize_refuses_a_path_leaving_the_output_directory(
    committed: Path, tmp_path: Path
) -> None:
    files = {"../escape.py": _blob(committed / "probe.py")}
    with pytest.raises(ValueError, match="outside the repository"):
        materialize(_record(files), committed / "probe.json", tmp_path / "out", arranged=False)


def test_the_gate_refuses_a_repository_with_no_commit(
    git_repo: GitRepo, monkeypatch: pytest.MonkeyPatch
) -> None:
    experiments = fake_harness(git_repo.root / "experiments")
    monkeypatch.setattr(source, "HARNESS_ROOT", experiments)
    package = experiments / "pkg"
    package.mkdir()
    (package / "probe.py").write_text("PROBE = None\n", encoding="utf-8")

    with pytest.raises(SourceMapError, match="has no commit"):
        clean_tree_refusal(source_map(package), package)


def test_the_gate_compares_paths_under_a_repository_subdirectory(
    git_repo: GitRepo, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Keys are relative to each base, which here sits below the repository's top level, so
    # `HEAD`'s top-relative paths must lose the base's prefix to compare.
    experiments = fake_harness(git_repo.root / "nested" / "experiments")
    monkeypatch.setattr(source, "HARNESS_ROOT", experiments)
    package = experiments / "pkg"
    package.mkdir()
    (package / "probe.py").write_text("PROBE = None\n", encoding="utf-8")
    git_repo.commit()

    assert clean_tree_refusal(source_map(package), package) is None
    (package / "probe.py").write_text("PROBE = 1\n", encoding="utf-8")
    refusal = clean_tree_refusal(source_map(package), package)
    assert refusal is not None
    assert refusal[1:] == ("  modified: experiments/pkg/probe.py",)


def test_materialize_refuses_to_write_through_a_dangling_symlink(
    committed: Path, tmp_path: Path
) -> None:
    out = tmp_path / "out"
    (out / "experiments/pkg").mkdir(parents=True)
    (out / "experiments/pkg/probe.py").symlink_to(tmp_path / "elsewhere.py")
    files = {"experiments/pkg/probe.py": _blob(committed / "probe.py")}

    with pytest.raises(FileExistsError):
        materialize(_record(files), committed / "probe.json", out, arranged=False)
    assert not (tmp_path / "elsewhere.py").exists()
