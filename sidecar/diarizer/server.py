"""Serves nvidia/Nemotron-3-Diarization over a Unix socket, streaming.

The client writes raw little-endian f32 samples, 16 kHz mono. Each connection is one session that
starts at session sample 0. After every model chunk the server writes one JSON line:

    {"start": 1.28, "frame": 0.08, "probs": [[p0, ..., p7], ...], "ms": 41.2}

`start` is the session time in seconds of the first frame, `probs` holds one row of eight speaker
probabilities per `frame` seconds, and `ms` is the chunk's compute time. Speaker numbers are stable
within a session only. One session runs at a time; later connections wait.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import sys
import time
from pathlib import Path

import numpy as np
import torch
from transformers import AutoModelForAudioFrameClassification, AutoProcessor

DEFAULT_SOCKET = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "digital-standin-diarizer.sock"
MODEL = "nvidia/Nemotron-3-Diarization"
RATE = 16_000
MODEL_FRAME = 0.01
OUT_FRAME = 0.08
GROUP = round(OUT_FRAME / MODEL_FRAME)


def log(msg: str) -> None:
    print(f"[diarizer] {msg}", file=sys.stderr, flush=True)


class Diarizer:
    def __init__(self, mode: str) -> None:
        t = time.perf_counter()
        self.proc = AutoProcessor.from_pretrained(MODEL)
        self.proc.set_streaming_mode(mode)
        self.model = AutoModelForAudioFrameClassification.from_pretrained(MODEL, dtype=torch.bfloat16).cuda().eval()
        log(f"loaded {MODEL} in {time.perf_counter() - t:.1f}s ({mode}, {self.proc.streaming_latency_ms} ms)")

    def session(self):
        """Yields (start_sample, sample_count) for each chunk the model needs, in order."""
        p = self.proc
        yield 0, p.num_samples_first_audio_chunk
        mel = 0
        while True:
            mel += p.num_mel_frames_per_step
            yield p.audio_chunk_start(mel), p.num_samples_per_audio_chunk

    def step(self, chunk: np.ndarray, first: bool, cache):
        inputs = self.proc(chunk, sampling_rate=RATE, is_streaming=True, is_first_audio_chunk=first, is_last_audio_chunk=False)
        inputs = inputs.to("cuda", dtype=torch.bfloat16)
        with torch.inference_mode():
            out = self.model(**inputs, speaker_cache=cache)
        return torch.sigmoid(out.logits[0].float()).cpu().numpy(), out.speaker_cache


def serve(conn: socket.socket, d: Diarizer) -> None:
    buf = np.zeros(0, dtype=np.float32)
    base = 0  # session sample index of buf[0]
    pending = b""
    cache = None
    rows = np.zeros((0, 8), dtype=np.float32)  # 10 ms rows not yet grouped
    emitted = 0  # 80 ms frames sent so far
    chunks = d.session()
    start, n = next(chunks)
    first = True
    while True:
        data = conn.recv(1 << 16)
        if not data:
            return
        data = pending + data
        whole = len(data) - len(data) % 4
        pending = data[whole:]
        buf = np.concatenate([buf, np.frombuffer(data[:whole], dtype="<f4")])
        while base + len(buf) >= start + n:
            t = time.perf_counter()
            # The first chunk can start before sample 0; the processor pads it.
            lo = max(start - base, 0)
            probs, cache = d.step(buf[lo : start + n - base], first, cache)
            ms = (time.perf_counter() - t) * 1000
            first = False
            rows = np.concatenate([rows, probs])
            k = len(rows) // GROUP
            if k:
                frames = rows[: k * GROUP].reshape(k, GROUP, -1).mean(1)
                rows = rows[k * GROUP :]
                line = {"start": round(emitted * OUT_FRAME, 3), "frame": OUT_FRAME, "probs": np.round(frames, 3).tolist(), "ms": round(ms, 1)}
                conn.sendall((json.dumps(line) + "\n").encode())
                emitted += k
            start, n = next(chunks)
            drop = max(start - base, 0)
            buf = buf[drop:]
            base += drop


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--socket", type=Path, default=DEFAULT_SOCKET)
    p.add_argument("--mode", default="ultra_low_latency")
    args = p.parse_args()

    if not torch.cuda.is_available():
        sys.exit("CUDA is not available to torch")
    d = Diarizer(args.mode)
    # The first forward allocates and autotunes; pay that before the socket opens.
    t = time.perf_counter()
    d.step(np.zeros(d.proc.num_samples_first_audio_chunk, dtype=np.float32), True, None)
    log(f"warmup in {time.perf_counter() - t:.1f}s")
    args.socket.unlink(missing_ok=True)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as srv:
        srv.bind(str(args.socket))
        srv.listen(4)
        log(f"ready on {args.socket}")
        while True:
            conn, _ = srv.accept()
            with conn:
                try:
                    serve(conn, d)
                except (BrokenPipeError, ConnectionResetError):
                    pass
            log("session ended")


if __name__ == "__main__":
    main()
