"""The shared stub servers: the MCP server a provider spawns, and the fake model endpoint."""

import json
import subprocess
import sys
import urllib.request
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest

from tests.conftest import EXPERIMENTS_DIR

STUBS = EXPERIMENTS_DIR / "lib" / "stubs"

EXCHANGE = [
    {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "2025-03-26"},
    },
    {"jsonrpc": "2.0", "method": "notifications/initialized"},
    {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
    {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "alpha"}},
    {"jsonrpc": "2.0", "id": 4, "method": "resources/list"},
    {"jsonrpc": "2.0", "id": 5, "method": "resources/templates/list"},
    {"jsonrpc": "2.0", "id": 6, "method": "resources/read", "params": {"uri": "fx://U"}},
    {"jsonrpc": "2.0", "id": 7, "method": "bogus/method"},
]


def _fx(tmp_path: Path, *flags: str) -> tuple[list[dict[str, Any]], Path]:
    stamp = tmp_path / f"stamp{len(list(tmp_path.glob('stamp*')))}"
    completed = subprocess.run(
        [sys.executable, str(STUBS / "fx_server.py"), "--stamp", str(stamp), *flags],
        input="".join(json.dumps(m) + "\n" for m in EXCHANGE),
        capture_output=True,
        text=True,
        timeout=30,
        check=True,
    )
    return [json.loads(line) for line in completed.stdout.splitlines()], stamp


def test_fx_server_answers_the_exchange_and_writes_its_stamp(tmp_path: Path) -> None:
    replies, stamp = _fx(tmp_path)
    by_id = {r["id"]: r for r in replies}
    # The notification gets no reply.
    assert sorted(by_id) == [1, 2, 3, 4, 5, 6, 7]
    assert by_id[1]["result"] == {
        "protocolVersion": "2025-03-26",
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "fx", "version": "0"},
    }
    assert [t["name"] for t in by_id[2]["result"]["tools"]] == ["alpha", "beta"]
    assert by_id[3]["result"] == {"content": [{"type": "text", "text": "ok"}]}
    for i, method in (
        (4, "resources/list"),
        (5, "resources/templates/list"),
        (6, "resources/read"),
        (7, "bogus/method"),
    ):
        assert by_id[i]["error"] == {"code": -32601, "message": f"unsupported: {method}"}
    assert stamp.read_text(encoding="utf-8") == "tools/list\n"


def test_reply_changes_only_the_tools_call_text(tmp_path: Path) -> None:
    plain, _ = _fx(tmp_path)
    replied, _ = _fx(tmp_path, "--reply", "X")
    differing = [a["id"] for a, b in zip(plain, replied, strict=True) if a != b]
    assert differing == [3]
    assert replied[2]["result"] == {"content": [{"type": "text", "text": "X"}]}


def test_resources_adds_the_three_resource_methods(tmp_path: Path) -> None:
    replies, _ = _fx(tmp_path, "--resources", "T")
    by_id = {r["id"]: r for r in replies}
    assert by_id[1]["result"]["capabilities"] == {"tools": {}, "resources": {}}
    assert by_id[4]["result"] == {
        "resources": [
            {"uri": "fx://AGENTSPEC-RESOURCE-T", "name": "probe-doc", "mimeType": "text/plain"}
        ]
    }
    assert by_id[5]["result"] == {
        "resourceTemplates": [
            {"uriTemplate": "fx://AGENTSPEC-RESOURCE-T/{id}", "name": "probe-template"}
        ]
    }
    assert by_id[6]["result"] == {
        "contents": [
            {"uri": "fx://U", "mimeType": "text/plain", "text": "AGENTSPEC-RESOURCE-CONTENT-T"}
        ]
    }
    assert "error" in by_id[7]


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
