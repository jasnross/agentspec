"""Fake OpenAI-compatible chat endpoint for agentspec probes.

Usage: fake_provider.py <dump-dir> [--marker <marker> --tool <name> --args <json>]

Stdlib only, and imports nothing from the probe harness. Binds 127.0.0.1 on an ephemeral
port and prints the port on the first stdout line. Every POST body is written verbatim to
<dump-dir>/<NNNN>.request.json. The oracle is the dump, never the reply.

Without `--marker`, every request is answered with a streamed "ok". With it, a request
whose system message carries <marker> and which holds no `role: "tool"` message is answered
with one streamed call to <name>, whose `arguments` string is <json>; every other request
(the follow-up carrying the tool result, and OpenCode's title generation) gets "ok". The
call is scripted because a fake that only ever answers "ok" never makes OpenCode run a tool.
"""

import argparse
import itertools
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

OK_CHUNKS: list[dict[str, Any]] = [
    {"choices": [{"index": 0, "delta": {"role": "assistant"}}]},
    {"choices": [{"index": 0, "delta": {"content": "ok"}}]},
    {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
]


def main() -> None:
    parser = argparse.ArgumentParser(prog="fake_provider.py")
    parser.add_argument("dump")
    parser.add_argument("--marker")
    parser.add_argument("--tool")
    parser.add_argument("--args", dest="arguments")
    args = parser.parse_args()

    scripted = (args.marker, args.tool, args.arguments)
    if any(value is not None for value in scripted) and None in scripted:
        parser.error("--marker, --tool and --args are given together or not at all")
    if args.arguments is not None:
        # Malformed arguments would send the call down OpenCode's invalid-tool repair path,
        # which still produces a tool result, so refuse them before binding.
        try:
            json.loads(args.arguments)
        except ValueError:
            sys.exit(f"fake_provider: <arguments-json> is not JSON: {args.arguments}")

    call_chunks: list[dict[str, Any]] = [
        {"choices": [{"index": 0, "delta": {"role": "assistant"}}]},
        {
            "choices": [
                {
                    "index": 0,
                    "delta": {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_1",
                                "type": "function",
                                "function": {"name": args.tool, "arguments": args.arguments},
                            }
                        ]
                    },
                }
            ]
        },
        {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
    ]
    counter = itertools.count()

    def wants_call(body: dict[str, Any]) -> bool:
        if args.marker is None:
            return False
        messages = body.get("messages") or []
        marked = any(
            m.get("role") == "system" and args.marker in json.dumps(m.get("content"))
            for m in messages
        )
        answered = any(m.get("role") == "tool" for m in messages)
        return marked and not answered

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format: str, *log_args: Any) -> None:
            pass

        def do_POST(self) -> None:
            raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            path = os.path.join(args.dump, f"{next(counter):04d}.request.json")  # noqa: PTH118
            with open(path, "wb") as f:  # noqa: PTH123
                f.write(raw)
            # The body is parsed only to script a call; a plain stub never reads it.
            scripted_call = args.marker is not None and wants_call(json.loads(raw))
            chunks = call_chunks if scripted_call else OK_CHUNKS
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            for chunk in chunks:
                chunk = dict(chunk, id="c1", object="chat.completion.chunk", created=0, model="m")
                self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
            self.wfile.write(b"data: [DONE]\n\n")

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_address[1], flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
