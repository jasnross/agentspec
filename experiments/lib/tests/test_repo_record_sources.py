"""The reachability rule: every blob id a package's newest v2 record names is reachable from
`HEAD`, so `probe source` can write the code that judged it back out.

A record names its code by blob id alone. A rebase that rewrites the commit holding a
probe's code before it reaches `main` can leave the record naming an object no ref
reaches, which `git gc` later deletes. `arranged` is not checked: it names the code that
arranged a workspace, which may never have been committed.
"""

import subprocess
from pathlib import Path

from probe_harness.contract import RecordV2, parse_record
from probe_harness.state import newest_record

from tests.conftest import EXPERIMENTS_DIR, GitRepo


def test_every_newest_v2_record_s_sources_are_reachable_from_head() -> None:
    assert _unreachable_sources(EXPERIMENTS_DIR) == []


def test_the_checker_names_a_file_whose_blob_history_rewriting_lost(git_repo: GitRepo) -> None:
    experiments = git_repo.root / "experiments"
    package = experiments / "pkg"
    package.mkdir(parents=True)
    probe = package / "probe.py"
    probe.write_text("PROBE = 1\n", encoding="utf-8")
    git_repo.commit()
    lost = git_repo.git("rev-parse", "HEAD:experiments/pkg/probe.py").strip()
    _write_record(package, {"experiments/pkg/probe.py": lost})
    assert _unreachable_sources(experiments) == []

    # The commit holding the blob is rewritten, so only the unreachable object still has it.
    probe.write_text("PROBE = 2\n", encoding="utf-8")
    git_repo.git("add", "--", str(probe))
    git_repo.git("commit", "--quiet", "--amend", "--no-edit", "--no-verify")

    assert _unreachable_sources(experiments) == [
        f"{package.name}: experiments/pkg/probe.py {lost} is not reachable from HEAD"
    ]


def _unreachable_sources(experiments: Path) -> list[str]:
    """One line per `source` entry of a newest v2 record that `HEAD` does not reach."""
    listed = subprocess.run(
        ["git", "rev-list", "--objects", "HEAD"],
        cwd=experiments,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    reachable = {line.split(" ", 1)[0] for line in listed.splitlines()}
    problems: list[str] = []
    for package in sorted(p for p in experiments.iterdir() if p.is_dir()):
        path = newest_record(package)
        record = parse_record(path) if path is not None else None
        if not isinstance(record, RecordV2):
            continue
        problems += (
            f"{package.name}: {key} {blob} is not reachable from HEAD"
            for key, blob in record.source.items()
            if blob not in reachable
        )
    return problems


def _write_record(package: Path, files: dict[str, str]) -> None:
    record = RecordV2.model_validate(
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
    results = package / "results"
    results.mkdir(exist_ok=True)
    (results / "2026-10-30T000000-opencode-unknown.json").write_text(
        record.model_dump_json(), encoding="utf-8"
    )
