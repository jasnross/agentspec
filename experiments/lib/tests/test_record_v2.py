"""`probe record` over a v2 package inside a git repository: the clean-tree gate and the
`source` and `arranged` maps a real record carries.

In process, so `source.HARNESS_ROOT` can point at a committed stand-in harness: the real
harness tree is dirty while it is developed, and the gate would refuse it.
"""

import json
from pathlib import Path

import pytest
from probe_harness import families, source
from probe_harness.source import source_map, write_source

from tests.conftest import GitRepo, fake_harness, workspaces

MANIFEST = {
    "schema_version": 2,
    "provider": "opencode",
    "driver": "unattended",
    "depth": None,
    "question": "q",
    "version_source": {"kind": "none"},
    "assertion": {"expected": "high"},
}

PROBE_PY = """\
import json
from pathlib import Path
from probe_harness.families.base import CustomProbe, ProbeFunctions

def _parsed(ws: Path) -> object:
    return json.loads((ws / "view.json").read_text())

def _edit(view: object) -> None:
    # Edits a covered file while the workspace is judged.
    (Path(__file__).parent / "fixtures" / "assertion" / "config.json").write_text("{{}}\\n")

def _act(ws: Path) -> None:
    (ws / "view.json").write_text('{{"variant": "high"}}\\n')

def _act_and_edit(ws: Path) -> None:
    _act(ws)
    _edit(None)

PROBE = CustomProbe(required_tools=(), arrange=lambda ws, variant: None, act={act},
                    judge=_parsed, assertion=lambda view: view["variant"],
                    functions=ProbeFunctions(judge={judge}))
"""


@pytest.fixture
def experiments(git_repo: GitRepo, monkeypatch: pytest.MonkeyPatch) -> Path:
    root = fake_harness(git_repo.root / "experiments")
    monkeypatch.setattr(source, "HARNESS_ROOT", root)
    return root


def _package(
    experiments: Path, git_repo: GitRepo, *, judge: str = "None", act: str = "_act"
) -> Path:
    package = experiments / "pkg"
    (package / "fixtures" / "assertion").mkdir(parents=True)
    (package / "fixtures" / "assertion" / "config.json").write_text('{"v": 1}\n')
    (package / "probe.json").write_text(json.dumps(MANIFEST, indent=2) + "\n")
    (package / "probe.py").write_text(PROBE_PY.format(judge=judge, act=act))
    git_repo.commit()
    return package


@pytest.fixture
def ws(tmp_path: Path) -> Path:
    workspace = tmp_path / "ws"
    workspace.mkdir()
    (workspace / "run.json").write_text('{"package": "pkg", "variant": "assertion"}\n')
    (workspace / "view.json").write_text('{"variant": "high"}\n')
    return workspace


def _record(experiments: Path, ws: Path, *, dry_run: bool = False) -> int:
    return families.record_package("pkg", ws, None, dry_run=dry_run, experiments_dir=experiments)


def _written(package: Path) -> dict[str, object]:
    (path,) = sorted((package / "results").glob("*.json"))
    document: dict[str, object] = json.loads(path.read_text())
    return document


def _head_ids(git_repo: GitRepo) -> dict[str, str]:
    listed = git_repo.git("ls-tree", "-r", "HEAD", "--", "experiments")
    return {
        line.split("\t", 1)[1]: line.split()[2]
        for line in listed.splitlines()
        if not line.split("\t", 1)[1].startswith("experiments/pkg/results/")
    }


def test_a_real_record_carries_head_s_ids_and_no_arranged_when_the_run_s_map_matches(
    experiments: Path, git_repo: GitRepo, ws: Path
) -> None:
    package = _package(experiments, git_repo)
    write_source(ws, source_map(package))

    assert _record(experiments, ws) == 0

    written = _written(package)
    assert written["status"] == "confirmed"
    assert written["source"] == _head_ids(git_repo)
    assert "arranged" not in written


def test_a_real_record_carries_arranged_when_the_run_s_map_differs(
    experiments: Path, git_repo: GitRepo, ws: Path
) -> None:
    package = _package(experiments, git_repo)
    arranged = {**source_map(package), "experiments/pkg/probe.py": "0" * 40}
    write_source(ws, arranged)

    assert _record(experiments, ws) == 0

    written = _written(package)
    assert written["source"] == _head_ids(git_repo)
    assert written["arranged"] == arranged


def test_a_real_record_of_a_dirty_tree_is_refused_before_judging(
    experiments: Path, git_repo: GitRepo, ws: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    package = _package(experiments, git_repo)
    (package / "probe.json").write_text(json.dumps({**MANIFEST, "question": "q2"}) + "\n")

    assert _record(experiments, ws) == 1

    err = capsys.readouterr().err
    assert "commit these files and re-run" in err
    assert "  modified: experiments/pkg/probe.json\n" in err
    assert not (package / "results").exists()


def test_a_dry_run_of_a_dirty_tree_prints_the_record_from_the_files_on_disk(
    experiments: Path, git_repo: GitRepo, ws: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    package = _package(experiments, git_repo)
    (package / "fixtures" / "assertion" / "new.json").write_text("{}\n")

    assert _record(experiments, ws, dry_run=True) == 0

    printed = json.loads(capsys.readouterr().out)
    assert printed["source"] == source_map(package)
    assert "experiments/pkg/fixtures/assertion/new.json" in printed["source"]
    assert not (package / "results").exists()


def test_a_file_edited_while_judging_is_refused_with_the_workspace_kept(
    experiments: Path, git_repo: GitRepo, ws: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    package = _package(experiments, git_repo, judge="_edit")
    write_source(ws, source_map(package))

    assert _record(experiments, ws) == 1

    err = capsys.readouterr().err
    assert "the probe's source changed after its map was computed" in err
    assert "  experiments/pkg/fixtures/assertion/config.json\n" in err
    assert f"the workspace has been kept for inspection: {ws}" in err
    assert ws.is_dir()
    assert not (package / "results").exists()


def test_a_real_run_records_the_map_preflight_computed(
    experiments: Path, git_repo: GitRepo
) -> None:
    package = _package(experiments, git_repo)

    assert families.run_package("pkg", experiments) == 0

    written = _written(package)
    assert written["status"] == "confirmed"
    assert written["source"] == _head_ids(git_repo)
    assert "arranged" not in written


def test_a_real_run_of_a_dirty_tree_is_refused_before_a_workspace_exists(
    experiments: Path, git_repo: GitRepo, capsys: pytest.CaptureFixture[str]
) -> None:
    package = _package(experiments, git_repo)
    (package / "probe.py").write_text((package / "probe.py").read_text() + "\n")

    assert families.run_package("pkg", experiments) == 1

    assert "  modified: experiments/pkg/probe.py\n" in capsys.readouterr().err
    assert workspaces("pkg") == []


def test_a_real_run_whose_source_changes_under_it_is_refused_with_the_workspace_kept(
    experiments: Path, git_repo: GitRepo, capsys: pytest.CaptureFixture[str]
) -> None:
    package = _package(experiments, git_repo, act="_act_and_edit")

    assert families.run_package("pkg", experiments) == 1

    err = capsys.readouterr().err
    assert "the probe's source changed after its map was computed" in err
    (ws,) = workspaces("pkg")
    assert f"the workspace has been kept for inspection: {ws}" in err
    assert not (package / "results").exists()
