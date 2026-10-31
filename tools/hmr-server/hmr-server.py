#!/usr/bin/env python3
import base64, hashlib, html, os, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PAGE = """<!doctype html><title>hmr-server</title>
<p id="message">{message}</p><p id="host">served for {host}</p><p id="loads"></p><p id="state">connecting</p>
<script>
const loads = Number(sessionStorage.loads || 0) + 1; sessionStorage.loads = loads;
document.getElementById("loads").textContent = "loads " + loads;
const ws = new WebSocket("ws://" + location.host + "/hmr");
ws.onopen = () => document.getElementById("state").textContent = "connected";
ws.onmessage = e => {{ document.getElementById("message").textContent = e.data; document.getElementById("state").textContent = "hot updated"; }};
ws.onclose = () => document.getElementById("state").textContent = "disconnected";
</script>
"""


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def message(self):
        try:
            with open(os.path.join(self.server.folder, "message.txt")) as f:
                return f.read().strip()
        except OSError:
            return "no message.txt"

    def do_GET(self):
        if self.path == "/hmr" and self.headers.get("Upgrade", "").lower() == "websocket":
            return self.hmr()
        body = PAGE.format(message=html.escape(self.message()), host=html.escape(self.headers.get("Host", ""))).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def hmr(self):
        accept = base64.b64encode(hashlib.sha1((self.headers["Sec-WebSocket-Key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        self.send_response(101)
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", accept)
        self.end_headers()
        self.wfile.flush()
        last = self.message()
        while True:
            time.sleep(0.1)
            now = self.message()
            if now == last:
                continue
            last = now
            data = now.encode()
            header = bytes([0x81, len(data)]) if len(data) < 126 else bytes([0x81, 126]) + len(data).to_bytes(2, "big")
            try:
                self.wfile.write(header + data)
                self.wfile.flush()
            except OSError:
                return

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler)
    server.folder = sys.argv[2] if len(sys.argv) > 2 else "."
    server.daemon_threads = True
    print(f"hmr-server on http://127.0.0.1:{sys.argv[1]}/", flush=True)
    server.serve_forever()
