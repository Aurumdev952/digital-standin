"""Times decide() on a realistic stand-in request. Usage: latency.py [--fast]"""
import json, sys, time, statistics
from pathlib import Path
import torch
from pplx_decider import Decider, FastOptions

fast = "--fast" in sys.argv
model = Path(__file__).resolve().parents[3] / "models" / "pplx-decider-flash-v1.2"
req = json.loads((Path(__file__).parent / "request.json").read_text())
t = time.perf_counter()
d = Decider.from_pretrained(str(model), device="cuda", fast=FastOptions(buckets=(2048,)) if fast else False, max_length=4096)
print(f"load {time.perf_counter()-t:.1f}s", flush=True)
if fast:
    t = time.perf_counter(); d.warmup(); print(f"warmup {time.perf_counter()-t:.1f}s", flush=True)
r = d.decide(req)
print("tokens", r["usage"], json.dumps(r["answers"], indent=None)[:900])
ms = []
for _ in range(20):
    torch.cuda.synchronize(); t = time.perf_counter(); d.decide(req); torch.cuda.synchronize(); ms.append((time.perf_counter()-t)*1000)
ms.sort()
print(f"decide ms: p50 {statistics.median(ms):.1f} p90 {ms[17]:.1f} min {ms[0]:.1f} max {ms[-1]:.1f}; vram {torch.cuda.max_memory_allocated()/1e9:.2f} GB")
