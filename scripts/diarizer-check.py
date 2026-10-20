"""Streams a file to the diarizer sidecar in real time and prints who spoke in each window.

    python3 -I scripts/diarizer-check.py test-media/two-voices.wav [--turn 8]

Prints the majority speaker per `--turn` seconds and the per-chunk compute time.
"""

import argparse
import json
import os
import socket
import statistics
import subprocess
import threading
import time
from pathlib import Path

RATE = 16_000
BLOCK = RATE // 50

p = argparse.ArgumentParser()
p.add_argument("audio", type=Path)
p.add_argument("--turn", type=float, default=8.0)
p.add_argument("--socket", type=Path, default=Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "digital-standin-diarizer.sock")
args = p.parse_args()

pcm = subprocess.run(
    ["ffmpeg", "-v", "error", "-nostdin", "-i", str(args.audio), "-ac", "1", "-ar", str(RATE), "-f", "f32le", "-"],
    check=True, capture_output=True,
).stdout
total = len(pcm) / 4 / RATE

s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(str(args.socket))
frames, ms = [], []


def read() -> None:
    for line in s.makefile("r"):
        msg = json.loads(line)
        ms.append(msg["ms"])
        for i, row in enumerate(msg["probs"]):
            frames.append((msg["start"] + (i + 1) * msg["frame"], row))


reader = threading.Thread(target=read)
reader.start()
t0 = time.monotonic()
step = BLOCK * 4
for i in range(0, len(pcm), step):
    due = t0 + i / 4 / RATE
    time.sleep(max(0.0, due - time.monotonic()))
    s.sendall(pcm[i : i + step])
time.sleep(1.0)
s.shutdown(socket.SHUT_WR)
reader.join()

print(f"{len(ms)} chunks, {len(frames)} frames covering {frames[-1][0] if frames else 0:.2f} of {total:.2f} s")
turn = 0.0
while turn < total:
    rows = [r for end, r in frames if turn < end <= turn + args.turn]
    sums = [sum(r[k] for r in rows) for k in range(8)]
    best = max(range(8), key=lambda k: sums[k]) if rows else None
    share = sums[best] / len(rows) if rows else 0
    print(f"[{turn:5.1f}, {turn + args.turn:5.1f})  speaker {best}  mean p {share:.2f}")
    turn += args.turn
w = sorted(ms[3:])
print(f"ms per chunk p50 {statistics.median(w):.1f} p90 {w[int(len(w) * 0.9)]:.1f} max {w[-1]:.1f}")
