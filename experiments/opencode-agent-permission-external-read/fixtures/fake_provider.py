"""Fake OpenAI-compatible chat endpoint that scripts one `read` tool call.

Usage: fake_provider.py <dump-dir> <read-path> <marker>. Binds 127.0.0.1 on an
ephemeral port and prints the port on the first stdout line. Every POST body is
written verbatim to <dump-dir>/<NNNN>.request.json.

A request whose system message carries <marker> and which holds no
`role: "tool"` message is answered with one streamed `read` tool call for
<read-path>. Every other request — the follow-up carrying the tool result, and
OpenCode's title generation — is answered with a streamed "ok".

The call is scripted because the question is what OpenCode does when the agent
reads outside the project, and a fake that only ever answers "ok" never makes
OpenCode run a tool. The oracle is still the dump: the tool result OpenCode
sends back in the follow-up request, not anything the fake replies.
"""
import itertools
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DUMP = sys.argv[1]
READ_PATH = sys.argv[2]
MARKER = sys.argv[3]
COUNTER = itertools.count()
OK_CHUNKS = [
    {"choices": [{"index": 0, "delta": {"role": "assistant"}}]},
    {"choices": [{"index": 0, "delta": {"content": "ok"}}]},
    {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
]
READ_CHUNKS = [
    {"choices": [{"index": 0, "delta": {"role": "assistant"}}]},
    {"choices": [{"index": 0, "delta": {"tool_calls": [{
        "index": 0,
        "id": "call_1",
        "type": "function",
        "function": {"name": "read", "arguments": json.dumps({"filePath": READ_PATH})},
    }]}}]},
    {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
]


def wants_read(body):
    messages = body.get("messages") or []
    marked = any(m.get("role") == "system" and MARKER in json.dumps(m.get("content"))
                 for m in messages)
    answered = any(m.get("role") == "tool" for m in messages)
    return marked and not answered


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        with open(os.path.join(DUMP, f"{next(COUNTER):04d}.request.json"), "wb") as f:
            f.write(raw)
        chunks = READ_CHUNKS if wants_read(json.loads(raw)) else OK_CHUNKS
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
