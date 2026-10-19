# Digital Standin

A virtual webcam that stands in for you in video meetings. It listens to the meeting audio, works out what is happening, and plays short recorded clips of you that fit: listening, nodding, smiling at a joke, laughing along, looking thoughtful at a question. When someone says your name or asks you something directly, it alerts you so you can take over.

Everything runs on your machine. No audio or transcript leaves it.

- Hears the meeting even with your speakers muted (PipeWire sink monitor).
- Transcribes speech on the GPU with NVIDIA Nemotron streaming ASR, and only while someone is talking.
- Detects laughter and applause with an AudioSet tagger, and reacts within about a second.
- Decides on subtler reactions with `perplexity-ai/pplx-decider-flash-v1.2`, a local decision model.
- Publishes a 1280×720, 30 fps camera that Google Meet, Zoom or any browser can select.
- Comes with a control panel for tuning, and a replay tool that scores it on recorded meetings.

## Requirements

- Linux with PipeWire (tested on Linux Mint 22.3).
- An NVIDIA GPU with about 12 GB free (tested on an RTX 5090 Laptop GPU), CUDA 13 and cuDNN 9.
- Rust, [`uv`](https://docs.astral.sh/uv/), ffmpeg and the Hugging Face CLI (`hf`).
- A Hugging Face token with access to `perplexity-ai/pplx-decider-flash-v1.2`, which is a gated repository. Run `hf auth login` once.

## Set up

```sh
git clone https://github.com/Aurumdev952/digital-standin.git
cd digital-standin
make setup
```

`make setup` installs the system packages (it asks for `sudo`), loads the virtual camera, downloads about 11 GB of models, builds the low-latency speech model, prepares the decision model's Python environment, builds the app, and runs `standin doctor`. Every line of the doctor output should read `ok`. The speech-model step installs NVIDIA NeMo (about 8 GB) and takes a few minutes; `SKIP_ASR_160=1 make setup` skips it and keeps the published model, which delivers words about 0.16 s later.

## Run

```sh
make run
```

Then:

1. Type your name into **YOUR NAMES** in the control panel and click **SAVE**.
2. In Google Meet, open **Settings > Video** and pick **Digital Standin**.
3. Mute your speakers if you like. The stand-in still hears the meeting.

`make stop` stops everything and frees the GPU. `make help` lists the other targets.

The default clips come from a free stock video. To use clips of yourself, follow the recording guide.

## Documentation

- [Try it yourself](docs/try-it.md): a step-by-step walkthrough that checks every part.
- [Usage](docs/usage.md): commands, the control panel, and the settings file.
- [How it works](docs/architecture.md): the pipeline, design decisions, and known limits.
- [Evaluation](docs/evaluation.md): tests, scoring tools, and measured results.
- [Record your clips](docs/recording-guide.md): how to film and register your own reactions.

## License

MIT. The models keep their own licenses: NVIDIA Open Model License for Nemotron, Apache-2.0 for pplx-decider and CED, and MIT for Earshot.
