# How it works

## Pipeline

```
PipeWire sink monitor ──► 16 kHz mono ──► voice activity + laughter tagger (CPU) ──────────┐
  (before volume and mute)                 │ speech only, through the Gate                  │ cues
                                           └──► speech-to-text (Nemotron 0.6B, GPU) ────────┤ words
                                                                                            ▼
                     decision model (pplx-decider, GPU, Python sidecar) ◄──► brain: reflexes + arbiter ──► alerts
                                                                                  │ reaction
                                                clips in RAM ──► 30 fps player, crossfades ──► /dev/video10 ──► Meet
```

Every stage runs on its own thread and hands data on through channels. A slow decision never stalls the camera.

| Crate | Role |
|---|---|
| `crates/audio` | Captures the default sink's monitor through PipeWire as 16 kHz mono, and follows the default sink when you switch devices. Fans each block out to consumers. |
| `crates/cues` | Earshot voice activity, the CED-small AudioSet tagger for laughter and applause, and the `Gate` that passes only speech on to recognition. |
| `crates/asr` | Streams audio through `nvidia/nemotron-speech-streaming-en-0.6b` with `parakeet-rs` on ONNX Runtime CUDA, in 160 ms chunks (or 560 ms with the published export). Live, it runs in a worker process (`standin asr-worker`) that is replaced every 20 minutes of speech. |
| `crates/brain` | The decision policy: the rolling transcript, reflexes, decision-model requests, the arbiter, and name matching. It is driven by an explicit clock, so live runs and replays use the same code. |
| `crates/video` | Clip manifest, decoding into RAM, the playback state machine, crossfades, and the v4l2loopback writer. |
| `app` | The `standin` binary: control panel, headless mode, replay and scoring, doctor, alerts, and sidecar management. |
| `sidecar/decider` | Serves the decision model over a Unix socket, one JSON object per line. |

## Decisions

**Reflexes.** Laughter above `laugh_threshold` triggers a smile, and above `big_laugh` a laugh, 0.15 to 0.4 s later. While someone keeps talking, a soft nod follows every 7 to 16 s; lively talk shortens that interval and dull talk stretches it. A name match on your aliases raises an alert at once.

**The decision model.** When an utterance ends (0.7 s of silence), and every 2 s during long speech, the brain sends one request with four questions:
- `reaction`: which state fits, chosen from the manifest's state descriptions.
- `addressed`: was the latest utterance aimed at you?
- `question`: did it ask a question?
- `engagement`: how lively is the talk?

The model answers with probabilities in a single forward pass of about 125 ms. If you were addressed, the stand-in alerts you and shows `thinking`.

**The arbiter.** Every reaction waits a human-like delay. Reactions keep a minimum gap, and no state repeats within `same_state_gap_secs`. Priorities run manual, then laughter, then being addressed, then the decision model, then nods. A reaction blocked by the gap waits up to 2 s rather than being lost.

**Video.** All clips are decoded once into RAM as YUYV, so playback is a memory copy. One-shot clips finish their motion before the next request plays. Loops yield at once. Every clip change crossfades over `blend_frames`. Frames are paced on a monotonic clock, and a late frame repeats rather than stalling the stream.

## Silence costs almost nothing

The cue thread's voice detector gates speech recognition. It keeps 0.6 s of pre-roll, so the first word arrives whole, and holds the gate open 1.2 s after speech, so the recognizer commits the last words. After 3 s of silence the recognizer restarts from a clean state. The laughter tagger skips windows quieter than -50 dBFS, the voice detector skips digital silence, and the decision model is asked only when new words arrive.

## Why it is built this way

- **One ONNX Runtime per process.** sherpa-onnx bundles its own ONNX Runtime, which replaced the CUDA-enabled one at load time. CED and voice activity therefore run through the same `ort` build as the speech model, sharing one environment. `tests/parity.rs` checks that our CED frontend matches sherpa-onnx's.
- **Earshot instead of Silero.** Running Silero through ONNX Runtime 31 times a second grew the process by about 0.55 KB per call, and no session drop released it (`crates/cues/examples/ortgrowth.rs`). Earshot is pure Rust and matched Silero as a gate.
- **CED-small instead of CED-mini.** On 12 laughs mixed into a meeting, CED-small caught 12 with no false episodes in 5 minutes of plain meeting. CED-mini caught 11, with 0.6 false episodes a minute.
- **160 ms recognition chunks.** The published ONNX export fixes 560 ms chunks. `scripts/export-asr-160.sh` re-exports NVIDIA's checkpoint with a right attention context of 1. On the test meeting, words arrived a median 0.16 s sooner (p90 0.40 s), and 98% of the words were identical, for about 2.7 times the GPU work (21 ms per 160 ms chunk).
- **The decision model's reference path.** The packed fast path pads short requests to 2048 tokens. It measured 315 ms against 122 ms p50.
- **A recyclable speech worker.** The CUDA driver keeps about 2 MB a minute for the speech model's kernel launches, and releases it only when the process exits. Recognition therefore runs in a child process. After `recycle_minutes` of speech, a replacement loads in the background and takes over where speech resumes after a pause, and the old one flushes its last words and exits. The main process stays flat, and each worker is retired before it has grown 40 MB.
- **A Python sidecar.** The decision model needs its own package, with noncausal attention and per-question readouts. A separate process also means a crash there never stops the camera.
- **v4l2loopback with `exclusive_caps=1`.** Chrome lists a loopback device only with exclusive caps. Chrome's PipeWire camera support is still behind a flag.

## Known limits

- **Recognition latency.** With 160 ms chunks, words reach the brain about 0.2 to 0.5 s after they are spoken. With `SKIP_ASR_160=1`, the published 560 ms export adds about 0.16 s more. Laughter reactions do not depend on either.
- **No speaker labels.** The transcript doesn't say who spoke. The decision model judges "addressed to you" from wording and your name, so set your name.
- **Ghosting.** Crossfading between clips with different head positions shows a faint double outline. Clips that start and end on the neutral pose avoid it.
