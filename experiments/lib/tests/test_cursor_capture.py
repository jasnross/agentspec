"""Coverage for the Cursor probe's capture handling, driven with hand-written payloads.

Ported from `cursor-capture.bats`. The package is copied into the test's own tree and the
copy is driven, so a test can never write into the tracked `results/` directory: a record
the recorder writes is byte-for-byte indistinguishable from a real measurement.

The black-box tests run the package's definition end to end through `experiments/probe
run`, with `PROBE_EXPERIMENTS_DIR` pointing at the copy.
"""

import json
import os
import shutil
import subprocess
import time
from pathlib import Path

import pytest
from probe_harness import jq_cli
from probe_harness.families import manual
from probe_harness.workspace import load_json_values, parse_json_values, template_file

from tests.conftest import EXPERIMENTS_DIR, Cli, Result

PACKAGE_NAME = "cursor-subagent-effort"


@pytest.fixture
def package(tmp_path: Path, experiments_dir: Path) -> Path:
    """A copy of the Cursor package with no `results/`, inside the test's own tree."""
    copy = tmp_path / "experiments" / PACKAGE_NAME
    shutil.copytree(experiments_dir / PACKAGE_NAME, copy, symlinks=True)
    shutil.rmtree(copy / "results", ignore_errors=True)
    return copy


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


def install_hook(package: Path, ws: Path, *, jq: str, payloads: Path) -> Path:
    """Template the capture hook into <ws> the way the runner does, and make it executable."""
    (ws / "capture").mkdir(parents=True)
    hook = ws / "capture" / "dump-hook.sh"
    template_file(
        package / "fixtures" / "capture" / "dump-hook.sh", hook, JQ=jq, PAYLOADS=str(payloads)
    )
    hook.chmod(hook.stat().st_mode | 0o111)
    return hook


def run_hook(hook: Path, payload: str) -> subprocess.CompletedProcess[str]:
    """Pipe <payload> into <hook>, with stderr merged into stdout as bats' `run` does."""
    return subprocess.run(
        [str(hook)],
        input=payload,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        timeout=30,
        check=False,
    )


def system_jq() -> str:
    jq = shutil.which("jq")
    assert jq is not None
    return jq


def line_count(path: Path) -> int:
    return path.read_text(encoding="utf-8").count("\n")


def test_a_capture_resolves_tool_version_from_its_own_cursor_version(
    tmp_path: Path, package: Path
) -> None:
    # The surviving job of the recorder's `capture`. Four of six manifests declare
    # `version_source.kind: "capture"`, so this is the only path by which a Cursor record
    # gets a version at all — and no CLI can supply it, since `cursor-agent --version`
    # reports a different artifact on a different versioning scheme.
    ws = tmp_path / "ws"
    seed_workspace(ws)

    assert manual.record_capture(package, ws, dry_run=False) == 0

    record = read_record(newest_record(package))
    assert record["status"] == "confirmed"
    assert record["provider"] == "cursor"
    assert record["tool_version"] == "3.16.17"
    assertion = record["assertion"]
    assert isinstance(assertion, dict)
    assert assertion["observed"] == "claude-opus-5-thinking-low"


def test_a_capture_showing_the_default_model_records_refuted_not_confirmed(
    tmp_path: Path, package: Path
) -> None:
    # The non-default-value rule in action: the baseline must not pass.
    ws = tmp_path / "ws-default"
    seed_workspace(ws, "claude-opus-5-thinking-high")

    assert manual.record_capture(package, ws, dry_run=False) == 0
    assert read_record(newest_record(package))["status"] == "refuted"


def test_a_workspace_with_no_payloads_is_refused_and_writes_no_record(
    tmp_path: Path, package: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    # The one failure a stamp was never needed to catch: the hook never fired.
    ws = tmp_path / "ws-empty"
    (ws / "capture").mkdir(parents=True)

    assert manual.record_capture(package, ws, dry_run=False) != 0
    captured = capsys.readouterr()
    assert "no captured payloads" in captured.out + captured.err
    assert newest_record(package) is None


def test_a_directory_that_is_not_a_probe_workspace_is_refused(
    tmp_path: Path, package: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    ws = tmp_path / "not-a-workspace"
    ws.mkdir()

    assert manual.record_capture(package, ws, dry_run=False) != 0
    captured = capsys.readouterr()
    assert "not a probe workspace" in captured.out + captured.err


def test_the_runner_takes_no_arguments(cli: Cli, package: Path, tmp_path: Path) -> None:
    # There is no resume, so every argument is a mistake, including the `--capture <ws>` an
    # operator may remember from a removed fallback. Silently starting a fresh run would
    # cost them a live session.
    env = _tree_env(package)
    result = cli("run", PACKAGE_NAME, "--capture", tmp_path, env=env)
    assert result.returncode != 0
    assert "unexpected argument: --capture" in result.output
    assert "Workspace ready" not in result.output

    result = cli("run", PACKAGE_NAME, "--anything", env=env)
    assert result.returncode != 0
    assert "Workspace ready" not in result.output


def test_the_blocking_path_records_without_a_second_invocation(
    package: Path, tmp_path: Path
) -> None:
    # The central claim: the operator runs one command and the record appears. A writer in
    # this test stands in for the live Cursor session.
    outfile = tmp_path / "probe.out"
    with outfile.open("w", encoding="utf-8") as out:
        runner = subprocess.Popen(
            [str(EXPERIMENTS_DIR / "probe"), "run", PACKAGE_NAME],
            env={**os.environ, **_tree_env(package), "PROBE_TIMEOUT_SECONDS": "30"},
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
    assert read_record(newest_record(package))["status"] == "confirmed"


def test_the_generated_workspace_has_no_unsubstituted_placeholder_anywhere(
    cli: Cli, package: Path
) -> None:
    result = cli("run", PACKAGE_NAME, env={**_tree_env(package), "PROBE_TIMEOUT_SECONDS": "1"})
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
    cli: Cli, package: Path
) -> None:
    # A probe whose oracle is the agent's answer is defeated if the agent can read the
    # apparatus: a marker findable by filesystem search makes "the hook injected it" and
    # "the agent grepped for it" indistinguishable.
    result = cli("run", PACKAGE_NAME, env={**_tree_env(package), "PROBE_TIMEOUT_SECONDS": "1"})
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
    cli: Cli, package: Path
) -> None:
    # Losing a workspace after a live session is the one unrecoverable failure. With no
    # resume path the message must not imply one: the workspace is kept for inspection, not
    # for a second invocation.
    result = cli("run", PACKAGE_NAME, env={**_tree_env(package), "PROBE_TIMEOUT_SECONDS": "1"})
    assert result.returncode != 0

    project = _project_of(result)
    ws = project.parent
    assert ws.is_dir()
    assert project.is_dir()
    # The message names the workspace root, where capture/ lives, not the project.
    assert f"kept for inspection: {ws}" in result.output
    assert "re-run the probe" in result.output
    assert "--capture" not in result.output


def test_the_capture_hook_appends_one_json_line_and_echoes(tmp_path: Path, package: Path) -> None:
    ws = tmp_path / "hookws"
    payloads = ws / "capture" / "payloads.jsonl"
    hook = install_hook(package, ws, jq=system_jq(), payloads=payloads)

    result = run_hook(hook, '{"hook_event_name":"subagentStart"}')
    assert result.returncode == 0
    assert parse_json_values(result.stdout) == [{}]
    values = load_json_values(payloads)
    assert [value["hook_event_name"] for value in values] == ["subagentStart"]
    # One line, one JSON value — what every later slurping reader depends on.
    assert line_count(payloads) == 1


def test_the_capture_hook_still_echoes_and_exits_0_when_jq_is_missing(
    tmp_path: Path, package: Path
) -> None:
    # Its contract inside a live editor is absolute. A failure here must be invisible to
    # Cursor and caught downstream by the recorder refusing an empty capture — never a
    # nonzero exit that breaks the session.
    ws = tmp_path / "hookws-nojq"
    hook = install_hook(
        package, ws, jq="/nonexistent/jq", payloads=ws / "capture" / "payloads.jsonl"
    )

    result = run_hook(hook, '{"a":1}')
    assert result.returncode == 0
    assert parse_json_values(result.stdout) == [{}]


def test_the_capture_hook_echoes_and_writes_nothing_on_an_empty_payload(
    tmp_path: Path, package: Path
) -> None:
    # An empty read must not append a blank line: that would pass the recorder's non-empty
    # check and then break the slurp for every later reader.
    ws = tmp_path / "hookws-empty"
    payloads = ws / "capture" / "payloads.jsonl"
    hook = install_hook(package, ws, jq=system_jq(), payloads=payloads)

    result = run_hook(hook, "")
    assert result.returncode == 0
    assert parse_json_values(result.stdout) == [{}]
    assert not (payloads.is_file() and payloads.stat().st_size > 0)


def test_an_unparsable_payload_is_still_appended_as_one_line(tmp_path: Path, package: Path) -> None:
    # payloads.jsonl is one JSON value per line. A pretty-printed fallback would break the
    # slurp for every later reader, including the recorder.
    ws = tmp_path / "hookws-unparsable"
    payloads = ws / "capture" / "payloads.jsonl"
    hook = install_hook(package, ws, jq=system_jq(), payloads=payloads)

    result = run_hook(hook, "not json at all")
    assert result.returncode == 0

    assert line_count(payloads) == 1
    assert jq_cli.test('length == 1 and (.[0].unparsed | type == "string")', payloads, slurp=True)


def test_a_scalar_payload_is_wrapped_rather_than_appended_verbatim(
    tmp_path: Path, package: Path
) -> None:
    # Every line of payloads.jsonl must be an object. A bare scalar would be valid JSON and
    # land verbatim, and then every later slurping filter indexing a field errors on it —
    # which the capture poll reads as "no match", so the runner polls to the full timeout on
    # a capture that already arrived.
    ws = tmp_path / "hookws-scalar"
    payloads = ws / "capture" / "payloads.jsonl"
    hook = install_hook(package, ws, jq=system_jq(), payloads=payloads)

    result = run_hook(hook, '"just a string"')
    assert result.returncode == 0

    assert line_count(payloads) == 1
    assert jq_cli.test('length == 1 and (.[0] | type == "object")', payloads, slurp=True)
    # A filter indexing a field must not error on it.
    assert jq_cli.test('any(.[]; .hook_event_name == "nope") | not', payloads, slurp=True)


def test_the_capture_hook_echoes_when_its_payloads_path_is_unwritable(
    tmp_path: Path, package: Path
) -> None:
    ws = tmp_path / "hookws-unwritable"
    hook = install_hook(
        package, ws, jq=system_jq(), payloads=ws / "no" / "such" / "dir" / "payloads.jsonl"
    )

    result = run_hook(hook, '{"a":1}')
    assert result.returncode == 0
    assert parse_json_values(result.stdout) == [{}]


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
