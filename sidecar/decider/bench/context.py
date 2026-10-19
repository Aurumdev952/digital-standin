"""Decide() latency against transcript length: python bench/context.py"""
import json, statistics, time, copy
from pathlib import Path
import torch
from pplx_decider import Decider

model = Path(__file__).resolve().parents[3] / "models" / "pplx-decider-flash-v1.2"
base = json.loads((Path(__file__).parent / "request.json").read_text())
d = Decider.from_pretrained(str(model), device="cuda", fast=False, max_length=4096)
line = "[ 10s ago] So I guess the ask would be to work with your GTM team and see whether the launch timing still holds for everyone involved."
for n in (2, 5, 10, 20, 30):
    req = copy.deepcopy(base)
    req["state"]["transcript_of_meeting_audio"] = "\n".join([line] * n)
    tokens = d.decide(req)["usage"]["input_tokens"]
    ms = []
    for _ in range(12):
        torch.cuda.synchronize(); t = time.perf_counter(); d.decide(req); torch.cuda.synchronize(); ms.append((time.perf_counter() - t) * 1000)
    print(f"{n:3d} lines {tokens:5d} tokens: p50 {statistics.median(ms):6.1f} ms  max {max(ms):6.1f} ms", flush=True)
