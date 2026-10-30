"""The top-level argument parser in `cli.py`, and `probe source`."""

import json
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from tests.conftest import EXPERIMENTS_DIR, Cli, GitRepo


def test_no_subcommand_exits_2(cli: Cli) -> None:
    result = cli()
    assert result.returncode == 2
    assert "probe: error: the following arguments are required: command" in result.stderr


def test_an_unknown_subcommand_exits_2(cli: Cli) -> None:
    result = cli("bogus")
    assert result.returncode == 2
    assert "probe: error: argument command: invalid choice: 'bogus'" in result.stderr


def test_an_abbreviated_subcommand_flag_is_refused(cli: Cli) -> None:
    result = cli("status", "--sum")
    assert result.returncode == 2
    assert "probe status: error: unrecognized arguments: --sum" in result.stderr


def _record_file(directory: Path, document: dict[str, Any]) -> Path:
    results = directory / "results"
    results.mkdir(parents=True, exist_ok=True)
    path = results / "2026-10-30T000000-opencode-1.0.json"
    path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    return path


def _v2(files: dict[str, str]) -> dict[str, Any]:
    return {
        "schema_version": 2,
        "provider": "opencode",
        "status": "confirmed",
        "depth": None,
        "date": "2026-10-30",
        "tool_version": "1.0",
        "source": files,
        "assertion": {"expected": 1, "observed": 1},
    }


@pytest.fixture
def committed_file(git_repo: GitRepo) -> tuple[Path, str]:
    """A file committed in <git_repo>, and its blob id."""
    path = git_repo.root / "experiments" / "pkg" / "probe.py"
    path.parent.mkdir(parents=True)
    path.write_text("PROBE = 1\n", encoding="utf-8")
    git_repo.commit()
    return path, git_repo.git("rev-parse", "HEAD:experiments/pkg/probe.py").strip()


def test_source_refuses_a_schema_1_record(cli: Cli, tmp_path: Path) -> None:
    record = _record_file(
        tmp_path,
        {
            **{k: v for k, v in _v2({}).items() if k != "source"},
            "schema_version": 1,
            "assertion": {"projection": ".", "expected": 1, "observed": 1},
        },
    )

    result = cli("source", record, tmp_path / "out")

    assert result.returncode == 1
    assert result.stderr == (
        f"probe source: {record} is a schema 1 record; its assertion text is in the record\n"
    )
    assert not (tmp_path / "out").exists()


def test_source_writes_a_record_s_files(
    cli: Cli, tmp_path: Path, committed_file: tuple[Path, str]
) -> None:
    path, blob = committed_file
    record = _record_file(path.parent, _v2({"experiments/pkg/probe.py": blob}))
    out = tmp_path / "out"

    result = cli("source", record, out)

    assert result.returncode == 0, result.output
    assert result.stdout == f"wrote 1 files to {out}\n"
    assert (out / "experiments/pkg/probe.py").read_text() == "PROBE = 1\n"


def test_source_lists_each_missing_object_and_exits_1(
    cli: Cli, tmp_path: Path, committed_file: tuple[Path, str]
) -> None:
    path, blob = committed_file
    files = {"experiments/pkg/probe.py": blob, "experiments/pkg/gone.py": "e" * 40}
    record = _record_file(path.parent, _v2(files))
    out = tmp_path / "out"

    result = cli("source", record, out)

    assert result.returncode == 1
    assert result.stdout == "experiments/pkg/gone.py\n"
    assert "probe source: these files' objects are missing from the repository:" in result.stderr
    assert (out / "experiments/pkg/probe.py").is_file()


def test_source_refuses_an_unreadable_record(cli: Cli, tmp_path: Path) -> None:
    record = tmp_path / "r.json"
    record.write_text("[]\n", encoding="utf-8")
    result = cli("source", record, tmp_path / "out")
    assert result.returncode == 1
    assert result.stderr == f"probe source: not a readable record: {record}\n"


def test_source_executes_no_probe_authored_code(
    tmp_path: Path, committed_file: tuple[Path, str]
) -> None:
    path, blob = committed_file
    record = _record_file(path.parent, _v2({"experiments/pkg/probe.py": blob}))
    script = (
        "import sys\n"
        "from probe_harness import cli\n"
        f"code = cli.main(['source', {str(record)!r}, {str(tmp_path / 'out')!r}])\n"
        "loaded = [m in sys.modules for m in ('probe_harness.loader', 'probe_harness.families')]\n"
        "print(code, *loaded)\n"
    )
    completed = subprocess.run(
        [sys.executable, "-c", script],
        cwd=EXPERIMENTS_DIR / "lib",
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.stdout.splitlines()[-1] == "0 False False", completed.stderr
