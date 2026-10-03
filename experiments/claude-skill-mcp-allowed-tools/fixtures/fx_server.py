"""Minimal MCP stdio server for agentspec probes: server `fx`, tools `alpha` and `beta`.

Usage: fx_server.py <stamp-path>. Stdlib only. Speaks newline-delimited JSON-RPC
2.0 on stdin/stdout and answers initialize, tools/list, and tools/call.
Notifications (no `id`) are ignored. Answering tools/list writes <stamp-path>,
which the runner reads as the arm's proof that the provider connected.

A call answers AGENTSPEC-FX-REPLY-CCSKILL7, so a call that ran is told apart
from a denied one by text only this server returns.
"""
import json
import sys

STAMP = sys.argv[1]
TOOLS = [
    {"name": name, "description": f"agentspec probe tool {name}",
     "inputSchema": {"type": "object", "properties": {}}}
    for name in ("alpha", "beta")
]

for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion": msg["params"].get("protocolVersion", "2025-06-18"),
                  "capabilities": {"tools": {}},
                  "serverInfo": {"name": "fx", "version": "0"}}
    elif method == "tools/list":
        with open(STAMP, "w") as f:
            f.write("tools/list\n")
        result = {"tools": TOOLS}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "AGENTSPEC-FX-REPLY-CCSKILL7"}]}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"],
                          "error": {"code": -32601, "message": f"unsupported: {method}"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
