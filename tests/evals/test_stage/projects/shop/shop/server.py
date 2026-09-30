"""The HTTP server around `Api`."""

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from .api import Api
from .repository import OrderRepository
from .service import OrderService


def make_server(db_path: str, host: str = "127.0.0.1", port: int = 0) -> ThreadingHTTPServer:
    api = Api(OrderService(OrderRepository(db_path)))

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            self._dispatch("GET")

        def do_POST(self):
            self._dispatch("POST")

        def _dispatch(self, method: str) -> None:
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length) if length else b""
            status, payload = api.handle(method, self.path, body)
            data = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, format, *args):
            pass

    return ThreadingHTTPServer((host, port), Handler)


def main(argv=None) -> None:
    parser = argparse.ArgumentParser(description="Serve the order API.")
    parser.add_argument("--db", default="shop.db")
    parser.add_argument("--port", type=int, default=8080)
    args = parser.parse_args(argv)
    make_server(args.db, port=args.port).serve_forever()


if __name__ == "__main__":
    main()
