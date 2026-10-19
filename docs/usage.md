# Usage

## Make targets

| Target | What it does |
|---|---|
| `make setup` | One-time setup. Safe to re-run. |
| `make asr-160` | Builds the 160 ms speech model. `make setup` runs it unless `SKIP_ASR_160=1`. Needs about 8 GB of disk for NVIDIA NeMo. |
| `make decider-kernel` | Optional. Compiles `causal-conv1d` so the decision model runs 5 to 11% faster. Takes about 20 minutes and needs `nvcc`. |
| `make cpu-check` | Live CPU and GPU load with nothing playing, then with speech through the muted speakers. |
| `make demo` | A 50-second demo with a laugh and a question to "Cindy". Uses no speakers. |
| `make run` | Control panel plus the live stand-in. Refuses to start when less than 12 GB of GPU memory is free. |
| `make run-headless` | The stand-in without a window, printing each reaction, alert and decision. |
| `make run-file FILE=x.wav` | Feeds a file in real time instead of the meeting audio. Nothing plays on your speakers. |
| `make play` | Drives the camera by hand. Type a state name such as `nod`, or `!nod` to crossfade immediately. |
| `make listen` | Records 5 s of the meeting audio to `listen.wav`. |
| `make doctor` | Checks the camera, audio capture, models, GPU and sidecar. |
| `make stop` | Stops the stand-in and the decision model. |
| `make test`, `make eval`, `make media`, `make chrome-check`, `make soak FILE=x.wav` | See [Evaluation](evaluation.md). |

## The `standin` binary

`target/release/standin` takes `--config PATH` before any subcommand. The default is `~/.config/digital-standin/config.toml`.

| Command | What it does |
|---|---|
| `standin` or `standin run` | Control panel plus the live stand-in. |
| `standin run --headless [--seconds N]` | No window. Stops after `N` seconds when given. |
| `standin run --audio-file F` | Uses `F` instead of the meeting audio. |
| `standin run --no-decider` | Reflexes only: laughter, nods and name alerts. |
| `standin run --no-camera` | Renders frames without sending them to the camera. |
| `standin replay F [--labels L] [--no-decider] [--no-gate]` | Runs the whole pipeline offline on `F`. Writes `out/<name>-replay.mp4` (with the audio), `.srt` and `.jsonl`, and scores against `F.labels.toml` when present. |
| `standin play`, `standin listen`, `standin doctor` | As in the make targets. |

## The control panel

- **Status bar.** LCD readouts show the state on camera and the one queued next, frames sent and late frames, speech-recognition time per chunk with the share of audio that reached it, and the decision model's status.
- **Camera out.** The preview of what the meeting sees. **HOLD IDLE** freezes the camera on the idle loop until you click **RESUME**. The other buttons trigger a reaction immediately.
- **Live graphs.** The last 2 minutes of:
  - room laughter, applause and audio level, shaded while someone speaks, with ticks for reactions (lime) and alerts (magenta);
  - the decision model's probability for each reaction;
  - speech-recognition and decision latency;
  - camera frame lateness;
  - process CPU;
  - RAM and GPU memory;
  - GPU load.
- **Live transcript.** The open utterance is in italics.
- **What the stand-in did.** Every reaction and alert with its reason. Tick **every decision** to see each model call.
- **Right now.** LED meters for audio, laughter, applause and engagement, and the last decision's probabilities.
- **Settings.** You can turn each reaction on or off and set the confidence it needs, along with every value in the settings file. Changes apply immediately. **SAVE** writes them to the config file.

## Settings

Every value is optional. A missing file means defaults.

```toml
[brain]
aliases = ["Sam", "Samuel"]   # your names, for alerts
disabled = ["confused"]       # reactions never chosen automatically
laugh_threshold = 0.10        # laughter score that triggers a smile
big_laugh = 0.45              # laughter score that triggers a laugh
applause_threshold = 0.5
reaction_threshold = 0.45     # decision-model probability needed to show a reaction
alert_threshold = 0.8         # probability that a question was aimed at you
end_of_utterance_secs = 0.7   # pause that ends an utterance and triggers a decision
decide_every_secs = 2.0       # decision rate during long speech
context_secs = 60.0           # transcript the decision model sees
context_words = 120           # newest words of it at most (bounds decision latency)
nod_min_secs = 7.0            # back-channel nod interval range
nod_max_secs = 16.0
delay_min_secs = 0.25         # human-like delay before a reaction
delay_max_secs = 0.7
min_gap_secs = 2.0            # between any two reactions
same_state_gap_secs = 6.0     # before repeating a reaction
alert_gap_secs = 15.0

[brain.state_thresholds]      # per-state overrides of reaction_threshold
confused = 0.7

[video]
blend_frames = 8              # crossfade length, at 30 fps
device = "/dev/video10"
manifest = "clips/manifest.toml"

[asr]
model = "models/nemotron-160ms"   # unset: the 160 ms export if built, else the 560 ms one
recycle_minutes = 20.0            # speech after which a fresh recognizer process takes over
```

## Clips

`clips/manifest.toml` lists the states and their clips. The default set is cut from a free Mixkit stock portrait by `scripts/make-real-dev-clips.sh`. `clips/manifest-synthetic.toml` uses labelled placeholder cards from `scripts/make-dev-clips.sh`, which make every state change obvious when you debug. To use your own recordings, see [Record your clips](recording-guide.md).

## Troubleshooting

- **Meet does not list the camera.** Chrome lists it only while something writes to it. Start the stand-in first, then reload the Meet tab. Check that `v4l2-ctl --list-devices` shows "Digital Standin".
- **`make run` reports too little GPU memory.** Another process holds the GPU, often vLLM. `nvidia-smi` lists it.
- **No transcript.** The stand-in hears only the system's default output device. In Meet, set **Settings > Audio > Speakers** to **System default**, or to the device your sound settings use. `make doctor` names the device it listens to. To check, run `make listen` while audio plays and confirm `listen.wav` has sound.
- **The decider says "not ready yet".** It loads for about 20 s after start. Its log is `/tmp/digital-standin-decider.log`.
