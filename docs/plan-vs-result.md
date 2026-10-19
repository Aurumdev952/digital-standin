# Plan against result

This page compares each success criterion in the original build plan with what was measured on the finished system, an RTX 5090 Laptop GPU, on 2026-10-19. Where a result differs from the plan, the reason follows.

## Measured targets

| Plan criterion | Target | Result | Status |
|---|---|---|---|
| Camera listed by Chrome and usable as a meeting camera | Listed as "Digital Standin" | Listed in headless Chrome, which received 1280×720 at 30 fps; also captured by ffmpeg and OBS Studio 30.0.2 | Met |
| Camera frame pacing | Jitter under 2 ms at 30 fps | Mean 0.03 to 0.04 ms; worst single frame 3.7 to 9 ms; 0 late frames in 77,818 over 43 minutes, twice | Met on average; occasional single frames exceed 2 ms but stay far inside the 33 ms frame period |
| Transcript flows with the speakers muted | Works when muted | Capture with the sink muted or at 0% volume is bit-identical to unmuted; live transcription through the muted sink confirmed | Met |
| Audio to recognised word | Under 400 ms | Median 181 ms after the word ends, p90 421 ms, 85% of words under 400 ms (160 ms model, against Parakeet TDT word timings, `scripts/word-latency.py`) | Met for the typical word. The slowest 15% are the model's own emission delay: an 80 ms-chunk export measured the same (median 180 ms, p90 420 ms) |
| Laughter reaction | Within about 500 ms, few false positives | Median 0.56 s after laughter onset, 12 of 12 laughs, no reaction outside laughter in 5 minutes of plain meeting | Close: 60 ms over, with no false positives |
| Decision-model latency | Under 150 ms; if missed, shorten the context | 107 to 123 ms p50, 120 to 156 ms p95, on every labelled recording, with the context capped at 120 words | Met, by the remedy the plan named |
| Replay with review video and scoring | Deterministic replay, labelled scoring | `standin replay` writes the video with audio, captions and a decision log, and scores against labels; `make eval` runs all of them | Met |
| Alerts for name and direct questions | Alert when named or asked | Personas: asked "what do you think Cindy" → alert and `thinking`; thanked by name → alert only; never mentioned → no alert | Met |
| One-hour run | Memory, GPU memory and frame drops checked | Two 43-minute runs: 0 late frames; main process flat (1067 to 1078 MB); GPU memory flat; the speech worker recycled every 20 minutes | Met |
| Switch to your own clips by changing only the manifest | Manifest only | `clips/manifest.toml` and `clips/manifest-synthetic.toml` swap with one setting | Met |

## Where the build differs from the plan, and why

- **Laughter is 60 ms slower than planned.** The tagger has to hear enough laughter to recognise it, about half a second, so the reaction now follows detection with no added delay. Checking more often (a 62.5 ms hop) did not detect sooner. Lower thresholds (0.06 and 0.08) did not either: through the whole pipeline they reacted at 0.59 and 0.66 s and added stray smiles to the laughter-free meeting. So the default stays at 0.10, the fastest setting measured that adds no stray smiles.
- **Words take the model's own time to commit.** Smaller recognition chunks do not shorten the slowest words: the 80 ms export matched the 160 ms one (p90 420 ms) at twice the GPU work, so 160 ms stays.
- **The laughter tagger is CED-small, not CED-mini.** CED-mini missed 1 of 12 laughs and produced 0.6 false episodes a minute.
- **Voice activity uses Earshot, not Silero, and ONNX Runtime runs inside our process, not through sherpa-onnx.** sherpa-onnx brought a second ONNX Runtime that replaced the CUDA-enabled one. Silero, run through ONNX Runtime 31 times a second, grew the process by about 150 MB per meeting.
- **Speech recognition runs in a worker process that is replaced every 20 minutes of speech.** The CUDA driver keeps about 2 MB a minute for the speech model's kernel launches and frees it only at process exit. This was not in the plan. It keeps memory bounded.
- **The 160 ms speech model comes from a NeMo re-export.** The published ONNX export fixes 560 ms chunks. `make setup` builds the 160 ms one (`SKIP_ASR_160=1` skips it).
- **The decision model's fast path is unused.** It pads short prompts to 2048 tokens and measured 315 ms against 122 ms.
- **The Kyutai fallback was not needed.** Nemotron ran on this GPU at 6.3× real time.

## Not yet verified

- **A live Google Meet call.** Step 5 of [Try it yourself](try-it.md) walks through it with a phone as the second participant. The Chrome check uses the same browser API Meet uses, but Meet's own video effects are untested.
- **Clips of you.** The default clips are cut from a stock video. Ghosting between different head positions is the main risk, and the [recording guide](recording-guide.md) is built around avoiding it.
