"""The OpenCode fake-provider family, driven by a stub `opencode` first on `PATH`.

The stub stands in for `opencode run`: it reads the arm's generated `opencode.json`, spawns
each configured MCP server and sends it `initialize` and `tools/list` (unless `STUB_NO_MCP`
is set), then POSTs the request bodies in the JSON file named by `STUB_BODIES` to the fake
provider's endpoint, as OpenCode would. A reply carrying a tool call is answered with a
follow-up request holding a `tool` message, as OpenCode answers one. `--version` prints 9.9.9.

Side channels: `STUB_REPLIES` names a file each reply is appended to, `STUB_PWD_FILE` one the
stub writes its `PWD` and working directory to, and `STUB_EXIT` makes it exit with that code
before contacting anything. `STUB_HANG` names a file the stub creates before spinning
forever, so a test can interrupt a run mid-arm.
"""

import json
import os
import signal
import subprocess
import textwrap
import time
from pathlib import Path
from typing import Any

import pytest

from tests.conftest import (
    EXPERIMENTS_DIR,
    Cli,
    workspaces,
)

MARKER = "AGENTSPEC-PROBE-MARKER-FAKETEST"


STUB = textwrap.dedent("""\
    #!/usr/bin/env python3
    import json
    import os
    import subprocess
    import sys
    import urllib.request

    if sys.argv[1:] == ["--version"]:
        print("9.9.9")
        sys.exit(0)
    if os.environ.get("STUB_PWD_FILE"):
        with open(os.environ["STUB_PWD_FILE"], "a") as f:
            f.write(os.environ.get("PWD", "") + "\\t" + os.getcwd() + "\\n")
    if os.environ.get("STUB_EXIT"):
        print("stub failing", file=sys.stderr)
        sys.exit(int(os.environ["STUB_EXIT"]))
    if os.environ.get("STUB_HANG"):
        open(os.environ["STUB_HANG"], "w").close()
        while True:
            pass
    config = json.load(open("opencode.json"))
    if not os.environ.get("STUB_NO_MCP"):
        for server in config.get("mcp", {}).values():
            messages = [
                {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
                {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            ]
            lines = "".join(json.dumps(m) + "\\n" for m in messages)
            subprocess.run(server["command"], input=lines, capture_output=True, text=True,
                           check=True)
    base = config["provider"]["fake"]["options"]["baseURL"]

    def post(body):
        request = urllib.request.Request(
            base + "/chat/completions", data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json"}, method="POST",
        )
        reply = urllib.request.urlopen(request).read().decode()
        if os.environ.get("STUB_REPLIES"):
            with open(os.environ["STUB_REPLIES"], "a") as f:
                f.write(reply)
        return reply

    for body in json.load(open(os.environ["STUB_BODIES"])):
        if "tool_calls" in post(body):
            post({"messages": body["messages"] + [{"role": "tool", "content": "result"}]})
""")


OPENCODE_JSON = {
    "provider": {"fake": {"options": {"baseURL": "http://127.0.0.1:{{PORT}}/v1"}}},
    "mcp": {
        "fx": {
            "type": "local",
            "command": ["{{PYTHON}}", "{{FX_SERVER}}", "--stamp", "{{FX_STAMP}}"],
        }
    },
}


DEFINITION = textwrap.dedent(f"""\
    from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe

    PROBE = OpenCodeFakeProbe(
        marker="{MARKER}",
        arms=(OpenCodeArm("one", "agent-one"), OpenCodeArm("two", "agent-two")),
        fx_servers=("fx",),
    )
""")


MANIFEST = {
    "schema_version": 1,
    "provider": "opencode",
    "driver": "unattended",
    "depth": "outbound-request",
    "question": "q",
    "version_source": {"kind": "command", "command": "opencode --version"},
    "assertion": {
        "projection": "{one: (.one | length), two: (.two | length)}",
        "expected": {"one": 1, "two": 1},
    },
}


@pytest.fixture
def package(tree: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    stub = bin_dir / "opencode"
    stub.write_text(STUB, encoding="utf-8")
    stub.chmod(0o755)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ['PATH']}")

    package = tree / "fake-family"
    (package / "fixtures" / "project").mkdir(parents=True)
    (package / "fixtures" / "project" / "opencode.json").write_text(
        json.dumps(OPENCODE_JSON, indent=2), encoding="utf-8"
    )
    (package / "probe.json").write_text(json.dumps(MANIFEST), encoding="utf-8")
    (package / "probe.py").write_text(DEFINITION, encoding="utf-8")
    return package


def _bodies(tmp_path: Path, system: str) -> str:
    bodies: list[dict[str, Any]] = [
        {"messages": [{"role": "system", "content": system}, {"role": "user", "content": "hi"}]}
    ]
    path = tmp_path / "bodies.json"
    path.write_text(json.dumps(bodies), encoding="utf-8")
    return str(path)


def _no_fake_running(scope: Path) -> bool:
    """No fake provider is running with a sink under <scope>, this test's own directory."""
    found = subprocess.run(
        ["pgrep", "-f", f"fake_provider.py {scope}"], capture_output=True, check=False
    )
    return found.returncode == 1


def test_a_two_arm_definition_records_under_a_dry_run(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    result = cli(
        "run",
        "fake-family",
        env={"STUB_BODIES": _bodies(tmp_path, f"body {MARKER}"), "PROBE_DRY_RUN": "1"},
    )
    assert result.returncode == 0, result.stderr
    record = json.loads(result.stdout)
    assert record["status"] == "confirmed"
    assert record["tool_version"] == "9.9.9"
    assert "probe run: the assembled view is at " in result.stderr
    assert not (package / "results").exists()
    assert _no_fake_running(tmp_path)


def test_bodies_without_the_marker_are_refused(cli: Cli, package: Path, tmp_path: Path) -> None:
    result = cli("run", "fake-family", env={"STUB_BODIES": _bodies(tmp_path, "plain")})
    assert result.returncode == 1
    assert f"probe run: arm one: no request carried {MARKER} in a system message." in result.stderr
    assert "probe run: the workspace has been kept for inspection: " in result.stderr
    assert _no_fake_running(tmp_path)


def test_an_mcp_server_that_never_answered_is_refused(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    result = cli(
        "run", "fake-family", env={"STUB_BODIES": _bodies(tmp_path, MARKER), "STUB_NO_MCP": "1"}
    )
    assert result.returncode == 1
    assert "the fx server never answered tools/list" in result.stderr
    assert _no_fake_running(tmp_path)


def test_a_real_run_records_and_keeps_the_workspace(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    result = cli("run", "fake-family", env={"STUB_BODIES": _bodies(tmp_path, MARKER)})
    assert result.returncode == 0, result.stderr
    assert len(list((package / "results").glob("*.json"))) == 1
    (ws,) = workspaces("fake-family")
    assert f"probe run: the workspace is kept at {ws}\n" in result.stderr
    assert (ws / "source.json").is_file()
    assert _no_fake_running(tmp_path)


def test_a_run_leaves_no_requests_json_in_any_arm(cli: Cli, package: Path, tmp_path: Path) -> None:
    run = cli(
        "run", "fake-family", env={"STUB_BODIES": _bodies(tmp_path, MARKER), "PROBE_DRY_RUN": "1"}
    )
    assert run.returncode == 0, run.stderr
    (ws,) = workspaces("fake-family")
    assert (ws / "view.json").is_file()
    assert list(ws.glob("*/requests.json")) == []


def test_replay_judges_a_kept_fake_provider_workspace(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    run = cli(
        "run", "fake-family", env={"STUB_BODIES": _bodies(tmp_path, MARKER), "PROBE_DRY_RUN": "1"}
    )
    assert run.returncode == 0, run.stderr
    (ws,) = workspaces("fake-family")
    (ws / "two" / "fx.stamp").unlink()
    replayed = cli("replay", "fake-family", ws)
    assert replayed.returncode == 1
    assert "probe replay: arm two: the fx server never answered tools/list" in replayed.stderr


def test_a_provider_exiting_nonzero_stops_the_stub_and_keeps_the_workspace(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    result = cli(
        "run", "fake-family", env={"STUB_BODIES": _bodies(tmp_path, MARKER), "STUB_EXIT": "4"}
    )
    assert result.returncode == 1
    assert 'probe run: arm one: "opencode run --agent agent-one" exited nonzero' in result.stderr
    assert "stub failing" in result.stderr
    assert "probe run: the workspace has been kept for inspection: " in result.stderr
    assert _no_fake_running(tmp_path)


def test_a_template_failure_mid_arm_stops_the_stub(cli: Cli, package: Path, tmp_path: Path) -> None:
    config = package / "fixtures" / "project" / "opencode.json"
    text = config.read_text(encoding="utf-8").replace('"fake"', '"{{UNKNOWN}}"', 1)
    config.write_text(text, encoding="utf-8")
    result = cli("run", "fake-family", env={"STUB_BODIES": _bodies(tmp_path, MARKER)})
    assert result.returncode == 1
    assert "probe run: unsubstituted placeholder left in" in result.stderr
    assert "probe run: the workspace has been kept for inspection: " in result.stderr
    assert _no_fake_running(tmp_path)


def test_the_provider_runs_in_the_arm_project_with_a_matching_pwd(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    # OpenCode finds its project through `PWD`; a stale one points it elsewhere.
    pwd_file = tmp_path / "pwd.txt"
    env = {
        "STUB_BODIES": _bodies(tmp_path, MARKER),
        "STUB_PWD_FILE": str(pwd_file),
        "PROBE_DRY_RUN": "1",
    }
    result = cli("run", "fake-family", env=env)
    assert result.returncode == 0, result.stderr
    rows = [line.split("\t") for line in pwd_file.read_text(encoding="utf-8").splitlines()]
    assert len(rows) == 2
    for pwd, cwd in rows:
        assert Path(pwd).resolve() == Path(cwd).resolve()
        assert Path(pwd).name == "project"


SCRIPTED = """\
from pathlib import Path

from probe_harness.families.base import CustomCheck
from probe_harness.families.opencode_fake import OpenCodeArm, OpenCodeFakeProbe, ScriptedCall
from probe_harness.gates.opencode import GOVERNED_JQ


def _arrange_arm(package: Path, arm_dir: Path, arm: OpenCodeArm) -> ScriptedCall | None:
    return ScriptedCall("listing", '{"b": 2}') if arm.name == "two" else None


PROBE = OpenCodeFakeProbe(
    marker="MARKER_VALUE",
    arms=(
        OpenCodeArm("one", "agent-one", ONE_CALL),
        OpenCodeArm("two", "agent-two"),
    ),
    fx_servers=("fx",),
    tool_result_gate=True,
    custom_checks=(
        CustomCheck(
            expression=f"[.one[] | select(any({GOVERNED_JQ}))] | length >= CHECK_MIN",
            failure=("the custom check refused.",),
        ),
    ),
    arrange_arm=ARRANGE,
)
"""


def _scripted(package: Path, *, check_min: int = 1, scripted: bool = True) -> None:
    source = (
        SCRIPTED.replace("MARKER_VALUE", MARKER)
        .replace("CHECK_MIN", str(check_min))
        .replace("ONE_CALL", """ScriptedCall("reading", '{"a":1}')""" if scripted else "None")
        .replace("ARRANGE", "_arrange_arm" if scripted else "None")
    )
    (package / "probe.py").write_text(source, encoding="utf-8")
    manifest = json.loads((package / "probe.json").read_text(encoding="utf-8"))
    manifest["assertion"]["expected"] = {"one": 2, "two": 2}
    (package / "probe.json").write_text(json.dumps(manifest), encoding="utf-8")


def test_scripted_calls_reach_the_stub_and_the_tool_result_gate_passes(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    _scripted(package)
    replies = tmp_path / "replies.txt"
    env = {
        "STUB_BODIES": _bodies(tmp_path, MARKER),
        "STUB_REPLIES": str(replies),
        "PROBE_DRY_RUN": "1",
    }
    result = cli("run", "fake-family", env=env)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["status"] == "confirmed"
    text = replies.read_text(encoding="utf-8")
    # The arm's own call, and the call `arrange_arm` substituted for the other arm.
    assert '"name": "reading", "arguments": "{\\"a\\":1}"' in text
    assert '"name": "listing", "arguments": "{\\"b\\": 2}"' in text


def test_a_custom_check_reading_the_marker_can_refuse(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    _scripted(package, check_min=3)
    result = cli("run", "fake-family", env={"STUB_BODIES": _bodies(tmp_path, MARKER)})
    assert result.returncode == 1
    assert "probe run: the custom check refused." in result.stderr
    assert _no_fake_running(tmp_path)


def test_without_a_scripted_call_the_tool_result_gate_refuses(
    cli: Cli, package: Path, tmp_path: Path
) -> None:
    _scripted(package, scripted=False)
    result = cli("run", "fake-family", env={"STUB_BODIES": _bodies(tmp_path, MARKER)})
    assert result.returncode == 1
    assert "probe run: arm one: no marked request carried a tool result." in result.stderr


def test_a_terminated_run_stops_the_stub_and_names_the_workspace(
    package: Path, tmp_path: Path
) -> None:
    hanging = tmp_path / "hanging"
    runner = subprocess.Popen(
        [str(EXPERIMENTS_DIR / "probe"), "run", "fake-family"],
        env={**os.environ, "STUB_BODIES": _bodies(tmp_path, MARKER), "STUB_HANG": str(hanging)},
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        for _ in range(200):
            if hanging.exists():
                break
            time.sleep(0.05)
        assert hanging.exists()
        assert not _no_fake_running(tmp_path)
        runner.send_signal(signal.SIGTERM)
        _, stderr = runner.communicate(timeout=30)
    finally:
        if runner.poll() is None:
            runner.kill()
            runner.wait()
    assert runner.returncode == 128 + signal.SIGTERM
    assert "probe run: the workspace has been kept for inspection: " in stderr
    assert _no_fake_running(tmp_path)
