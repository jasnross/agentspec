"""The fake model endpoint, `lib/stubs/fake_provider.py`."""

import json
import subprocess
import sys
import urllib.request
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from tests.conftest import (
    STUBS,
)


@pytest.fixture
def fake(tmp_path: Path, request: pytest.FixtureRequest) -> Iterator[tuple[int, Path]]:
    """A running fake provider, started with the flags the test is parametrized by."""
    flags: tuple[str, ...] = getattr(request, "param", ())
    dump = tmp_path / "sink"
    dump.mkdir()
    process = subprocess.Popen(
        [sys.executable, str(STUBS / "fake_provider.py"), str(dump), *flags],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        assert process.stdout is not None
        port = int(process.stdout.readline())
        yield port, dump
    finally:
        process.terminate()
        process.wait()


def _post(port: int, body: dict[str, Any]) -> str:
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return str(response.read().decode())


def test_fake_provider_prints_a_port_and_dumps_each_request(fake: tuple[int, Path]) -> None:
    port, dump = fake
    body = {"messages": [{"role": "user", "content": "hi"}]}
    reply = _post(port, body)
    assert json.loads((dump / "0000.request.json").read_text(encoding="utf-8")) == body
    assert '"content": "ok"' in reply
    assert reply.endswith("data: [DONE]\n\n")


MARKED = {"role": "system", "content": "agent body MARK"}


@pytest.mark.parametrize(
    "fake",
    [("--marker", "MARK", "--tool", "read", "--args", '{"filePath": "/x"}')],
    indirect=True,
)
def test_with_a_marker_it_calls_only_for_a_marked_request_without_a_tool_message(
    fake: tuple[int, Path],
) -> None:
    port, dump = fake
    unmarked = _post(port, {"messages": [{"role": "user", "content": "hi"}]})
    marked = _post(port, {"messages": [MARKED, {"role": "user", "content": "hi"}]})
    answered = _post(port, {"messages": [MARKED, {"role": "tool", "content": "r"}]})
    assert "tool_calls" not in unmarked
    assert "tool_calls" not in answered
    assert '"name": "read", "arguments": "{\\"filePath\\": \\"/x\\"}"' in marked
    assert '"finish_reason": "tool_calls"' in marked
    assert sorted(p.name for p in dump.iterdir()) == [
        "0000.request.json",
        "0001.request.json",
        "0002.request.json",
    ]


def test_bad_arguments_exit_before_a_port_is_printed(tmp_path: Path) -> None:
    completed = subprocess.run(
        [
            sys.executable,
            str(STUBS / "fake_provider.py"),
            str(tmp_path),
            "--marker",
            "M",
            "--tool",
            "t",
            "--args",
            "{not json",
        ],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    assert completed.returncode != 0
    assert completed.stdout == ""
    assert "fake_provider: <arguments-json> is not JSON: {not json" in completed.stderr


def test_the_scripted_flags_go_together(tmp_path: Path) -> None:
    completed = subprocess.run(
        [sys.executable, str(STUBS / "fake_provider.py"), str(tmp_path), "--marker", "M"],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    assert completed.returncode != 0
    assert completed.stdout == ""
    assert "--marker, --tool and --args are given together or not at all" in completed.stderr


def test_a_plain_stub_answers_a_body_that_is_not_json(fake: tuple[int, Path]) -> None:
    port, dump = fake
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/v1/chat/completions", data=b"not json", method="POST"
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        assert '"content": "ok"' in response.read().decode()
    assert (dump / "0000.request.json").read_bytes() == b"not json"
