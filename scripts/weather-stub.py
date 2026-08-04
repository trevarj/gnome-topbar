#!/usr/bin/env python3
"""A stand-in for Open-Meteo, for the visual smoke run.

Serves the same recorded fixtures the parser tests are built on, so a
screenshot of a populated forecast does not depend on somebody else's uptime,
somebody else's rate limit, or the machine having a network at all.

    python3 scripts/weather-stub.py --port 18080 [--status 500]

Two paths, matching what TOPBAR_WEATHER_API and TOPBAR_GEOCODING_API are
pointed at:

    /forecast   the five-day Celsius forecast fixture
    /search     the geocoding fixture, six places for "moscow"

--status makes every answer that status with the rate-limit body instead,
which is how the stale and unavailable states get photographed.
"""

import argparse
import json
import pathlib
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

FIXTURES = pathlib.Path(__file__).resolve().parent.parent / (
    "crates/topbar-services/tests/fixtures"
)


def fixture(name):
    return (FIXTURES / name).read_text(encoding="utf-8")


class Handler(BaseHTTPRequestHandler):
    status = 200

    def do_GET(self):  # noqa: N802 - the base class names it
        if self.status != 200:
            body = fixture("open-meteo-rate-limit.json")
            self.answer(self.status, body)
            return

        path = self.path.split("?", 1)[0]
        if path.startswith("/forecast"):
            self.answer(200, fixture("open-meteo-forecast-celsius.json"))
        elif path.startswith("/search"):
            self.answer(200, fixture("open-meteo-geocoding.json"))
        else:
            self.answer(404, json.dumps({"error": True, "reason": "no such path"}))

    def answer(self, status, body):
        encoded = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, fmt, *args):
        sys.stderr.write("weather-stub: " + (fmt % args) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=18080)
    parser.add_argument("--status", type=int, default=200)
    options = parser.parse_args()

    Handler.status = options.status
    server = ThreadingHTTPServer(("127.0.0.1", options.port), Handler)
    sys.stderr.write(
        f"weather-stub: serving fixtures on 127.0.0.1:{options.port} "
        f"with status {options.status}\n"
    )
    server.serve_forever()


if __name__ == "__main__":
    main()
