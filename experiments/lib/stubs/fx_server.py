"""Minimal MCP stdio server for agentspec probes: server `fx`, tools `alpha` and `beta`.

Usage: fx_server.py --stamp <path> [--reply <text>] [--resources <tag>]

Stdlib only, and imports nothing from the probe harness: the provider spawns it from the
command a probe's fixture names, under a bare interpreter. It speaks newline-delimited
JSON-RPC 2.0 on stdin/stdout and answers `initialize`, `tools/list`, and `tools/call`.
Notifications (no `id`) are ignored. Answering `tools/list` writes the stamp file, which
the runner reads as the arm's proof that the provider connected.

`--reply` sets the text a `tools/call` answers with, so a call that ran is told apart from a
denied one by text only this server returns. `--resources <tag>` advertises the
`resources` capability and serves one resource and one template whose URIs and content
carry <tag>, for probes of a provider's MCP resource tools.
"""

import argparse
import json
import sys
from typing import Any


def main() -> None:
    parser = argparse.ArgumentParser(prog="fx_server.py")
    parser.add_argument("--stamp", required=True)
    parser.add_argument("--reply", default="ok")
    parser.add_argument("--resources", metavar="TAG")
    args = parser.parse_args()

    tools = [
        {
            "name": name,
            "description": f"agentspec probe tool {name}",
            "inputSchema": {"type": "object", "properties": {}},
        }
        for name in ("alpha", "beta")
    ]
    tag = args.resources
    resources = [
        {"uri": f"fx://AGENTSPEC-RESOURCE-{tag}", "name": "probe-doc", "mimeType": "text/plain"}
    ]
    templates = [{"uriTemplate": f"fx://AGENTSPEC-RESOURCE-{tag}/{{id}}", "name": "probe-template"}]

    for line in sys.stdin:
        msg = json.loads(line)
        if "id" not in msg:
            continue
        method = msg.get("method")
        result: dict[str, Any]
        if method == "initialize":
            capabilities: dict[str, Any] = {"tools": {}}
            if tag is not None:
                capabilities["resources"] = {}
            result = {
                "protocolVersion": msg["params"].get("protocolVersion", "2025-06-18"),
                "capabilities": capabilities,
                "serverInfo": {"name": "fx", "version": "0"},
            }
        elif method == "tools/list":
            with open(args.stamp, "w") as f:  # noqa: PTH123 - kept as the copies wrote it
                f.write("tools/list\n")
            result = {"tools": tools}
        elif method == "tools/call":
            result = {"content": [{"type": "text", "text": args.reply}]}
        elif tag is not None and method == "resources/list":
            result = {"resources": resources}
        elif tag is not None and method == "resources/templates/list":
            result = {"resourceTemplates": templates}
        elif tag is not None and method == "resources/read":
            result = {
                "contents": [
                    {
                        "uri": msg["params"]["uri"],
                        "mimeType": "text/plain",
                        "text": f"AGENTSPEC-RESOURCE-CONTENT-{tag}",
                    }
                ]
            }
        else:
            error = {"code": -32601, "message": f"unsupported: {method}"}
            print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "error": error}), flush=True)
            continue
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)


if __name__ == "__main__":
    main()
