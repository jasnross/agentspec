"""Fake OpenAI-compatible chat endpoint for agentspec probes.

Usage: fake_provider.py <dump-dir>. Binds 127.0.0.1 on an ephemeral port and
prints the port on the first stdout line. Every POST body is written verbatim
to <dump-dir>/<NNNN>.request.json, and the reply is a streamed "ok". The fake
never inspects what it records: the probe's oracle is the dump, not the reply.
"""
import itertools
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

DUMP = sys.argv[1]
COUNTER = itertools.count()
CHUNKS = [
    {"choices": [{"index": 0, "delta": {"role": "assistant"}}]},
    {"choices": [{"index": 0, "delta": {"content": "ok"}}]},
    {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
]


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        with open(os.path.join(DUMP, f"{next(COUNTER):04d}.request.json"), "wb") as f:
            f.write(raw)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for chunk in CHUNKS:
            chunk.update({"id": "c1", "object": "chat.completion.chunk", "created": 0, "model": "m"})
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
