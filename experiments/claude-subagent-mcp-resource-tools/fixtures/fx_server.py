"""Minimal MCP stdio server for agentspec probes: server `fx`, tools `alpha` and `beta`, one resource and one resource template.

Usage: fx_server.py <stamp-path>. Stdlib only. Speaks newline-delimited JSON-RPC
2.0 on stdin/stdout and answers initialize, tools/list, tools/call,
resources/list, resources/templates/list, and resources/read. Notifications (no
`id`) are ignored. Answering tools/list writes <stamp-path>, which the runner
reads as the arm's proof that the provider connected.

The server advertises the `resources` capability and serves resources, so a
session connected to it has resources for Claude Code's MCP resource tools to
reach. This package makes no call, so what the resources hold is never read.
"""
import json
import sys

STAMP = sys.argv[1]
TOOLS = [
    {"name": name, "description": f"agentspec probe tool {name}",
     "inputSchema": {"type": "object", "properties": {}}}
    for name in ("alpha", "beta")
]
RESOURCES = [{"uri": "fx://AGENTSPEC-RESOURCE-CCRES5", "name": "probe-doc", "mimeType": "text/plain"}]
TEMPLATES = [{"uriTemplate": "fx://AGENTSPEC-RESOURCE-CCRES5/{id}", "name": "probe-template"}]

for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method = msg.get("method")
    if method == "initialize":
        result = {"protocolVersion": msg["params"].get("protocolVersion", "2025-06-18"),
                  "capabilities": {"tools": {}, "resources": {}},
                  "serverInfo": {"name": "fx", "version": "0"}}
    elif method == "tools/list":
        with open(STAMP, "w") as f:
            f.write("tools/list\n")
        result = {"tools": TOOLS}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "ok"}]}
    elif method == "resources/list":
        result = {"resources": RESOURCES}
    elif method == "resources/templates/list":
        result = {"resourceTemplates": TEMPLATES}
    elif method == "resources/read":
        result = {"contents": [{"uri": msg["params"]["uri"], "mimeType": "text/plain",
                                "text": "AGENTSPEC-RESOURCE-CONTENT-CCRES5"}]}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"],
                          "error": {"code": -32601, "message": f"unsupported: {method}"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
