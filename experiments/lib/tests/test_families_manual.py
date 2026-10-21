"""The manual family: its runner, and `manual.judge` over a captured workspace.

Refusals that cost no session, a dry run that writes no record, and the Cursor package's
capture handling driven with hand-written payloads. A committed package a test drives is
copied into the test's own tree, so a test can never write into the tracked `results/`
directory: a record the recorder writes is byte-for-byte indistinguishable from a real
measurement.

The black-box tests run a package's definition end to end through `experiments/probe run`,
with `PROBE_EXPERIMENTS_DIR` pointing at the copy.
"""

import io
import json
import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

import pytest
from probe_harness import console, record
from probe_harness.contract import Manifest, parse_manifest
from probe_harness.families import manual
from probe_harness.families.base import GateFailure
from probe_harness.families.manual import ManualProbe

from tests.conftest import (
    CURSOR_PACKAGE_NAME,
    EXPERIMENTS_DIR,
    Cli,
    Result,
    make_capture_workspace,
    workspaces,
)


def record_in_process(manifest: Manifest, ws: Path) -> int:
    """Judge <ws> as a live run with a terminal does, then record it; 1 on a refusal."""
    try:
        result = manual.judge(manifest, ws, ManualProbe(instructions=""), None, interactive=True)
        record.write(record.compute(manifest, ws, result, dry_run=False))
    except (GateFailure, record.RecordRefused) as e:
        console.say_refusal(e)
        return 1
    return 0


def put_capture_package(tmp_path: Path, manifest: str) -> tuple[Path, Path]:
    """A package carrying <manifest> and a workspace holding one payload: `(pkg, ws)`."""
    pkg = tmp_path / "pkg"
    pkg.mkdir()
    (pkg / "probe.json").write_text(manifest, encoding="utf-8")
    ws = make_capture_workspace(tmp_path / "ws", '{"marker":"seen"}\n')
    return pkg, ws


def test_the_manual_judge_prompts_when_the_assertion_declares_options(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    # The branch reads the assertion's shape, not the driver — the recorder gates the
    # correspondence that makes that read safe. Reaching `prompt_selection`, whose tty guard
    # exits 2, is what proves the options path was taken rather than the projection one.
    pkg, ws = put_capture_package(
        tmp_path,
        """{
  "schema_version": 1, "provider": "cursor", "driver": "manual", "depth": null,
  "question": "Which markers appeared?", "version_source": { "kind": "none" },
  "wait_for": "length > 0",
  "assertion": {
    "options": [
      { "id": "neither", "text": "Neither marker was visible" },
      { "id": "couldnt-tell", "text": "Could not determine", "status": "inconclusive" }
    ],
    "expected": "neither"
  }
}
""",
    )
    monkeypatch.setattr("sys.stdin", io.StringIO(""))

    with pytest.raises(SystemExit) as exc:
        record_in_process(parse_manifest(pkg / "probe.json"), ws)
    assert exc.value.code == 2
    captured = capsys.readouterr()
    assert "stdin is not a terminal" in captured.out + captured.err
    assert "Which markers appeared?" in captured.out + captured.err


def test_the_manual_judge_projects_when_the_assertion_declares_a_projection(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # The other half of the same branch. A `manual` manifest may still be machine-answered —
    # which is what `claude-session-start` is — so the driver alone cannot decide this.
    pkg, ws = put_capture_package(
        tmp_path,
        """{
  "schema_version": 1, "provider": "cursor", "driver": "manual", "depth": null,
  "question": "q", "version_source": { "kind": "none" }, "wait_for": "length > 0",
  "assertion": { "projection": "[.[] | .marker] | first", "expected": "seen" }
}
""",
    )
    monkeypatch.setattr("sys.stdin", io.StringIO(""))

    assert record_in_process(parse_manifest(pkg / "probe.json"), ws) == 0
    records = list((pkg / "results").glob("*.json"))
    assert len(records) == 1
    assert json.loads(records[0].read_text(encoding="utf-8"))["status"] == "confirmed"


DUMP_HOOK = EXPERIMENTS_DIR / "cursor-subagent-effort" / "fixtures" / "capture" / "dump-hook.sh"


MANUAL_PROBE_PY = """\
from probe_harness.families.manual import ManualProbe

PROBE = ManualProbe(instructions="Open the workspace and run the session.")
"""


def _put_manual(
    tree: Path,
    name: str,
    *,
    wait_for: str | None,
    version_source: dict[str, Any] | None = None,
) -> Path:
    package = tree / name
    capture = package / "fixtures" / "capture"
    capture.mkdir(parents=True)
    shutil.copy2(DUMP_HOOK, capture / "dump-hook.sh")
    manifest: dict[str, Any] = {
        "schema_version": 1,
        "provider": "cursor",
        "driver": "manual",
        "depth": None,
        "question": "q",
        "version_source": version_source or {"kind": "none"},
        "assertion": {"projection": ".[0].x", "expected": 1},
    }
    if wait_for is not None:
        manifest["wait_for"] = wait_for
    (package / "probe.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    (package / "probe.py").write_text(MANUAL_PROBE_PY, encoding="utf-8")
    return package


def test_a_manual_manifest_without_wait_for_fails_before_the_workspace_is_ready(
    cli: Cli, tree: Path
) -> None:
    _put_manual(tree, "manual-stub", wait_for=None)

    result = cli("run", "manual-stub")

    assert result.returncode == 1, result.output
    assert "manifest declares no wait_for filter" in result.output
    assert "Workspace ready" not in result.output
    assert workspaces("manual-stub") == []


def test_a_dry_run_with_a_matching_capture_writes_no_record(tree: Path) -> None:
    package = _put_manual(tree, "manual-stub", wait_for="length > 0")

    process = subprocess.Popen(
        [str(EXPERIMENTS_DIR / "probe"), "run", "manual-stub"],
        env={**os.environ, "PROBE_DRY_RUN": "1", "PROBE_TIMEOUT_SECONDS": "60"},
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    )
    try:
        # Seed the capture as the provider's hook would, once Arrange has created it. The
        # payload lands whole via a rename, so the poll never reads a partial line.
        capture = _await_capture_dir("manual-stub", deadline=time.monotonic() + 30)
        staged = capture / "payloads.jsonl.tmp"
        staged.write_text('{"x":1}\n', encoding="utf-8")
        staged.replace(capture / "payloads.jsonl")

        stdout, stderr = process.communicate(timeout=60)
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate()

    assert process.returncode == 0, stdout + stderr
    assert "Workspace ready" in stderr
    assert "probe run: dry run" in stderr
    assert json.loads(stdout)["status"] == "confirmed"
    results = package / "results"
    assert not results.exists() or not any(results.iterdir())


def test_a_command_sourced_run_records_the_tool_version_before_the_instructions(
    tree: Path, tmp_path: Path
) -> None:
    stub = tmp_path / "version-stub"
    stub.write_text("#!/bin/sh\nprintf '%s\\n' '4.5.6'\n", encoding="utf-8")
    stub.chmod(0o755)
    _put_manual(
        tree,
        "manual-stub",
        wait_for="length > 0",
        version_source={"kind": "command", "command": str(stub)},
    )
    outfile = tmp_path / "probe.out"
    with outfile.open("w", encoding="utf-8") as out:
        runner = subprocess.Popen(
            [str(EXPERIMENTS_DIR / "probe"), "run", "manual-stub"],
            env={**os.environ, "PROBE_TIMEOUT_SECONDS": "30"},
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=subprocess.STDOUT,
        )
    try:
        project = _wait_for_project(outfile)
        assert project is not None
        # The instructions are out, so the version must already be on disk.
        assert (project.parent / "tool_version").read_text(encoding="utf-8") == "4.5.6\n"
    finally:
        runner.kill()
        runner.wait()


def _await_capture_dir(name: str, *, deadline: float) -> Path:
    while time.monotonic() < deadline:
        for ws in workspaces(name):
            # The capture script is templated in after `capture/` is created; once it is
            # present, Arrange has finished with the directory.
            if (ws / "capture" / "dump-hook.sh").is_file():
                return ws / "capture"
        time.sleep(0.1)
    raise AssertionError(f"no workspace with a capture directory appeared for {name}")


def seed_workspace(ws: Path, model: str = "claude-opus-5-thinking-low") -> None:
    """A workspace shaped the way the runner shapes one, carrying a matching payload."""
    (ws / "capture").mkdir(parents=True)
    payload = {
        "hook_event_name": "subagentStart",
        "subagent_type": "arm-effort-low",
        "subagent_model": model,
        "cursor_version": "3.16.17",
    }
    (ws / "capture" / "payloads.jsonl").write_text(
        json.dumps(payload, separators=(",", ":")) + "\n", encoding="utf-8"
    )


def newest_record(package: Path) -> Path | None:
    records = sorted((package / "results").rglob("*.json"))
    return records[-1] if records else None


def read_record(path: Path | None) -> dict[str, object]:
    assert path is not None
    record = json.loads(path.read_text(encoding="utf-8"))
    assert isinstance(record, dict)
    return record


def test_a_capture_resolves_tool_version_from_its_own_cursor_version(
    tmp_path: Path, cursor_package: Path
) -> None:
    # The surviving job of the recorder's `capture`. Four of six manifests declare
    # `version_source.kind: "capture"`, so this is the only path by which a Cursor record
    # gets a version at all — and no CLI can supply it, since `cursor-agent --version`
    # reports a different artifact on a different versioning scheme.
    ws = tmp_path / "ws"
    seed_workspace(ws)

    assert record_in_process(parse_manifest(cursor_package / "probe.json"), ws) == 0

    record = read_record(newest_record(cursor_package))
    assert record["status"] == "confirmed"
    assert record["provider"] == "cursor"
    assert record["tool_version"] == "3.16.17"
    assertion = record["assertion"]
    assert isinstance(assertion, dict)
    assert assertion["observed"] == "claude-opus-5-thinking-low"


def test_a_capture_showing_the_default_model_records_refuted_not_confirmed(
    tmp_path: Path, cursor_package: Path
) -> None:
    # The non-default-value rule in action: the baseline must not pass.
    ws = tmp_path / "ws-default"
    seed_workspace(ws, "claude-opus-5-thinking-high")

    assert record_in_process(parse_manifest(cursor_package / "probe.json"), ws) == 0
    assert read_record(newest_record(cursor_package))["status"] == "refuted"


def test_a_workspace_with_no_payloads_is_refused_and_writes_no_record(
    tmp_path: Path, cursor_package: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    # The one failure a stamp was never needed to catch: the hook never fired.
    ws = tmp_path / "ws-empty"
    (ws / "capture").mkdir(parents=True)

    assert record_in_process(parse_manifest(cursor_package / "probe.json"), ws) != 0
    captured = capsys.readouterr()
    assert "no captured payloads" in captured.out + captured.err
    assert newest_record(cursor_package) is None


def test_a_directory_that_is_not_a_probe_workspace_is_refused(
    tmp_path: Path, cursor_package: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    ws = tmp_path / "not-a-workspace"
    ws.mkdir()

    assert record_in_process(parse_manifest(cursor_package / "probe.json"), ws) != 0
    captured = capsys.readouterr()
    assert "not a probe workspace" in captured.out + captured.err


def test_the_runner_takes_no_arguments(cli: Cli, cursor_package: Path, tmp_path: Path) -> None:
    # There is no resume, so every argument is a mistake, including the `--capture <ws>` an
    # operator may remember from a removed fallback. Silently starting a fresh run would
    # cost them a live session.
    env = _tree_env(cursor_package)
    result = cli("run", CURSOR_PACKAGE_NAME, "--capture", tmp_path, env=env)
    assert result.returncode != 0
    assert "unexpected argument: --capture" in result.output
    assert "Workspace ready" not in result.output

    result = cli("run", CURSOR_PACKAGE_NAME, "--anything", env=env)
    assert result.returncode != 0
    assert "Workspace ready" not in result.output


def test_the_blocking_path_records_without_a_second_invocation(
    cursor_package: Path, tmp_path: Path
) -> None:
    # The central claim: the operator runs one command and the record appears. A writer in
    # this test stands in for the live Cursor session.
    outfile = tmp_path / "probe.out"
    with outfile.open("w", encoding="utf-8") as out:
        runner = subprocess.Popen(
            [str(EXPERIMENTS_DIR / "probe"), "run", CURSOR_PACKAGE_NAME],
            env={**os.environ, **_tree_env(cursor_package), "PROBE_TIMEOUT_SECONDS": "30"},
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=subprocess.STDOUT,
        )
    try:
        project = _wait_for_project(outfile)
        assert project is not None
        ws = project.parent
        assert (project / ".cursor" / "hooks.json").is_file()
        # The apparatus must not be reachable from inside the opened project.
        assert not (project / "capture").exists()
        assert (ws / "capture").is_dir()

        seed = {
            "hook_event_name": "subagentStart",
            "subagent_type": "arm-effort-low",
            "subagent_model": "claude-opus-5-thinking-low",
            "cursor_version": "3.16.17",
        }
        with (ws / "capture" / "payloads.jsonl").open("a", encoding="utf-8") as payloads:
            payloads.write(json.dumps(seed, separators=(",", ":")) + "\n")

        assert runner.wait(timeout=60) == 0
    finally:
        if runner.poll() is None:
            runner.kill()
            runner.wait()
    assert read_record(newest_record(cursor_package))["status"] == "confirmed"
    assert f"probe run: the workspace is kept at {ws}\n" in outfile.read_text(encoding="utf-8")
    assert (ws / "source.json").is_file()


def test_the_generated_workspace_has_no_unsubstituted_placeholder_anywhere(
    cli: Cli, cursor_package: Path
) -> None:
    result = cli(
        "run", CURSOR_PACKAGE_NAME, env={**_tree_env(cursor_package), "PROBE_TIMEOUT_SECONDS": "1"}
    )
    project = _project_of(result)
    ws = project.parent

    hooks = project / ".cursor" / "hooks.json"
    hook = ws / "capture" / "dump-hook.sh"
    assert hooks.is_file()
    assert "{{" not in hooks.read_text(encoding="utf-8")
    assert "{{" not in hook.read_text(encoding="utf-8")

    # The generated hooks.json is valid JSON pointing at an executable hook that writes
    # into this workspace.
    json.loads(hooks.read_text(encoding="utf-8"))
    assert os.access(hook, os.X_OK)
    assert str(ws / "capture" / "payloads.jsonl") in hook.read_text(encoding="utf-8")
    assert (project / ".cursor" / "agents" / "arm-effort-low.md").is_file()


def test_the_capture_apparatus_is_not_reachable_from_inside_the_opened_project(
    cli: Cli, cursor_package: Path
) -> None:
    # A probe whose oracle is the agent's answer is defeated if the agent can read the
    # apparatus: a marker findable by filesystem search makes "the hook injected it" and
    # "the agent grepped for it" indistinguishable.
    result = cli(
        "run", CURSOR_PACKAGE_NAME, env={**_tree_env(cursor_package), "PROBE_TIMEOUT_SECONDS": "1"}
    )
    project = _project_of(result)
    ws = project.parent

    assert (ws / "capture").is_dir()
    assert not (project / "capture").exists()
    # Nothing under the opened project but hooks.json may name the payloads file.
    naming = [
        path
        for path in project.rglob("*")
        if path.is_file()
        and path.name != "hooks.json"
        and "payloads.jsonl" in path.read_text(encoding="utf-8", errors="replace")
    ]
    assert naming == []


def test_a_timeout_leaves_the_workspace_intact_and_says_the_run_is_over(
    cli: Cli, cursor_package: Path
) -> None:
    # Losing a workspace after a live session is the one unrecoverable failure. With no
    # resume path the message must not imply one: the workspace is kept for inspection, not
    # for a second invocation.
    result = cli(
        "run", CURSOR_PACKAGE_NAME, env={**_tree_env(cursor_package), "PROBE_TIMEOUT_SECONDS": "1"}
    )
    assert result.returncode != 0

    project = _project_of(result)
    ws = project.parent
    assert ws.is_dir()
    assert project.is_dir()
    # A prefixed refusal, naming the workspace root, where capture/ lives, not the project.
    assert "probe run: timed out waiting for the capture after 1s\n" in result.stderr
    assert "probe run: this run is over; re-run the probe to try again.\n" in result.stderr
    assert (
        "probe run: if the capture arrived after the deadline, record it with: "
        f"experiments/probe record {CURSOR_PACKAGE_NAME} {ws}\n" in result.stderr
    )
    assert f"probe run: the workspace has been kept for inspection: {ws}\n" in result.stderr
    assert "--capture" not in result.output


def _tree_env(package: Path) -> dict[str, str]:
    return {"PROBE_EXPERIMENTS_DIR": str(package.parent)}


def _project_of(result: Result) -> Path:
    for line in result.output.splitlines():
        if line.startswith("Workspace ready: "):
            return Path(line.removeprefix("Workspace ready: "))
    raise AssertionError(f"no workspace line in:\n{result.output}")


def _wait_for_project(outfile: Path) -> Path | None:
    for _ in range(40):
        for line in outfile.read_text(encoding="utf-8").splitlines():
            if line.startswith("Workspace ready: "):
                return Path(line.removeprefix("Workspace ready: "))
        time.sleep(0.25)
    return None


def test_a_capture_whose_payloads_are_not_json_is_refused_without_a_record(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    pkg, ws = put_capture_package(
        tmp_path,
        """{
  "schema_version": 1, "provider": "cursor", "driver": "manual", "depth": null,
  "question": "q", "version_source": { "kind": "none" }, "wait_for": "length > 0",
  "assertion": { "projection": ".", "expected": 1 }
}
""",
    )
    (ws / "capture" / "payloads.jsonl").write_text("{not json\n", encoding="utf-8")

    assert record_in_process(parse_manifest(pkg / "probe.json"), ws) == 1
    captured = capsys.readouterr()
    assert "the captured payloads at" in captured.err
    assert "are not valid JSON" in captured.err
    assert not (pkg / "results").exists()
