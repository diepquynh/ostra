"""Starts the real server for a test and talks to it over HTTP."""

import json
import os
import tempfile
import threading
import urllib.error
import urllib.request
from contextlib import contextmanager

from shop.server import make_server


@contextmanager
def running_server():
    """Yield the base URL of a server on a fresh database, and stop the server afterwards."""
    with tempfile.TemporaryDirectory() as tmp:
        server = make_server(os.path.join(tmp, "shop.db"))
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        thread.start()
        try:
            host, port = server.server_address[:2]
            yield f"http://{host}:{port}"
        finally:
            server.shutdown()
            server.server_close()


def call(base_url: str, method: str, path: str, payload=None) -> tuple[int, dict]:
    """Send one request and return the status code and the decoded JSON body, for errors too."""
    data = None if payload is None else json.dumps(payload).encode()
    request = urllib.request.Request(
        base_url + path, data=data, method=method, headers={"Content-Type": "application/json"}
    )
    try:
        with urllib.request.urlopen(request) as response:
            return response.status, json.load(response)
    except urllib.error.HTTPError as error:
        return error.code, json.load(error)
