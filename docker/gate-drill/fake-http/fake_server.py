#!/usr/bin/env python3
"""A deliberately fake HTTP server, standing in for a broken `ordo-host` or
`ordo-playground` boot so the gate drill (.github/workflows/gate-drill.yml)
can prove `images.yml`'s smoke assertions fail on the responses they are
supposed to catch, without patching the real services -- which live in
crates/, out of this fixture's remit; see docs/roadmap-checklist.md 15a.

FAKE_ROUTES overrides individual paths: "path=status,path=status". Any path
not listed answers 200. /health/live is special-cased to also carry a body
containing "live" unless FAKE_ROUTES overrides it, since the real smoke's
boot-wait loop polls that path first and checks for the word.

FAKE_LOG_LINES prints '|'-separated lines to stdout at startup, standing in
for the playground's `seeded: <db>` / `UNAVAILABLE: <db> (...)` boot lines,
which the real smoke reads from `docker logs` rather than from HTTP.
"""
import os
from http.server import BaseHTTPRequestHandler, HTTPServer

ROUTES = {}
for pair in os.environ.get("FAKE_ROUTES", "").split(","):
    pair = pair.strip()
    if not pair:
        continue
    path, status = pair.split("=")
    ROUTES[path] = int(status)

for line in os.environ.get("FAKE_LOG_LINES", "").split("|"):
    if line:
        print(line, flush=True)


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        status = ROUTES.get(self.path, 200)
        if self.path == "/health/live" and "/health/live" not in ROUTES:
            body = b'{"status":"live"}'
        else:
            body = b'{"fake":true}'
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):  # noqa: A002 - stdlib signature
        pass


if __name__ == "__main__":
    port = int(os.environ.get("FAKE_PORT", "8080"))
    HTTPServer(("0.0.0.0", port), Handler).serve_forever()
