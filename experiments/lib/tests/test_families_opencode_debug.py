"""The OpenCode debug family, driven by the `opencode_debug_stub` fixture's stub `opencode`.

Each test hands the stub an oracle document through `STUB_ORACLE` and judges what the
family makes of it.
"""

import json
import shutil
from pathlib import Path

from probe_harness.workspace import workspace_root

from tests.conftest import (
    AGENT_VIEW,
    Cli,
    Result,
    copy_package,
    oracle_file,
    workspaces,
)

KEPT = "probe run: the workspace has been kept for inspection: "


def _kept(result: Result) -> Path:
    for line in result.stderr.splitlines():
        if line.startswith(KEPT):
            return Path(line.removeprefix(KEPT))
    raise AssertionError(f"no kept-workspace line in:\n{result.stderr}")


def test_an_empty_oracle_is_refused_and_keeps_the_workspace(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    copy_package(experiments_dir, tree, "opencode-agent-variant")
    result = cli("run", "opencode-agent-variant", env={"STUB_ORACLE": oracle_file(tmp_path, "")})
    assert result.returncode == 1
    assert "probe run: the oracle produced no output; its stderr follows:" in result.stderr
    assert "stub stderr" in result.stderr
    assert _kept(result).is_dir()


def test_an_unparsable_oracle_is_refused(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    copy_package(experiments_dir, tree, "opencode-agent-variant")
    result = cli("run", "opencode-agent-variant", env={"STUB_ORACLE": oracle_file(tmp_path, "{")})
    assert result.returncode == 1
    assert (
        "probe run: the oracle wrote a document that is not valid JSON — 1 bytes at"
        in result.stderr
    )


def test_a_nonzero_oracle_is_refused_with_its_stderr(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path
) -> None:
    copy_package(experiments_dir, tree, "opencode-agent-variant")
    result = cli("run", "opencode-agent-variant", env={"STUB_EXIT": "3"})
    assert result.returncode == 1
    assert 'probe run: "opencode debug agent probe" exited nonzero:' in result.stderr
    # The provider's own output passes through unprefixed.
    assert "stub stderr" in result.stderr.splitlines()
    assert _kept(result).is_dir()


def test_a_config_without_the_fixture_command_lists_what_resolved(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    copy_package(experiments_dir, tree, "opencode-command-variant")
    config: dict[str, dict[str, object]] = {"command": {"other": {}, "another": {}}}
    result = cli(
        "run",
        "opencode-command-variant",
        env={"STUB_ORACLE": oracle_file(tmp_path, json.dumps(config))},
    )
    assert result.returncode == 1
    assert "probe run: the fixture command did not resolve. OpenCode resolved 2 commands:" in (
        result.stderr
    )
    assert "probe run:   another" in result.stderr
    assert "probe run:   other" in result.stderr
    assert "probe run: this is an apparatus failure, not a refutation; no record written." in (
        result.stderr
    )
    assert not (tree / "opencode-command-variant" / "results").exists()


def test_a_skill_list_carrying_the_fixture_twice_is_refused(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    copy_package(experiments_dir, tree, "opencode-skill-frontmatter-discard")
    skill = {"name": "agentspec-probe-discard", "description": "d", "content": "", "location": "x"}
    result = cli(
        "run",
        "opencode-skill-frontmatter-discard",
        env={"STUB_ORACLE": oracle_file(tmp_path, json.dumps([skill, skill]))},
    )
    assert result.returncode == 1
    assert "probe run: the fixture resolved 2 times, expected exactly 1. Resolved skills:" in (
        result.stderr
    )
    assert result.stderr.count("probe run:   agentspec-probe-discard") == 2


def test_a_dry_run_with_a_matching_oracle_records_nothing_and_keeps_the_workspace(
    cli: Cli,
    tree: Path,
    experiments_dir: Path,
    opencode_debug_stub: Path,
    tmp_path: Path,
) -> None:
    package = copy_package(experiments_dir, tree, "opencode-agent-variant")
    result = cli(
        "run",
        "opencode-agent-variant",
        env={"STUB_ORACLE": oracle_file(tmp_path, json.dumps(AGENT_VIEW)), "PROBE_DRY_RUN": "1"},
    )
    assert result.returncode == 0
    assert "probe run: dry run — confirmed" in result.stderr
    assert json.loads(result.stdout)["status"] == "confirmed"
    assert not (package / "results").exists()
    assert len(workspaces("opencode-agent-variant")) == 1


def test_a_real_run_records_and_keeps_the_workspace(
    cli: Cli,
    tree: Path,
    experiments_dir: Path,
    opencode_debug_stub: Path,
    tmp_path: Path,
) -> None:
    package = copy_package(experiments_dir, tree, "opencode-agent-variant")
    result = cli(
        "run",
        "opencode-agent-variant",
        env={"STUB_ORACLE": oracle_file(tmp_path, json.dumps(AGENT_VIEW))},
    )
    assert result.returncode == 0
    records = list((package / "results").glob("*.json"))
    assert len(records) == 1
    assert json.loads(records[0].read_text(encoding="utf-8"))["tool_version"] == "9.9.9"
    (ws,) = workspaces("opencode-agent-variant")
    assert f"probe run: the workspace is kept at {ws}\n" in result.stderr
    assert (ws / "source.json").is_file()
    assert (ws / "tool_version").read_text(encoding="utf-8") == "9.9.9\n"


def test_the_discriminator_without_a_dry_run_is_refused(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path
) -> None:
    copy_package(experiments_dir, tree, "opencode-agent-variant")
    result = cli("run", "opencode-agent-variant", env={"PROBE_FIXTURE": "discriminator"})
    assert result.returncode == 2
    assert (
        "probe run: the discriminator fixture is dry-run only; set PROBE_DRY_RUN=1" in result.stderr
    )
    assert list(workspace_root().glob("*/*")) == []


COMMAND_CONFIG = {
    "command": {
        "agentspec-probe-variant": {"model": "anthropic/claude-sonnet-4-5", "variant": "high"},
        "other": {},
    }
}


FIXTURE_SKILL = {
    "name": "agentspec-probe-discard",
    "description": "agentspec skill-frontmatter discard probe",
    "content": "",
    "location": "x",
}


def test_the_command_judge_accepts_a_config_carrying_the_fixture(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    copy_package(experiments_dir, tree, "opencode-command-variant")
    result = cli(
        "run",
        "opencode-command-variant",
        env={
            "STUB_ORACLE": oracle_file(tmp_path, json.dumps(COMMAND_CONFIG)),
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    assert json.loads(result.stdout)["status"] == "confirmed"


def test_a_config_with_no_commands_at_all_is_refused_as_unresolved(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    copy_package(experiments_dir, tree, "opencode-command-variant")
    result = cli(
        "run", "opencode-command-variant", env={"STUB_ORACLE": oracle_file(tmp_path, "{}")}
    )
    assert result.returncode == 1
    assert "probe run: the fixture command did not resolve. OpenCode resolved 0 commands:" in (
        result.stderr
    )
    assert "probe run: this is an apparatus failure, not a refutation; no record written." in (
        result.stderr
    )


def test_the_skill_judge_accepts_a_list_carrying_the_fixture_once(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    copy_package(experiments_dir, tree, "opencode-skill-frontmatter-discard")
    other = {**FIXTURE_SKILL, "name": "other", "description": "o"}
    result = cli(
        "run",
        "opencode-skill-frontmatter-discard",
        env={
            "STUB_ORACLE": oracle_file(tmp_path, json.dumps([other, FIXTURE_SKILL])),
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    assert json.loads(result.stdout)["status"] == "confirmed"


def test_a_skill_list_without_the_fixture_is_refused(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    copy_package(experiments_dir, tree, "opencode-skill-frontmatter-discard")
    other = {**FIXTURE_SKILL, "name": "other"}
    result = cli(
        "run",
        "opencode-skill-frontmatter-discard",
        env={"STUB_ORACLE": oracle_file(tmp_path, json.dumps([other]))},
    )
    assert result.returncode == 1
    assert "probe run: the fixture resolved 0 times, expected exactly 1. Resolved skills:" in (
        result.stderr
    )
    assert "probe run:   other" in result.stderr


def test_the_oracle_runs_in_the_workspace_with_stdout_to_a_file(
    cli: Cli, tree: Path, experiments_dir: Path, opencode_debug_stub: Path, tmp_path: Path
) -> None:
    # A pipe would truncate `opencode debug` output at 64 KB.
    copy_package(experiments_dir, tree, "opencode-agent-variant")
    call = tmp_path / "call.txt"
    result = cli(
        "run",
        "opencode-agent-variant",
        env={
            "STUB_ORACLE": oracle_file(tmp_path, json.dumps(AGENT_VIEW)),
            "STUB_CALL_FILE": str(call),
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    cwd, kind = call.read_text(encoding="utf-8").splitlines()
    view_line = next(
        line
        for line in result.stderr.splitlines()
        if line.startswith("probe run: the assembled view")
    )
    assert view_line == f"probe run: the assembled view is at {cwd}/view.json"
    assert kind == "file"


def test_the_discriminator_dry_run_arranges_the_discriminator_fixture(
    cli: Cli,
    tree: Path,
    experiments_dir: Path,
    opencode_debug_stub: Path,
    tmp_path: Path,
) -> None:
    package = copy_package(experiments_dir, tree, "opencode-agent-variant")
    result = cli(
        "run",
        "opencode-agent-variant",
        env={
            "STUB_ORACLE": oracle_file(tmp_path, json.dumps(AGENT_VIEW)),
            "PROBE_FIXTURE": "discriminator",
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    (ws,) = workspaces("opencode-agent-variant")
    source = package / "fixtures" / "discriminator"
    files = [path for path in source.rglob("*") if path.is_file()]
    assert files
    for path in files:
        assert (ws / path.relative_to(source)).read_bytes() == path.read_bytes()
    assertion = package / "fixtures" / "assertion"
    differs = [
        path
        for path in assertion.rglob("*")
        if path.is_file() and (ws / path.relative_to(assertion)).read_bytes() != path.read_bytes()
    ]
    assert differs


def test_replay_judges_a_kept_debug_workspace(
    cli: Cli,
    tree: Path,
    experiments_dir: Path,
    opencode_debug_stub: Path,
    tmp_path: Path,
) -> None:
    package = copy_package(experiments_dir, tree, "opencode-command-variant")
    run = cli(
        "run",
        "opencode-command-variant",
        env={
            "STUB_ORACLE": oracle_file(tmp_path, json.dumps(COMMAND_CONFIG)),
            "PROBE_DRY_RUN": "1",
        },
    )
    assert run.returncode == 0
    (ws,) = workspaces("opencode-command-variant")

    good = tmp_path / "good"
    shutil.copytree(ws, good)
    replayed = cli("replay", "opencode-command-variant", good)
    assert replayed.returncode == 0
    assert json.loads(replayed.stdout)["status"] == "confirmed"

    bad = tmp_path / "bad"
    shutil.copytree(ws, bad)
    (bad / "view.json").write_text(json.dumps({"command": {"other": {}}}), encoding="utf-8")
    refused = cli("replay", "opencode-command-variant", bad)
    assert refused.returncode == 1
    assert "probe replay: the fixture command did not resolve." in refused.stderr
    assert not (package / "results").exists()
