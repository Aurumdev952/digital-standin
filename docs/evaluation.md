# Evaluation

## Tests

`make test` runs the unit and integration tests:

- **Video:** manifest validation, the player state machine (loops, one-shots, urgent crossfades, wrap-around), and the YUYV blend.
- **Audio:** block numbering, the gate, and dropped consumers.
- **Cues:** the silence gate (pre-roll, hangover, reset), the mel filterbank, and a parity test of our CED frontend against sherpa-onnx's output on the talk-show recording.
- **Brain:** settings, transcript windowing, name matching (including names split across recognizer chunks), the decision request and response, and the policy. The policy tests cover laughter timing, gaps, nods, alerts, priorities, deferral, and disabled reactions.

The parity test needs the models and the test media, and skips itself without them.

## Scoring tools

| Tool | What it measures |
|---|---|
| `make media` | Downloads two public YouTube recordings: a GitLab team meeting on Zoom and a talk-show compilation. It cuts 16 kHz excerpts and mixes 12 ESC-50 laughs into the meeting at known times (`scripts/make-laugh-mix.py`). ESC-50 is CC BY-NC, for local testing only. |
| `make eval` | Replays every labelled recording through the whole pipeline on the audio's own clock, with the decision model, and writes `out/eval-summary.txt`. |
| `standin replay F` | One replay. Watch `out/<name>-replay.mp4` with its `.srt` captions to see each reaction and why it happened. |
| `make cpu-check` | Live CPU and GPU load with nothing playing, then with speech through the muted speakers. |
| `make obs-check` | Records the camera with OBS Studio on a virtual display with a throwaway OBS profile, and reports what OBS captured. |
| `make chrome-check` | Opens the camera in headless Chrome with `getUserMedia`, as Meet does. Reports resolution and frame rate, and saves snapshots to `out/chrome-check/`. |
| `make soak FILE=x.wav` | Plays a long recording through the muted speakers into a headless stand-in. Samples memory, GPU memory and CPU every 30 s. |
| `cargo run --release -p cues --example tagger_eval` | Sweeps tagger model, window, hop and threshold against labelled laughter and laughter-free audio. |

Labels live next to each recording as `<name>.labels.toml`. The persona configs in `test-media/personas/` replay the meeting as three listeners: Cindy, who is asked a question, Brian, who is thanked by name, and Sam, who is never mentioned.

## Results

Measured on an RTX 5090 Laptop GPU on 2026-10-19, with the 160 ms speech model. The laughter, persona, gate and decision-latency rows come from `make eval` run on its own; the CPU row from `make cpu-check`.

| Check | Result |
|---|---|
| Laughter, 12 laughs mixed into a meeting | 12/12 caught, median reaction 0.56 s after onset, no smiles outside the laughs |
| Talk-show audience reactions (caption labels) | 3/3 |
| Laughter-free meeting, 5 min | No smiles or laughs |
| Persona Cindy, asked "what do you think Cindy" | Alerted, showed `thinking` |
| Persona Brian, thanked by name | Alerted, no `thinking` |
| Persona Sam, never mentioned | No alerts |
| Decision model latency, alone on the GPU | 104 to 122 ms p50, 120 to 156 ms p95 across the scenarios, with the transcript capped at 120 words (124 to 212 ms p50 uncapped) |
| Audio to recognised word, 160 ms model | Median 181 ms after the word ends, p90 421 ms, 85% under 400 ms, against Parakeet TDT word timings (`scripts/word-latency.py`); 347 ms median with the 560 ms model |
| Decision latency against prompt length | About 0.2 ms per token: 104 ms at 530 tokens, 297 ms at 1,482. `make decider-kernel` makes it 5 to 11% faster |
| Speech recognition, 160 ms model | 21 ms GPU time per 160 ms chunk, 6.3× faster than real time; words a median 0.16 s sooner than the 560 ms model, 98% identical words |
| Silence gate, recording half silence and noise | Recognition ran on 49% of the audio, transcript kept 99% of the words |
| Live CPU, nothing playing / speech playing | 6% / 45% of one core (35% with the 560 ms model); GPU load about 12% during speech |
| Camera in Chrome | 1280×720, 29.95 to 30.0 fps, motion in 99.8% of frames |
| Camera in OBS Studio 30.0.2 | Captured `/dev/video10` as 1280×720 YUYV at 30 fps; recorded 1037 frames in 35 s showing the stand-in |
| 43-minute live soak through muted speakers, default settings, run twice | 0 late frames in 77,818 both times; main process 1067 to 1077 MB and 1068 to 1078 MB; speech worker recycled at 20 and 40 minutes, each generation 1.23 to 1.29 GB; 218 and 214 reactions, no errors. Chrome and OBS read the camera during the second run |
| 43-minute live soak through muted speakers, before the speech worker | 5 late frames in 77,813 (while replays and builds also ran), GPU memory flat, process memory +2.0 MB/min |
| Memory growth by component, 12 minutes each | Before the speech worker: file audio without camera, PipeWire capture without camera, and file audio with camera all grew 1 to 2 MB/min, so the growth was the speech model's, not capture's or the camera's. |
| Memory with the recycled speech worker, 12 minutes, recycling every 2 minutes | Main process 1224 to 1228 MB. Worker replaced 3 times without errors; each generation 1.23 to 1.26 GB. 0 late frames in 22,123. |
| Capture with the sink muted or at 0% volume | Bit-identical to unmuted capture |

## What the numbers do not cover

- The laughter set is 12 clips mixed into one meeting. Real meeting laughter, often several people over speech, may score differently. Tune `laugh_threshold` in the control panel.
- With no name configured, "can you comment on the issue" (asked of someone unnamed) scores 0.81 for "addressed to you" and raises an alert; with any name set it does not. Set your name.
- The persona checks use one recorded meeting. "Addressed to you" depends on the model reading the situation correctly, and fails more often without a name.
- Nobody has run it in a live Google Meet call yet. The Chrome check uses the same browser API, but Meet's own processing (background blur, low-light correction) is untested.
