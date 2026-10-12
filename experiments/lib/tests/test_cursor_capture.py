"""Coverage for the Cursor probe's capture handling, driven with hand-written payloads.

Ported from `cursor-capture.bats`. The package is copied into the test's own tree and the
copy is driven, so a test can never write into the tracked `results/` directory: a record
the recorder writes is byte-for-byte indistinguishable from a real measurement.

The five black-box tests that run the package's runner end to end (no arguments accepted,
the blocking path, placeholder-free workspace, unreachable apparatus, timeout) land with the
manual definitions, once the package carries a Python one.
"""

import json
import shutil
import subprocess
from pathlib import Path

import pytest
from probe_harness import jq_cli
from probe_harness.families import manual
from probe_harness.workspace import load_json_values, parse_json_values, template_file

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
