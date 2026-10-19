"""Streaming word latency: when each word is emitted against when it ends in the audio.

Usage: python3 -I scripts/word-latency.py <reference words.tsv> <emissions.tsv> <compute ms per chunk>
The reference comes from crates/asr/examples/word_times.rs (Parakeet TDT), the emissions from
crates/asr/examples/transcribe.rs with STANDIN_EMIT_LOG set.
"""
import difflib, re, statistics, sys

ref = []
for line in open(sys.argv[1]):
    w, _, end = line.rstrip("\n").split("\t")
    for tok in re.findall(r"[a-z0-9']+", w.lower()):
        ref.append((tok, float(end)))
text, times = "", []
for line in open(sys.argv[2]):
    t, piece = line.rstrip("\n").split("\t", 1)
    text += piece
    times += [float(t)] * len(piece)
emitted = [(m.group(0).lower(), times[m.end() - 1]) for m in re.finditer(r"[A-Za-z0-9']+", text)]
compute = float(sys.argv[3]) / 1000
sm = difflib.SequenceMatcher(a=[w for w, _ in ref], b=[w for w, _ in emitted], autojunk=False)
lat = sorted(emitted[b.b + k][1] + compute - ref[b.a + k][1] for b in sm.get_matching_blocks() for k in range(b.size))
q = lambda p: lat[min(len(lat) - 1, int(p * len(lat)))]
print(f"{len(lat)} of {len(ref)} reference words matched; latency after the word ends: "
      f"median {statistics.median(lat)*1000:.0f} ms, p10 {q(0.1)*1000:.0f} ms, p90 {q(0.9)*1000:.0f} ms, "
      f"share under 400 ms {100*sum(x < 0.4 for x in lat)/len(lat):.0f}%")
