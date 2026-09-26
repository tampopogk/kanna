#!/usr/bin/env python3
"""Local-only request recorder for the WKWebView cookie isolation test."""

import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


port_file, log_file = sys.argv[1:3]
open(log_file, "a", encoding="utf-8").close()


class Handler(BaseHTTPRequestHandler):
    def record(self):
        with open(log_file, "a", encoding="utf-8") as output:
            output.write(json.dumps({
                "method": self.command,
                "path": self.path,
                "cookie": self.headers.get("Cookie", ""),
                "origin": self.headers.get("Origin", ""),
            }) + "\n")
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()

    do_GET = record
    do_POST = record

    def log_message(self, format, *args):
        pass


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w", encoding="utf-8") as output:
    output.write(str(server.server_port))
server.serve_forever()
