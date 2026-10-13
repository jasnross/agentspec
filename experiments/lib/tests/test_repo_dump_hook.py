"""The Cursor package's capture hook, `fixtures/capture/dump-hook.sh`.

The hook is templated into a workspace the way the runner templates it, then driven with
hand-written payloads.
"""

import shutil
import subprocess
from pathlib import Path

from probe_harness import jq_cli
from probe_harness.json_values import load_stream
from probe_harness.workspace import template_file


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


def test_the_capture_hook_appends_one_json_line_and_echoes(
    tmp_path: Path, cursor_package: Path
) -> None:
    ws = tmp_path / "hookws"
    payloads = ws / "capture" / "payloads.jsonl"
    hook = install_hook(cursor_package, ws, jq=system_jq(), payloads=payloads)

    result = run_hook(hook, '{"hook_event_name":"subagentStart"}')
    assert result.returncode == 0
    assert load_stream(result.stdout) == [{}]
    values = load_stream(payloads.read_text(encoding="utf-8"))
    assert values == [{"hook_event_name": "subagentStart"}]
    # One line, one JSON value — what every later slurping reader depends on.
    assert line_count(payloads) == 1


def test_the_capture_hook_still_echoes_and_exits_0_when_jq_is_missing(
    tmp_path: Path, cursor_package: Path
) -> None:
    # Its contract inside a live editor is absolute. A failure here must be invisible to
    # Cursor and caught downstream by the recorder refusing an empty capture — never a
    # nonzero exit that breaks the session.
    ws = tmp_path / "hookws-nojq"
    hook = install_hook(
        cursor_package, ws, jq="/nonexistent/jq", payloads=ws / "capture" / "payloads.jsonl"
    )

    result = run_hook(hook, '{"a":1}')
    assert result.returncode == 0
    assert load_stream(result.stdout) == [{}]


def test_the_capture_hook_echoes_and_writes_nothing_on_an_empty_payload(
    tmp_path: Path, cursor_package: Path
) -> None:
    # An empty read must not append a blank line: that would pass the recorder's non-empty
    # check and then break the slurp for every later reader.
    ws = tmp_path / "hookws-empty"
    payloads = ws / "capture" / "payloads.jsonl"
    hook = install_hook(cursor_package, ws, jq=system_jq(), payloads=payloads)

    result = run_hook(hook, "")
    assert result.returncode == 0
    assert load_stream(result.stdout) == [{}]
    assert not (payloads.is_file() and payloads.stat().st_size > 0)


def test_an_unparsable_payload_is_still_appended_as_one_line(
    tmp_path: Path, cursor_package: Path
) -> None:
    # payloads.jsonl is one JSON value per line. A pretty-printed fallback would break the
    # slurp for every later reader, including the recorder.
    ws = tmp_path / "hookws-unparsable"
    payloads = ws / "capture" / "payloads.jsonl"
    hook = install_hook(cursor_package, ws, jq=system_jq(), payloads=payloads)

    result = run_hook(hook, "not json at all")
    assert result.returncode == 0

    assert line_count(payloads) == 1
    assert jq_cli.test('length == 1 and (.[0].unparsed | type == "string")', payloads, slurp=True)


def test_a_scalar_payload_is_wrapped_rather_than_appended_verbatim(
    tmp_path: Path, cursor_package: Path
) -> None:
    # Every line of payloads.jsonl must be an object. A bare scalar would be valid JSON and
    # land verbatim, and then every later slurping filter indexing a field errors on it —
    # which the capture poll reads as "no match", so the runner polls to the full timeout on
    # a capture that already arrived.
    ws = tmp_path / "hookws-scalar"
    payloads = ws / "capture" / "payloads.jsonl"
    hook = install_hook(cursor_package, ws, jq=system_jq(), payloads=payloads)

    result = run_hook(hook, '"just a string"')
    assert result.returncode == 0

    assert line_count(payloads) == 1
    assert jq_cli.test('length == 1 and (.[0] | type == "object")', payloads, slurp=True)
    # A filter indexing a field must not error on it.
    assert jq_cli.test('any(.[]; .hook_event_name == "nope") | not', payloads, slurp=True)


def test_the_capture_hook_echoes_when_its_payloads_path_is_unwritable(
    tmp_path: Path, cursor_package: Path
) -> None:
    ws = tmp_path / "hookws-unwritable"
    hook = install_hook(
        cursor_package, ws, jq=system_jq(), payloads=ws / "no" / "such" / "dir" / "payloads.jsonl"
    )

    result = run_hook(hook, '{"a":1}')
    assert result.returncode == 0
    assert load_stream(result.stdout) == [{}]
