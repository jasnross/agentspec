"""Fake OpenAI-compatible chat endpoint that scripts one tool call.

Usage: fake_provider.py <dump-dir> <marker> <tool-name> <arguments-json>. Binds
127.0.0.1 on an ephemeral port and prints the port on the first stdout line.
Every POST body is written verbatim to <dump-dir>/<NNNN>.request.json.

A request whose system message carries <marker> and which holds no
`role: "tool"` message is answered with one streamed call to <tool-name>, whose
`arguments` string is <arguments-json>. Every other request — the follow-up
carrying the tool result, and OpenCode's title generation — is answered with a
streamed "ok".

The call is scripted because the question is what OpenCode does when the agent
calls a tool its permission map governs, and a fake that only ever answers "ok"
never makes OpenCode run a tool. The oracle is still the dump: the tool result
OpenCode sends back in the follow-up request, not anything the fake replies.
"""
import itertools
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DUMP = sys.argv[1]
MARKER = sys.argv[2]
TOOL_NAME = sys.argv[3]
ARGUMENTS = sys.argv[4]
# Malformed arguments would send the call down OpenCode's invalid-tool repair
# path, which still produces a tool result, so refuse them before binding.
try:
    json.loads(ARGUMENTS)
except ValueError:
    sys.exit(f"fake_provider: <arguments-json> is not JSON: {ARGUMENTS}")
COUNTER = itertools.count()
OK_CHUNKS = [
    {"choices": [{"index": 0, "delta": {"role": "assistant"}}]},
    {"choices": [{"index": 0, "delta": {"content": "ok"}}]},
    {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
]
CALL_CHUNKS = [
    {"choices": [{"index": 0, "delta": {"role": "assistant"}}]},
    {"choices": [{"index": 0, "delta": {"tool_calls": [{
        "index": 0,
        "id": "call_1",
        "type": "function",
        "function": {"name": TOOL_NAME, "arguments": ARGUMENTS},
    }]}}]},
    {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]},
]


def wants_call(body):
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
        chunks = CALL_CHUNKS if wants_call(json.loads(raw)) else OK_CHUNKS
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
