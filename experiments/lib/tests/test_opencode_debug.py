"""The OpenCode debug family, driven by a stub `opencode` first on `PATH`.

The stub answers `--version`, and for `debug …` prints the file named by `STUB_ORACLE`
(nothing when unset) and exits `STUB_EXIT`. Side files let a test see how it was run:
`STUB_HOME_FILE` receives its `$HOME`, `STUB_CALL_FILE` its working directory and whether
its stdout was a pipe, and `STUB_ENV_FILE` its whole environment.
"""

import json
import os
import shutil
import textwrap
from pathlib import Path

import pytest

from tests.conftest import Cli, Result

STUB = textwrap.dedent("""\
    #!/usr/bin/env bash
    if [ "$1" = "--version" ]; then echo "9.9.9"; exit 0; fi
    if [ -n "${STUB_HOME_FILE:-}" ]; then printf '%s' "$HOME" > "$STUB_HOME_FILE"; fi
    if [ -n "${STUB_CALL_FILE:-}" ]; then
        if [ -p /dev/stdout ]; then kind=pipe; else kind=file; fi
        printf '%s\\n%s\\n' "$PWD" "$kind" > "$STUB_CALL_FILE"
    fi
    if [ -n "${STUB_ENV_FILE:-}" ]; then env > "$STUB_ENV_FILE"; fi
    echo "stub stderr" >&2
    if [ -n "${STUB_ORACLE:-}" ]; then cat "$STUB_ORACLE"; fi
    exit "${STUB_EXIT:-0}"
""")

AGENT_VIEW = {
    "name": "probe",
    "model": {"providerID": "anthropic", "modelID": "claude-sonnet-4-5"},
    "variant": "high",
}

KEPT = "probe: the workspace has been kept for inspection: "


@pytest.fixture
def stub_path(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    stub = bin_dir / "opencode"
    stub.write_text(STUB, encoding="utf-8")
    stub.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")
    return bin_dir


def _copy(experiments_dir: Path, tree: Path, name: str) -> Path:
    package = tree / name
    shutil.copytree(experiments_dir / name, package, symlinks=True)
    shutil.rmtree(package / "results", ignore_errors=True)
    return package


def _oracle(tmp_path: Path, content: str) -> str:
    path = tmp_path / "oracle.json"
    path.write_text(content, encoding="utf-8")
    return str(path)


def _kept(result: Result) -> Path:
    for line in result.stderr.splitlines():
        if line.startswith(KEPT):
            return Path(line.removeprefix(KEPT))
    raise AssertionError(f"no kept-workspace line in:\n{result.stderr}")


def test_an_empty_oracle_is_refused_and_keeps_the_workspace(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    _copy(experiments_dir, tree, "opencode-agent-variant")
    result = cli("run", "opencode-agent-variant", env={"STUB_ORACLE": _oracle(tmp_path, "")})
    assert result.returncode == 1
    assert "probe: the oracle produced no output; its stderr follows:" in result.stderr
    assert "stub stderr" in result.stderr
    assert _kept(result).is_dir()


def test_an_unparsable_oracle_is_refused(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    _copy(experiments_dir, tree, "opencode-agent-variant")
    result = cli("run", "opencode-agent-variant", env={"STUB_ORACLE": _oracle(tmp_path, "{")})
    assert result.returncode == 1
    assert "probe: the oracle wrote a document jq cannot parse — 1 bytes at" in result.stderr


def test_a_nonzero_oracle_is_refused_with_its_stderr(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path
) -> None:
    _copy(experiments_dir, tree, "opencode-agent-variant")
    result = cli("run", "opencode-agent-variant", env={"STUB_EXIT": "3"})
    assert result.returncode == 1
    assert 'probe: "opencode debug agent probe" exited nonzero:' in result.stderr
    assert "stub stderr" in result.stderr
    assert _kept(result).is_dir()


def test_a_config_without_the_fixture_command_lists_what_resolved(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    _copy(experiments_dir, tree, "opencode-command-variant")
    config: dict[str, dict[str, object]] = {"command": {"other": {}, "another": {}}}
    result = cli(
        "run",
        "opencode-command-variant",
        env={"STUB_ORACLE": _oracle(tmp_path, json.dumps(config))},
    )
    assert result.returncode == 1
    assert "probe: the fixture command did not resolve. OpenCode resolved 2 commands:" in (
        result.stderr
    )
    assert "probe:   another" in result.stderr
    assert "probe:   other" in result.stderr
    assert "probe: this is an apparatus failure, not a refutation; no record written." in (
        result.stderr
    )
    assert not (tree / "opencode-command-variant" / "results").exists()


def test_a_skill_list_carrying_the_fixture_twice_is_refused(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    _copy(experiments_dir, tree, "opencode-skill-frontmatter-discard")
    skill = {"name": "agentspec-probe-discard", "description": "d", "content": "", "location": "x"}
    result = cli(
        "run",
        "opencode-skill-frontmatter-discard",
        env={"STUB_ORACLE": _oracle(tmp_path, json.dumps([skill, skill]))},
    )
    assert result.returncode == 1
    assert "probe: the fixture resolved 2 times, expected exactly 1. Resolved skills:" in (
        result.stderr
    )
    assert result.stderr.count("probe:   agentspec-probe-discard") == 2


def test_a_dry_run_with_a_matching_oracle_records_nothing_and_keeps_the_workspace(
    cli: Cli,
    tree: Path,
    experiments_dir: Path,
    stub_path: Path,
    tmp_path: Path,
    _isolated_tmpdir: Path,
) -> None:
    package = _copy(experiments_dir, tree, "opencode-agent-variant")
    result = cli(
        "run",
        "opencode-agent-variant",
        env={"STUB_ORACLE": _oracle(tmp_path, json.dumps(AGENT_VIEW)), "PROBE_DRY_RUN": "1"},
    )
    assert result.returncode == 0
    assert "record: dry run — confirmed" in result.stderr
    assert json.loads(result.stdout)["status"] == "confirmed"
    assert not (package / "results").exists()
    assert len(list(_isolated_tmpdir.glob("probe-opencode-agent-variant.*"))) == 1


def test_a_real_run_records_and_deletes_the_workspace(
    cli: Cli,
    tree: Path,
    experiments_dir: Path,
    stub_path: Path,
    tmp_path: Path,
    _isolated_tmpdir: Path,
) -> None:
    package = _copy(experiments_dir, tree, "opencode-agent-variant")
    result = cli(
        "run",
        "opencode-agent-variant",
        env={"STUB_ORACLE": _oracle(tmp_path, json.dumps(AGENT_VIEW))},
    )
    assert result.returncode == 0
    records = list((package / "results").glob("*.json"))
    assert len(records) == 1
    assert json.loads(records[0].read_text(encoding="utf-8"))["tool_version"] == "9.9.9"
    assert list(_isolated_tmpdir.glob("probe-opencode-agent-variant.*")) == []


def test_the_discriminator_without_a_dry_run_is_refused(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, _isolated_tmpdir: Path
) -> None:
    _copy(experiments_dir, tree, "opencode-agent-variant")
    result = cli("run", "opencode-agent-variant", env={"PROBE_FIXTURE": "discriminator"})
    assert result.returncode == 2
    assert "probe: the discriminator fixture is dry-run only; set PROBE_DRY_RUN=1" in result.stderr
    assert list(_isolated_tmpdir.glob("probe-*")) == []


def test_an_isolated_definition_runs_the_oracle_with_home_inside_the_workspace(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    package = _copy(experiments_dir, tree, "opencode-agent-variant")
    (package / "probe.py").write_text(
        textwrap.dedent("""\
            from probe_harness.families.opencode_debug import OpenCodeDebugProbe
            from probe_harness.families.opencode_env import OpenCodeIsolation

            PROBE = OpenCodeDebugProbe(argv=("agent", "probe"), isolation=OpenCodeIsolation())
        """),
        encoding="utf-8",
    )
    home_file = tmp_path / "home.txt"
    result = cli(
        "run",
        "opencode-agent-variant",
        env={
            "STUB_ORACLE": _oracle(tmp_path, json.dumps(AGENT_VIEW)),
            "STUB_HOME_FILE": str(home_file),
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    home = Path(home_file.read_text(encoding="utf-8"))
    assert home.name == "home"
    ws = home.parent
    assert ws.name.startswith("probe-opencode-agent-variant.")
    assert (ws / "project" / ".opencode").is_dir()
    assert (ws / "xdg" / "config").is_dir()


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
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    _copy(experiments_dir, tree, "opencode-command-variant")
    result = cli(
        "run",
        "opencode-command-variant",
        env={"STUB_ORACLE": _oracle(tmp_path, json.dumps(COMMAND_CONFIG)), "PROBE_DRY_RUN": "1"},
    )
    assert result.returncode == 0
    assert json.loads(result.stdout)["status"] == "confirmed"


def test_a_config_with_no_commands_at_all_is_refused_as_unresolved(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    _copy(experiments_dir, tree, "opencode-command-variant")
    result = cli("run", "opencode-command-variant", env={"STUB_ORACLE": _oracle(tmp_path, "{}")})
    assert result.returncode == 1
    assert "probe: the fixture command did not resolve. OpenCode resolved 0 commands:" in (
        result.stderr
    )
    assert "probe: this is an apparatus failure, not a refutation; no record written." in (
        result.stderr
    )


def test_the_skill_judge_accepts_a_list_carrying_the_fixture_once(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    _copy(experiments_dir, tree, "opencode-skill-frontmatter-discard")
    other = {**FIXTURE_SKILL, "name": "other", "description": "o"}
    result = cli(
        "run",
        "opencode-skill-frontmatter-discard",
        env={
            "STUB_ORACLE": _oracle(tmp_path, json.dumps([other, FIXTURE_SKILL])),
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    assert json.loads(result.stdout)["status"] == "confirmed"


def test_a_skill_list_without_the_fixture_is_refused(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    _copy(experiments_dir, tree, "opencode-skill-frontmatter-discard")
    other = {**FIXTURE_SKILL, "name": "other"}
    result = cli(
        "run",
        "opencode-skill-frontmatter-discard",
        env={"STUB_ORACLE": _oracle(tmp_path, json.dumps([other]))},
    )
    assert result.returncode == 1
    assert "probe: the fixture resolved 0 times, expected exactly 1. Resolved skills:" in (
        result.stderr
    )
    assert "probe:   other" in result.stderr


def test_the_oracle_runs_in_the_workspace_with_stdout_to_a_file(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    # A pipe would truncate `opencode debug` output at 64 KB.
    _copy(experiments_dir, tree, "opencode-agent-variant")
    call = tmp_path / "call.txt"
    result = cli(
        "run",
        "opencode-agent-variant",
        env={
            "STUB_ORACLE": _oracle(tmp_path, json.dumps(AGENT_VIEW)),
            "STUB_CALL_FILE": str(call),
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    cwd, kind = call.read_text(encoding="utf-8").splitlines()
    view_line = next(
        line for line in result.stderr.splitlines() if line.startswith("probe: the assembled view")
    )
    assert view_line == f"probe: the assembled view is at {cwd}/view.json"
    assert kind == "file"


def test_an_isolated_oracle_sees_no_operator_config_and_its_own_directories(
    cli: Cli, tree: Path, experiments_dir: Path, stub_path: Path, tmp_path: Path
) -> None:
    package = _copy(experiments_dir, tree, "opencode-agent-variant")
    (package / "probe.py").write_text(
        textwrap.dedent("""\
            from probe_harness.families.opencode_debug import OpenCodeDebugProbe
            from probe_harness.families.opencode_env import OpenCodeIsolation

            PROBE = OpenCodeDebugProbe(argv=("agent", "probe"), isolation=OpenCodeIsolation())
        """),
        encoding="utf-8",
    )
    env_file = tmp_path / "env.txt"
    operator = {
        name: "operator"
        for name in (
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_DIR",
            "OPENCODE_CONFIG_CONTENT",
            "OPENCODE_PERMISSION",
            "OPENCODE_DISABLE_PROJECT_CONFIG",
        )
    }
    result = cli(
        "run",
        "opencode-agent-variant",
        env={
            **operator,
            "STUB_ORACLE": _oracle(tmp_path, json.dumps(AGENT_VIEW)),
            "STUB_ENV_FILE": str(env_file),
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    seen = dict(
        line.split("=", 1)
        for line in env_file.read_text(encoding="utf-8").splitlines()
        if "=" in line
    )
    assert not set(operator) & set(seen)
    ws = Path(seen["HOME"]).parent
    for kind in ("config", "cache", "data", "state"):
        assert seen[f"XDG_{kind.upper()}_HOME"] == str(ws / "xdg" / kind)
    assert seen["OPENCODE_DISABLE_AUTOUPDATE"] == "1"
    assert seen["OPENCODE_DISABLE_MODELS_FETCH"] == "1"


def test_the_discriminator_dry_run_arranges_the_discriminator_fixture(
    cli: Cli,
    tree: Path,
    experiments_dir: Path,
    stub_path: Path,
    tmp_path: Path,
    _isolated_tmpdir: Path,
) -> None:
    package = _copy(experiments_dir, tree, "opencode-agent-variant")
    result = cli(
        "run",
        "opencode-agent-variant",
        env={
            "STUB_ORACLE": _oracle(tmp_path, json.dumps(AGENT_VIEW)),
            "PROBE_FIXTURE": "discriminator",
            "PROBE_DRY_RUN": "1",
        },
    )
    assert result.returncode == 0
    (ws,) = _isolated_tmpdir.glob("probe-opencode-agent-variant.*")
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
    stub_path: Path,
    tmp_path: Path,
    _isolated_tmpdir: Path,
) -> None:
    package = _copy(experiments_dir, tree, "opencode-command-variant")
    run = cli(
        "run",
        "opencode-command-variant",
        env={"STUB_ORACLE": _oracle(tmp_path, json.dumps(COMMAND_CONFIG)), "PROBE_DRY_RUN": "1"},
    )
    assert run.returncode == 0
    (ws,) = _isolated_tmpdir.glob("probe-opencode-command-variant.*")

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
    assert "probe: the fixture command did not resolve." in refused.stderr
    assert not (package / "results").exists()
