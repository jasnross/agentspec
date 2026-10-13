"""The stub MCP server a provider spawns, `lib/stubs/fx_server.py`."""

import json
import subprocess
import sys
from pathlib import Path
from typing import Any

from tests.conftest import (
    STUBS,
)

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
