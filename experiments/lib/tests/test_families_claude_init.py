"""The Claude init family, driven by a stub `claude` first on `PATH`; no model is called.

The stub takes the arm from its working directory's parent, reads its whole stdin, and prints
`$STUB_SOURCE/<arm>.jsonl` as its stdout. It appends its arm, `PWD`, working directory, argv,
environment, stdin, and the relocated home's `.claude.json` (when `HOME` names one) to
`STUB_ENV_FILE` as one JSON line, then exits 1, as an unrecognized model does. `--version`
prints 9.9.9.
"""

import json
import os
import shutil
import textwrap
from collections.abc import Sequence
from pathlib import Path
from typing import Any

import pytest

from tests.conftest import Cli, workspaces

MODEL = "unrecognized-test-model"

STUB = textwrap.dedent("""\
    #!/usr/bin/env python3
    import json
    import os
    import sys
    from pathlib import Path

    if sys.argv[1:] == ["--version"]:
        print("9.9.9")
        sys.exit(0)
    stdin = sys.stdin.read()
    arm = Path.cwd().parent.name
    config = Path(os.environ["HOME"]) / ".claude.json"
    with open(os.environ["STUB_ENV_FILE"], "a") as f:
        f.write(json.dumps({"arm": arm, "pwd": os.environ.get("PWD"), "cwd": os.getcwd(),
                            "argv": sys.argv[1:], "env": dict(os.environ), "stdin": stdin,
                            "config": config.read_text() if config.is_file() else None})
                + "\\n")
    sys.stdout.write((Path(os.environ["STUB_SOURCE"]) / f"{arm}.jsonl").read_text())
    sys.exit(1)
""")

DEFINITION = textwrap.dedent(f"""\
    from probe_harness.families.claude_init import ClaudeInitArm, ClaudeInitProbe, UserConfig

    PROBE = ClaudeInitProbe(
        model="{MODEL}",
        arms=(
            ClaudeInitArm("login", "tree", ("--strict-mcp-config",)),
            ClaudeInitArm(
                "relocated",
                "tree",
                home=UserConfig(),
                setting_sources=None,
                set_env=(("STUB_SET_ME", "on"), ("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "0")),
            ),
        ),
        settle_seconds=0,
        unset_env=("STUB_UNSET_ME",),
        assertion=lambda arms: {{
            name: sorted(s.source for s in capture.init.mcp_servers)
            for name, capture in arms.items()
        }},
    )
""")

MANIFEST = {
    "schema_version": 2,
    "provider": "claude",
    "driver": "unattended",
    "depth": "resolved-config",
    "question": "q",
    "version_source": {"kind": "command", "command": "claude --version"},
    "assertion": {"expected": {"login": ["claudeai"], "relocated": ["user"]}},
}

USER_CONFIG = {
    "mcpServers": {
        "ux": {
            "type": "stdio",
            "command": "{{PYTHON}}",
            "args": ["{{FX_SERVER}}", "--stamp", "{{USER_STAMP}}"],
        }
    }
}


@pytest.fixture
def package(tree: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    stub = bin_dir / "claude"
    stub.write_text(STUB, encoding="utf-8")
    stub.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")

    package = tree / "claude-init-family"
    fixtures = package / "fixtures" / "assertion"
    (fixtures / "tree").mkdir(parents=True)
    (fixtures / "tree" / "README.md").write_text("fixture\n", encoding="utf-8")
    (fixtures / "user.claude.json").write_text(json.dumps(USER_CONFIG), encoding="utf-8")
    (package / "probe.json").write_text(json.dumps(MANIFEST), encoding="utf-8")
    (package / "probe.py").write_text(DEFINITION, encoding="utf-8")
    return package


def _init(*servers: tuple[str, str], model: str = MODEL) -> dict[str, Any]:
    return {
        "type": "system",
        "subtype": "init",
        "model": model,
        "tools": ["Bash", "mcp__claude_ai_Docs__guide"],
        "mcp_servers": [
            {"name": f"server {n}", "status": status, "source": source}
            for n, (source, status) in enumerate(servers)
        ],
    }


def _result(text: str = f"There's an issue with the selected model ({MODEL}).") -> dict[str, Any]:
    return {"type": "result", "subtype": "success", "is_error": True, "result": text}


def _source(
    tmp_path: Path,
    login: Sequence[dict[str, Any]] = (_init(("claudeai", "connected")), _result()),
    relocated: Sequence[dict[str, Any]] = (_init(("user", "connected")), _result("Invalid key")),
) -> Path:
    """A fabricated stdout per arm, each event on its own line."""
    source = tmp_path / "source"
    source.mkdir()
    for arm, events in (("login", login), ("relocated", relocated)):
        lines = "".join(json.dumps(event) + "\n" for event in events)
        (source / f"{arm}.jsonl").write_text(lines, encoding="utf-8")
    return source


def _env(source: Path, env_file: Path, **extra: str) -> dict[str, str]:
    return {"STUB_SOURCE": str(source), "STUB_ENV_FILE": str(env_file), **extra}


def test_a_two_arm_definition_passing_every_gate_records_under_a_dry_run(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    env = _env(_source(tmp_path), tmp_path / "env.jsonl", PROBE_DRY_RUN="1")
    result = cli("run", "claude-init-family", env=env)
    assert result.returncode == 0, result.stderr
    record = json.loads(result.stdout)
    assert record["status"] == "confirmed"
    assert record["tool_version"] == "9.9.9"
    assert "probe run: the assembled view is at " in result.stderr
    assert not (package / "results").exists()


def test_each_arm_runs_as_declared_with_its_message_on_stdin(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    env_file = tmp_path / "env.jsonl"
    env = _env(
        _source(tmp_path),
        env_file,
        PROBE_DRY_RUN="1",
        STUB_UNSET_ME="present",
        ANTHROPIC_API_KEY="the operator's key",
        CLAUDE_CONFIG_DIR="/elsewhere",
        CLAUDE_CODE_DISABLE_CLAUDE_MDS="0",
    )
    result = cli("run", "claude-init-family", env=env)
    assert result.returncode == 0, result.stderr
    login, relocated = [json.loads(line) for line in env_file.read_text().splitlines()]
    assert login["argv"] == [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--setting-sources",
        "project",
        "--model",
        MODEL,
        "--strict-mcp-config",
    ]
    assert "--setting-sources" not in relocated["argv"]
    for call in (login, relocated):
        seen = call["env"]
        assert "STUB_UNSET_ME" not in seen
        assert "CLAUDE_CONFIG_DIR" not in seen
        # Forced past the caller's environment and, in `relocated`, the arm's own `set_env`.
        assert seen["CLAUDE_CODE_DISABLE_CLAUDE_MDS"] == "1"
        assert json.loads(call["stdin"]) == {
            "type": "user",
            "message": {"role": "user", "content": "hi"},
        }
        project = Path(call["cwd"])
        assert project.name == "project"
        assert Path(call["pwd"]).resolve() == project.resolve()
    assert "ANTHROPIC_API_KEY" not in login["env"]
    assert login["env"]["HOME"] == os.environ["HOME"]
    arm_dir = Path(relocated["cwd"]).parent
    assert relocated["env"]["HOME"] == str(arm_dir / "home")
    assert relocated["env"]["ANTHROPIC_API_KEY"] == "sk-ant-agentspec-probe-placeholder"
    assert relocated["env"]["STUB_SET_ME"] == "on"
    config = json.loads(relocated["config"])
    assert config["mcpServers"]["ux"]["args"][1:] == ["--stamp", str(arm_dir / "user.stamp")]


def test_a_pending_server_refuses_and_keeps_the_workspace(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    login = (_init(("claudeai", "pending")), _result())
    result = cli("run", "claude-init-family", env=_env(_source(tmp_path, login), tmp_path / "e"))
    assert result.returncode == 1
    assert "probe run: arm login: init listed 1 MCP server(s) still pending" in result.stderr
    kept = result.stderr.split("probe run: the workspace has been kept for inspection: ", 1)[1]
    assert Path(kept.strip()).is_dir()


def test_an_init_naming_another_model_refuses(cli: Cli, package: Path, tmp_path: Path) -> None:
    login = (_init(("claudeai", "connected"), model="a-real-model"), _result())
    result = cli("run", "claude-init-family", env=_env(_source(tmp_path, login), tmp_path / "e"))
    assert result.returncode == 1
    assert f"probe run: arm login: init names the model a-real-model, not {MODEL}." in (
        result.stderr
    )


def test_a_run_that_ended_without_an_error_refuses(cli: Cli, package: Path, tmp_path: Path) -> None:
    answered = {**_result(), "is_error": False}
    login = (_init(("claudeai", "connected")), answered)
    result = cli("run", "claude-init-family", env=_env(_source(tmp_path, login), tmp_path / "e"))
    assert result.returncode == 1
    assert "probe run: arm login: the run ended without an error" in result.stderr


def test_a_login_arm_failing_on_anything_but_the_model_refuses(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    login = (_init(("claudeai", "connected")), _result("Not logged in · Please run /login"))
    result = cli("run", "claude-init-family", env=_env(_source(tmp_path, login), tmp_path / "e"))
    assert result.returncode == 1
    assert "probe run: arm login: the run failed on something other than the model:" in (
        result.stderr
    )
    assert "Not logged in" in result.stderr


def test_an_arm_printing_no_init_event_refuses(cli: Cli, package: Path, tmp_path: Path) -> None:
    result = cli(
        "run", "claude-init-family", env=_env(_source(tmp_path, (_result(),)), tmp_path / "e")
    )
    assert result.returncode == 1
    assert "holds 0 init events, not one." in result.stderr


def test_record_dry_run_judges_a_kept_workspace(cli: Cli, package: Path, tmp_path: Path) -> None:
    env = _env(_source(tmp_path), tmp_path / "env.jsonl", PROBE_DRY_RUN="1")
    assert cli("run", "claude-init-family", env=env).returncode == 0
    (ws,) = workspaces("claude-init-family")
    copy = tmp_path / "copy"
    shutil.copytree(ws, copy)
    replayed = cli("record", "claude-init-family", copy, "--dry-run")
    assert replayed.returncode == 0, replayed.stderr
    assert json.loads(replayed.stdout)["status"] == "confirmed"


def test_a_sample_keeps_only_the_server_sources_the_assertion_reads(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    env = _env(_source(tmp_path), tmp_path / "env.jsonl", PROBE_DRY_RUN="1")
    assert cli("run", "claude-init-family", env=env).returncode == 0
    (ws,) = workspaces("claude-init-family")
    written = cli("sample", "claude-init-family", ws, "confirming")
    assert written.returncode == 0, written.stderr
    sample = json.loads((package / "samples" / "confirming.json").read_text(encoding="utf-8"))
    assert sample["results"] == {"assertion": {"login": ["claudeai"], "relocated": ["user"]}}
    init = sample["input"]["login"]["init"]
    assert init["mcp_servers"] == [{"name": "", "status": "", "source": "claudeai"}]
    assert init["tools"] == ["", ""]
    assert sample["input"]["relocated"]["result"]["result"] == ""


def test_a_missing_user_config_template_is_refused_before_any_arm_runs(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    (package / "fixtures" / "assertion" / "user.claude.json").unlink()
    env_file = tmp_path / "env.jsonl"
    result = cli("run", "claude-init-family", env=_env(_source(tmp_path), env_file))
    assert result.returncode == 1
    assert "arm relocated reads fixtures/assertion/user.claude.json, which does not exist" in (
        result.stderr
    )
    assert not env_file.exists()
