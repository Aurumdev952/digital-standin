"""Serves pplx-decider over a Unix socket, one JSON object per line.

Request:  {"id": 7, "request": {"state": ..., "questions": {...}}}
Reply:    {"id": 7, "response": {...}, "ms": 41.2}   or   {"id": 7, "error": "..."}
Ping:     {"id": 0, "ping": true}  ->  {"id": 0, "pong": true, "device": "cuda", "fast": true}

One request is decided at a time; the client keeps at most one in flight.
"""

from __future__ import annotations

import argparse
import json
import os
import socketserver
import sys
import threading
import time
from pathlib import Path

import torch

from pplx_decider import Decider, FastOptions, RequestError

DEFAULT_SOCKET = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "digital-standin-decider.sock"
DEFAULT_MODEL = Path(__file__).resolve().parents[2] / "models" / "pplx-decider-flash-v1.2"


def log(msg: str) -> None:
    print(f"[decider] {msg}", file=sys.stderr, flush=True)


def load(model: Path, fast: bool) -> Decider:
    t = time.perf_counter()
    options = FastOptions(buckets=(2048, 4096)) if fast else False
    decider = Decider.from_pretrained(str(model), device="cuda", fast=options, max_length=4096)
    log(f"loaded {model.name} in {time.perf_counter() - t:.1f}s (fast={fast})")
    if fast:
        t = time.perf_counter()
        decider.warmup()
        log(f"warmup compiled in {time.perf_counter() - t:.1f}s")
    # The first call allocates and autotunes; pay that before the socket opens.
    decider.decide({"state": "warmup", "questions": {"q": {"type": "noul"}}})
    return decider


class Handler(socketserver.StreamRequestHandler):
    decider: Decider
    fast: bool
    lock = threading.Lock()

    def handle(self) -> None:
        for line in self.rfile:
            try:
                msg = json.loads(line)
            except json.JSONDecodeError as e:
                self.reply({"id": None, "error": f"bad json: {e}"})
                continue
            if msg.get("ping"):
                self.reply({"id": msg.get("id"), "pong": True, "device": "cuda", "fast": self.fast})
                continue
            t = time.perf_counter()
            try:
                with self.lock:
                    response = self.decider.decide(msg["request"])
                self.reply({"id": msg.get("id"), "response": response, "ms": (time.perf_counter() - t) * 1000})
            except (RequestError, KeyError, TypeError, ValueError) as e:
                self.reply({"id": msg.get("id"), "error": f"{type(e).__name__}: {e}"})

    def reply(self, obj: dict) -> None:
        self.wfile.write((json.dumps(obj) + "\n").encode())
        self.wfile.flush()


class Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--socket", type=Path, default=DEFAULT_SOCKET)
    p.add_argument("--model", type=Path, default=DEFAULT_MODEL)
    # The packed fast path pads short requests to a 2048-token bucket. For our ~600-token requests it
    # measured 315 ms p50 against 122 ms on the reference path, so it is opt-in.
    p.add_argument("--fast", action="store_true", help="packed, compiled path (throughput, not latency)")
    args = p.parse_args()

    if not torch.cuda.is_available():
        sys.exit("CUDA is not available to torch")
    Handler.decider = load(args.model, fast=args.fast)
    Handler.fast = args.fast
    args.socket.unlink(missing_ok=True)
    with Server(str(args.socket), Handler) as server:
        log(f"ready on {args.socket}")
        server.serve_forever()


if __name__ == "__main__":
    main()
