#!/usr/bin/env python3
"""One origin for the first app: static page plus a pass-through to the door.

Two reasons this exists, and neither is convenience.

**The door sends no CORS headers.** There is no `Access-Control-Allow-Origin`
anywhere in `crates/`, so a page on any other origin cannot read the door's
answer, and a page opened from `file://` sends `Origin: null` and cannot
either. Serving the page and the op routes from one origin removes the
question instead of answering it. A real deployment answers it properly, with
either a CORS layer at the door or a reverse proxy in front of both. That
choice is on the decisions board and is not made here.

**The token stays out of the browser.** The proxy reads `ORDO_TOKEN` from its
own environment and adds the header on the way through, so the page holds no
credential, keeps none in `localStorage`, and leaks none into a screenshot.
The page cannot mint privilege it was not handed: everything it can reach is
what this file forwards.

Stdlib only. Run it beside `ordo dev`:

    ORDO_TOKEN='<the token ordo dev printed>' ORDO_SPACE=dev \\
      python3 examples/first-app/web/serve.py

Then open http://127.0.0.1:4100.
"""

import json
import os
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

DOOR = os.environ.get("ORDO_DOOR", "http://127.0.0.1:4000")
SPACE = os.environ.get("ORDO_SPACE", "dev")
TOKEN = os.environ.get("ORDO_TOKEN", "")
PORT = int(os.environ.get("PORT", "4100"))

HERE = Path(__file__).resolve().parent

# The whole forwardable surface, named. A path that is not one of these is a
# 404 from this file rather than a request the door has to refuse.
FORWARD_PREFIXES = ("/op/", "/interface")


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        print(f"{self.command} {self.path} -> {fmt % args}")

    def _forward(self, body: bytes | None) -> None:
        req = urllib.request.Request(
            DOOR + self.path,
            data=body,
            method=self.command,
        )
        req.add_header("x-ordo-space", SPACE)
        if TOKEN:
            req.add_header("authorization", f"Bearer {TOKEN}")
        if body is not None:
            req.add_header("content-type", "application/json")
        try:
            with urllib.request.urlopen(req) as answer:
                status, payload = answer.status, answer.read()
        except urllib.error.HTTPError as refusal:
            # A refusal is part of the product. Hand the envelope through with
            # its own status so the page can show what the door said, rather
            # than flattening every failure into one of this file's own.
            status, payload = refusal.code, refusal.read()
        except urllib.error.URLError as unreachable:
            status = 502
            payload = json.dumps(
                {
                    "error": {
                        "code": "door_unreachable",
                        "message": f"{DOOR} did not answer ({unreachable.reason}). "
                        f"Is `ordo dev` running?",
                    }
                }
            ).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _page(self) -> None:
        html = (HERE / "index.html").read_bytes()
        self.send_response(200)
        self.send_header("content-type", "text/html; charset=utf-8")
        self.send_header("content-length", str(len(html)))
        self.end_headers()
        self.wfile.write(html)

    def do_GET(self) -> None:
        if self.path in ("/", "/index.html"):
            self._page()
        elif self.path.startswith(FORWARD_PREFIXES):
            self._forward(None)
        else:
            self.send_error(404)

    def do_POST(self) -> None:
        if self.path.startswith(FORWARD_PREFIXES):
            length = int(self.headers.get("content-length", "0"))
            self._forward(self.rfile.read(length))
        else:
            self.send_error(404)


if __name__ == "__main__":
    if not TOKEN:
        print("ORDO_TOKEN is not set, so every call will be refused with")
        print("access_denied. That is the door working. Set it to the token")
        print("`ordo dev` printed at boot and restart this.")
    print(f"first-app  http://127.0.0.1:{PORT}   door={DOOR}  space={SPACE}")
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
