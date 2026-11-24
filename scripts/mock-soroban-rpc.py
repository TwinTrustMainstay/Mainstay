#!/usr/bin/env python3
"""Small deterministic Soroban JSON-RPC mock for offline integration tests."""

import argparse
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


DEFAULT_RESPONSES = {
    "getHealth": {"status": "healthy"},
    "getLatestLedger": {
        "id": "offline-ledger",
        "protocolVersion": 22,
        "sequence": 1,
    },
}


def load_responses(path):
    if path is None:
        return dict(DEFAULT_RESPONSES)
    with path.open(encoding="utf-8") as fixture:
        responses = json.load(fixture)
    if not isinstance(responses, dict):
        raise ValueError("fixture must contain a JSON object keyed by RPC method")
    return responses


def make_handler(responses):
    class MockRpcHandler(BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path == "/health":
                self._write_json({"status": "healthy"})
            else:
                self._write_json({"error": "not found"}, 404)

        def do_POST(self):
            if self.path != "/soroban/rpc":
                self._write_json({"error": "not found"}, 404)
                return

            try:
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                method = request.get("method")
                if method not in responses:
                    self._write_json(
                        {
                            "jsonrpc": "2.0",
                            "id": request.get("id"),
                            "error": {
                                "code": -32601,
                                "message": f"method not configured: {method}",
                            },
                        }
                    )
                    return
                self._write_json(
                    {
                        "jsonrpc": "2.0",
                        "id": request.get("id"),
                        "result": responses[method],
                    }
                )
            except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
                self._write_json(
                    {
                        "jsonrpc": "2.0",
                        "id": None,
                        "error": {"code": -32600, "message": str(error)},
                    },
                    400,
                )

        def _write_json(self, payload, status=200):
            encoded = json.dumps(payload).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def log_message(self, *_):
            return

    return MockRpcHandler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", type=Path, help="JSON object mapping RPC methods to results")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8000)
    args = parser.parse_args()

    responses = load_responses(args.fixture)
    server = ThreadingHTTPServer((args.host, args.port), make_handler(responses))
    print(f"Mock Soroban RPC listening on http://{args.host}:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
