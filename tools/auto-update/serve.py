import http.server
import pathlib
import sys
import time

root = pathlib.Path(sys.argv[1])
port = int(sys.argv[2])


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        path = root / self.path.split("?")[0].lstrip("/")
        if not path.is_file():
            self.send_error(404)
            return
        body = path.read_bytes()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        rate = root / "rate"
        limit = int(rate.read_text()) if rate.is_file() else 0
        step = limit if limit else len(body)
        for start in range(0, len(body), step):
            try:
                self.wfile.write(body[start:start + step])
            except (BrokenPipeError, ConnectionResetError):
                return
            if limit:
                time.sleep(1)

    def log_message(self, *args):
        sys.stderr.write("%s\n" % self.path)


http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
