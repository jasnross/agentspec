"""The Claude OTEL family, driven by a stub `claude` first on `PATH`; no model is called.

The stub reads its sink from `OTEL_LOG_RAW_API_BODIES=file:<sink>`, takes the arm from the
sink's parent directory, and copies `$STUB_SOURCE/<arm>/sink/*` into the sink and
`$STUB_SOURCE/<arm>/fx.stamp` beside it when present. It appends its arm, `PWD`, working
directory, argv, environment, the project's `which.txt` and the files under the arm's
`fxp/` to `STUB_ENV_FILE` as one JSON line, then exits `STUB_EXIT` when set. `--version`
prints 9.9.9.
"""

import json
import os
import shutil
import textwrap
from pathlib import Path

import pytest
from probe_harness import samples
from probe_harness.families import load_package
from probe_harness.gates.bodies import DEFERRED_LISTING
from probe_harness.gates.claude_otel import CANARY_PREFIX

from tests.conftest import (
    Cli,
    claude_request,
    workspaces,
)

MARKER = "AGENTSPEC-PROBE-MARKER-CLAUDETEST"


MODEL = "claude-test-model"


STUB = textwrap.dedent("""\
    #!/usr/bin/env python3
    import json
    import os
    import shutil
    import sys
    from pathlib import Path

    if sys.argv[1:] == ["--version"]:
        print("9.9.9")
        sys.exit(0)
    sink = Path(os.environ["OTEL_LOG_RAW_API_BODIES"].removeprefix("file:"))
    arm = sink.parent.name
    source = Path(os.environ["STUB_SOURCE"]) / arm
    for body in sorted((source / "sink").iterdir()):
        shutil.copy(body, sink)
    if (source / "fx.stamp").exists():
        shutil.copy(source / "fx.stamp", sink.parent)
    which = Path("which.txt")
    plugin = sink.parent / "fxp"
    with open(os.environ["STUB_ENV_FILE"], "a") as f:
        f.write(json.dumps({"arm": arm, "pwd": os.environ.get("PWD"), "cwd": os.getcwd(),
                            "argv": sys.argv[1:], "env": dict(os.environ),
                            "which": which.read_text() if which.exists() else None,
                            "plugin": sorted(str(p.relative_to(plugin))
                                             for p in plugin.rglob("*") if p.is_file())
                            }) + "\\n")
    if os.environ.get("STUB_EXIT"):
        print("stub failing", file=sys.stderr)
        sys.exit(int(os.environ["STUB_EXIT"]))
""")


DEFINITION = textwrap.dedent(f"""\
    from probe_harness.families.claude_otel import ClaudeArm, ClaudeOtelProbe, McpConfig, Model

    PROBE = ClaudeOtelProbe(
        model="{MODEL}",
        budget_usd="0.10",
        marker="{MARKER}",
        arms=(
            ClaudeArm("a", "tree", "prompt a", ("--effort", "low"), McpConfig()),
            ClaudeArm("b", "tree", "prompt b", (), McpConfig()),
        ),
        gates=(Model(),),
        unset_env=("STUB_UNSET_ME",),
        set_env=(
            ("STUB_SET_ME", "on"),
            ("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "0"),
            ("ENABLE_CLAUDEAI_MCP_SERVERS", "true"),
        ),
        assertion=lambda arms: {{"a": len(arms["a"].requests), "b": len(arms["b"].requests)}},
    )
""")


MANIFEST = {
    "schema_version": 2,
    "provider": "claude",
    "driver": "billed",
    "depth": "outbound-request",
    "question": "q",
    "version_source": {"kind": "command", "command": "claude --version"},
    "assertion": {"expected": {"a": 1, "b": 1}},
}


MCP_JSON = {
    "mcpServers": {
        "fx": {
            "type": "stdio",
            "command": "{{PYTHON}}",
            "args": ["{{FX_SERVER}}", "--stamp", "{{FX_STAMP}}"],
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

    package = tree / "claude-family"
    fixtures = package / "fixtures" / "assertion"
    (fixtures / "tree").mkdir(parents=True)
    (fixtures / "tree" / "CLAUDE.md").write_text("fixture\n", encoding="utf-8")
    (fixtures / "mcp.json").write_text(json.dumps(MCP_JSON), encoding="utf-8")
    (package / "probe.json").write_text(json.dumps(MANIFEST), encoding="utf-8")
    (package / "probe.py").write_text(DEFINITION, encoding="utf-8")
    return package


def _source(
    tmp_path: Path, *, marked: tuple[str, ...] = ("a", "b"), stamped: tuple[str, ...] = ("a", "b")
) -> Path:
    """A fabricated capture: one request per arm, marked and stamped as asked."""
    source = tmp_path / "source"
    for arm in ("a", "b"):
        sink = source / arm / "sink"
        sink.mkdir(parents=True)
        system = f"agent body {MARKER}" if arm in marked else "plain"
        body = claude_request(system=system, model=MODEL, output_config={"effort": "low"})
        (sink / "0000.request.json").write_text(json.dumps(body), encoding="utf-8")
        if arm in stamped:
            (source / arm / "fx.stamp").write_text("tools/list\n", encoding="utf-8")
    return source


def _env(source: Path, env_file: Path, **extra: str) -> dict[str, str]:
    return {"STUB_SOURCE": str(source), "STUB_ENV_FILE": str(env_file), **extra}


def test_a_two_arm_definition_passing_every_gate_records_under_a_dry_run(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    env = _env(_source(tmp_path), tmp_path / "env.jsonl", PROBE_DRY_RUN="1")
    result = cli("run", "claude-family", env=env)
    assert result.returncode == 0, result.stderr
    record = json.loads(result.stdout)
    assert record["status"] == "confirmed"
    assert record["tool_version"] == "9.9.9"
    assert "probe run: the assembled view is at " in result.stderr
    assert not (package / "results").exists()


def test_a_marker_missing_from_one_arm_refuses_and_keeps_the_workspace(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    env = _env(_source(tmp_path, marked=("a",)), tmp_path / "env.jsonl")
    result = cli("run", "claude-family", env=env)
    assert result.returncode == 1
    assert "probe run: an arm captured no request whose .system carries the fixture marker." in (
        result.stderr
    )
    kept = result.stderr.split("probe run: the workspace has been kept for inspection: ", 1)[1]
    assert Path(kept.strip()).is_dir()


def test_an_arm_without_a_stamp_refuses(cli: Cli, package: Path, tmp_path: Path) -> None:
    env = _env(_source(tmp_path, stamped=("a",)), tmp_path / "env.jsonl")
    result = cli("run", "claude-family", env=env)
    assert result.returncode == 1
    assert "probe run: arm b: the fx server never answered tools/list" in result.stderr


def test_the_environment_reaching_claude_is_isolated_and_pointed_at_the_sink(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    env_file = tmp_path / "env.jsonl"
    env = _env(
        _source(tmp_path),
        env_file,
        PROBE_DRY_RUN="1",
        CLAUDE_CODE_EFFORT_LEVEL="max",
        STUB_UNSET_ME="present",
        # The definition sets both the other way too; the family's own values win over each.
        CLAUDE_CODE_DISABLE_CLAUDE_MDS="0",
        ENABLE_CLAUDEAI_MCP_SERVERS="true",
    )
    result = cli("run", "claude-family", env=env)
    assert result.returncode == 0, result.stderr
    calls = [json.loads(line) for line in env_file.read_text(encoding="utf-8").splitlines()]
    assert [c["arm"] for c in calls] == ["a", "b"]
    for call in calls:
        seen = call["env"]
        assert "CLAUDE_CODE_EFFORT_LEVEL" not in seen
        assert "STUB_UNSET_ME" not in seen
        assert seen["STUB_SET_ME"] == "on"
        assert seen["CLAUDE_CODE_DISABLE_CLAUDE_MDS"] == "1"
        assert seen["ENABLE_CLAUDEAI_MCP_SERVERS"] == "false"
        assert seen["CLAUDE_CODE_ENABLE_TELEMETRY"] == "1"
        project = Path(call["cwd"])
        assert project.name == "project"
        canary = (project.parent / "CLAUDE.md").read_text(encoding="utf-8")
        assert canary.startswith(f"Isolation canary: {CANARY_PREFIX}")
        assert Path(call["pwd"]).resolve() == project.resolve()
        assert seen["OTEL_LOG_RAW_API_BODIES"] == f"file:{Path(call['pwd']).parent / 'sink'}"
    a, b = calls
    assert a["argv"][:9] == [
        "-p",
        "prompt a",
        "--setting-sources",
        "project",
        "--model",
        MODEL,
        "--max-budget-usd",
        "0.10",
        "--mcp-config",
    ]
    assert a["argv"][10:] == ["--strict-mcp-config", "--effort", "low"]
    assert b["argv"][10:] == ["--strict-mcp-config"]
    config = json.loads(Path(a["argv"][9]).read_text(encoding="utf-8"))
    assert config["mcpServers"]["fx"]["args"][1:] == [
        "--stamp",
        str(Path(a["argv"][9]).parent / "fx.stamp"),
    ]


def test_record_dry_run_judges_a_kept_claude_workspace(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    run = cli(
        "run",
        "claude-family",
        env=_env(_source(tmp_path), tmp_path / "env.jsonl", PROBE_DRY_RUN="1"),
    )
    assert run.returncode == 0, run.stderr
    (ws,) = workspaces("claude-family")
    copy = tmp_path / "copy"
    shutil.copytree(ws, copy)
    replayed = cli("record", "claude-family", copy, "--dry-run")
    assert replayed.returncode == 0, replayed.stderr
    assert json.loads(replayed.stdout)["status"] == "confirmed"
    (copy / "a" / "fx.stamp").unlink()
    refused = cli("record", "claude-family", copy, "--dry-run")
    assert refused.returncode == 1
    assert "probe record: arm a: the fx server never answered tools/list" in refused.stderr


def test_a_claude_sample_keeps_no_unaccounted_line_its_expressions_do_not_read(
    cli: Cli, package: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # Claude Code sends the deferred-tools listing in one text block with its environment
    # details and its skill listing, which names the operator's own skills. The assertion
    # only counts requests, so none of it is needed; the cut keeps only the lines the
    # accounting check vouches for, here Claude Code's own tool name.
    listing = [DEFERRED_LISTING, "ToolSearch", "mcp__fx__alpha"]
    block = "\n".join(
        [
            "Working directory: /tmp/project",
            "",
            *listing,
            "mcp__claude_ai_Docs__guide",
            "",
            "The following skills are available:",
            "- my-own-skill: does the operator's own thing",
        ]
    )
    run = cli(
        "run",
        "claude-family",
        env=_env(_source(tmp_path), tmp_path / "env.jsonl", PROBE_DRY_RUN="1"),
    )
    assert run.returncode == 0, run.stderr
    (ws,) = workspaces("claude-family")
    # Written into the kept workspace after the run, since the isolation gate refuses a run
    # whose request names a connector; `probe sample` reads a workspace without gating it.
    request = claude_request(
        system=f"agent body {MARKER}",
        model=MODEL,
        output_config={"effort": "low"},
        messages=[{"role": "user", "content": [{"type": "text", "text": block}]}],
    )
    (ws / "a" / "sink" / "0000.request.json").write_text(json.dumps(request), encoding="utf-8")
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    definition, manifest = load_package(package)

    path = samples.write_sample(
        definition, manifest, ws, "confirming", identity=samples.Identity.current()
    )

    sample = json.loads(path.read_text(encoding="utf-8"))
    (cut,) = sample["input"]["a"]["requests"]
    assert cut["messages"][0]["content"][0]["text"] == "\nToolSearch\n"


def test_the_templated_server_runs_the_shared_stub_under_this_interpreter(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    env_file = tmp_path / "env.jsonl"
    result = cli("run", "claude-family", env=_env(_source(tmp_path), env_file, PROBE_DRY_RUN="1"))
    assert result.returncode == 0, result.stderr
    first = json.loads(env_file.read_text(encoding="utf-8").splitlines()[0])
    server = json.loads(Path(first["argv"][9]).read_text(encoding="utf-8"))["mcpServers"]["fx"]
    assert Path(server["args"][0]).is_file()
    assert Path(server["args"][0]).name == "fx_server.py"
    assert Path(server["command"]).name.startswith("python")


def test_a_nonzero_claude_exit_refuses_with_its_stderr(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    result = cli(
        "run",
        "claude-family",
        env=_env(_source(tmp_path), tmp_path / "env.jsonl", STUB_EXIT="3"),
    )
    assert result.returncode == 1
    assert "probe run: the a arm exited nonzero; its stderr follows:" in result.stderr
    assert "stub failing" in result.stderr
    assert "probe run: the workspace has been kept for inspection: " in result.stderr


RICH_DEFINITION = textwrap.dedent(f"""\
    from pathlib import Path

    from probe_harness.families.base import CustomCheck, ProbeFunctions
    from probe_harness.families.claude_otel import (
        ClaudeArm, ClaudeArms, ClaudeOtelProbe, McpConfig, McpConnected, PluginDir, ToolAnswered,
    )

    _ORDER = Path(__file__).parent / "order.log"


    def _arrange(fixtures: Path, arm_dir: Path, arm: ClaudeArm) -> None:
        agents = arm_dir / "fxp" / "agents"
        if arm.name == "b":
            agents.mkdir(parents=True, exist_ok=True)
            (agents / "arranged.md").write_text("arranged")


    def _judge(arms: ClaudeArms) -> None:
        a = arms["a"]
        with _ORDER.open("a") as f:
            f.write(f"judge saw {{len(a.requests)}} requests and {{len(a.responses)}} responses\\n")


    def _marked(arms: ClaudeArms) -> bool:
        return sum(
            1
            for capture in arms.values()
            for request in capture.requests
            if any("{MARKER}" in s for s in request.system.strings())
        ) >= MIN


    PROBE = ClaudeOtelProbe(
        model="{MODEL}",
        budget_usd="0.10",
        marker="{MARKER}",
        arms=(
            ClaudeArm("a", "tree", "prompt a", (), McpConfig()),
            ClaudeArm("b", "tree", "prompt b", (), PluginDir()),
        ),
        gates=(McpConnected(("Agent",)), ToolAnswered("a", "mcp__fx__alpha")),
        custom_checks=(CustomCheck("marked", _marked, ("the custom check refused.",)),),
        requires_responses=True,
        assertion=lambda arms: {{
            name: len(capture.requests) + len(capture.responses) for name, capture in arms.items()
        }},
        variants=("assertion", "discriminator"),
        arrange_arm=_arrange,
        functions=ProbeFunctions(judge=_judge),
    )
""")


def _rich(package: Path, *, custom_min: int = 2) -> None:
    for variant in ("assertion", "discriminator"):
        fixtures = package / "fixtures" / variant
        (fixtures / "tree").mkdir(parents=True, exist_ok=True)
        (fixtures / "tree" / "which.txt").write_text(variant, encoding="utf-8")
        # Each variant's server config is told apart by an extra key, so a test can see which
        # variant's copy an arm loaded.
        config = json.dumps({**MCP_JSON, "variant": variant})
        (fixtures / "mcp.json").write_text(config, encoding="utf-8")
        (fixtures / "plugin").mkdir()
        (fixtures / "plugin" / ".mcp.json").write_text(config, encoding="utf-8")
    (package / "probe.py").write_text(
        RICH_DEFINITION.replace("MIN", str(custom_min)), encoding="utf-8"
    )
    manifest = json.loads((package / "probe.json").read_text(encoding="utf-8"))
    manifest.update(assertion={"expected": {"a": 2, "b": 2}})
    (package / "probe.json").write_text(json.dumps(manifest), encoding="utf-8")


def _rich_source(tmp_path: Path, *, responses: tuple[str, ...] = ("a", "b")) -> Path:
    """Each arm: one marked main-thread request offering Agent, answering call t1."""
    source = tmp_path / "source"
    for arm in ("a", "b"):
        sink = source / arm / "sink"
        sink.mkdir(parents=True)
        request = claude_request(
            system=f"body {MARKER}",
            model=MODEL,
            tools=[{"name": "Agent"}],
            messages=[
                {
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}],
                },
            ],
        )
        (sink / "0000.request.json").write_text(json.dumps(request), encoding="utf-8")
        if arm in responses:
            response = {
                "content": [{"type": "tool_use", "id": "t1", "name": "mcp__fx__alpha", "input": {}}]
            }
            (sink / "0000.response.json").write_text(json.dumps(response), encoding="utf-8")
        (source / arm / "fx.stamp").write_text("tools/list\n", encoding="utf-8")
    return source


def test_a_rich_definition_wires_every_family_hook(cli: Cli, package: Path, tmp_path: Path) -> None:
    # McpConnected needs an ungoverned request offering Agent; this one is governed, so its
    # dispatch is exercised by the gate refusing below rather than here.
    _rich(package)
    source = _rich_source(tmp_path)
    for arm in ("a", "b"):
        extra = claude_request(system="main thread", model=MODEL, tools=[{"name": "Agent"}])
        (source / arm / "sink" / "0001.request.json").write_text(
            json.dumps(extra), encoding="utf-8"
        )
    env_file = tmp_path / "env.jsonl"
    env = _env(source, env_file, PROBE_FIXTURE="discriminator", PROBE_DRY_RUN="1")
    result = cli("run", "claude-family", env=env)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["status"] == "refuted"  # expected counts 2; arms hold 3

    calls = {c["arm"]: c for c in map(json.loads, env_file.read_text().splitlines())}
    # `PROBE_FIXTURE` selected the discriminator tree for both arms.
    assert calls["a"]["which"] == calls["b"]["which"] == "discriminator"
    # The plugin arm loads its own copy, set up before `arrange_arm`, which ran before claude.
    plugin = calls["b"]["argv"][calls["b"]["argv"].index("--plugin-dir") + 1]
    assert Path(plugin).name == "fxp"
    assert calls["b"]["plugin"] == [".mcp.json", "agents/arranged.md"]
    assert "--mcp-config" in calls["a"]["argv"]
    mcp_config = calls["a"]["argv"][calls["a"]["argv"].index("--mcp-config") + 1]
    assert json.loads(Path(mcp_config).read_text(encoding="utf-8"))["variant"] == "discriminator"
    plugin_config = json.loads((Path(plugin) / ".mcp.json").read_text(encoding="utf-8"))
    assert plugin_config["variant"] == "discriminator"

    order = (package / "order.log").read_text(encoding="utf-8").splitlines()
    # The definition's judge function saw the requests, and the response separately.
    assert order == ["judge saw 2 requests and 1 responses"]
    view_line = next(
        line
        for line in result.stderr.splitlines()
        if line.startswith("probe run: the assembled view is at ")
    )
    view = json.loads(Path(view_line.split(" at ", 1)[1]).read_text(encoding="utf-8"))
    assert len(view["a"]) == 3  # two requests plus the appended response


def test_a_real_run_keeps_the_workspace_with_its_source_map(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    _rich(package)
    source = _rich_source(tmp_path)
    for arm in ("a", "b"):
        extra = claude_request(system="main thread", model=MODEL, tools=[{"name": "Agent"}])
        (source / arm / "sink" / "0001.request.json").write_text(
            json.dumps(extra), encoding="utf-8"
        )
    result = cli("run", "claude-family", env=_env(source, tmp_path / "env.jsonl"))
    assert result.returncode == 0, result.stderr
    assert len(list((package / "results").glob("*.json"))) == 1
    (ws,) = workspaces("claude-family")
    assert f"probe run: the workspace is kept at {ws}\n" in result.stderr
    assert (ws / "source.json").is_file()


def test_a_gate_other_than_model_is_dispatched(cli: Cli, package: Path, tmp_path: Path) -> None:
    # Without an ungoverned request offering Agent, McpConnected refuses.
    _rich(package)
    result = cli("run", "claude-family", env=_env(_rich_source(tmp_path), tmp_path / "env.jsonl"))
    assert result.returncode == 1
    assert "probe run: an arm's main thread did not list Agent" in result.stderr


def test_a_custom_check_reading_the_marker_can_refuse(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    _rich(package, custom_min=5)
    source = _rich_source(tmp_path)
    for arm in ("a", "b"):
        extra = claude_request(system="main thread", model=MODEL, tools=[{"name": "Agent"}])
        (source / arm / "sink" / "0001.request.json").write_text(
            json.dumps(extra), encoding="utf-8"
        )
    result = cli("run", "claude-family", env=_env(source, tmp_path / "env.jsonl"))
    assert result.returncode == 1
    assert "probe run: the custom check refused." in result.stderr


def test_an_arm_without_responses_is_refused_when_responses_join_the_view(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    _rich(package)
    # Arm b has no response, but ToolAnswered only reads arm a, so the append is what refuses.
    source = _rich_source(tmp_path, responses=("a",))
    for arm in ("a", "b"):
        extra = claude_request(system="main thread", model=MODEL, tools=[{"name": "Agent"}])
        (source / arm / "sink" / "0001.request.json").write_text(
            json.dumps(extra), encoding="utf-8"
        )
    result = cli("run", "claude-family", env=_env(source, tmp_path / "env.jsonl"))
    assert result.returncode == 1
    assert "probe run: the b arm captured no response files in " in result.stderr
