# Try it yourself

This walkthrough checks every part of Digital Standin in about 30 minutes, from the camera alone up to a real Google Meet call. Each step says what you should see. If a step fails, stop there and check [Troubleshooting](usage.md#troubleshooting).

Run every command from the repository folder, `~/aurum/projects/digital-standin`.

## 0. Free the GPU and check the machine

The stand-in needs about 12 GB of GPU memory. If vLLM or another model server is running, stop it first.

```sh
nvidia-smi --query-gpu=memory.used,memory.free --format=csv
make doctor
```

You should see every line of `make doctor` read `ok`. The speech model line should say `160 ms chunks`. If it says `560 ms chunks`, the app still works; `make asr-160` builds the faster model.

## 1. See the camera on its own

```sh
make play
```

1. In Chrome, open the camera page of any video site, or <https://webcamtests.com>, and pick **Digital Standin** as the camera.
2. You should see the stock portrait sitting idle, moving slightly.
3. In the terminal, type `nod` and press Enter. The person nods once, then returns to idle.
4. Try `smile`, `laugh`, `thinking` and `confused`. Type `!laugh` to crossfade at once instead of waiting.
5. Press Ctrl+D to stop.

To check the camera with OBS Studio instead, leave `make play` running and run `make obs-check` in a second terminal. It prints `OBS recorded N frames at 1280x720` and saves a contact sheet to `out/obs-check.png`.

## 2. Check that muted audio is still heard

1. Play any YouTube video with speech in your browser.
2. Mute your speakers or headset with the system volume control.
3. Run `make listen`, wait 5 seconds, then play the recording with `pw-play listen.wav` after unmuting.

You should hear the video's audio in `listen.wav`, even though you heard nothing while it recorded.

## 3. Watch a 50-second demo

```sh
make demo
```

The control panel opens and plays a 50-second meeting excerpt into the stand-in, without using your speakers, as the listener "Cindy". The decision model takes about 20 seconds to load. Until then the status bar says `decision model: starting`.

You should see these, roughly at these times on the event log:

| Time | What happens |
|---|---|
| about 10 s | A back-channel nod during speech. |
| about 19 s | A smile, then a laugh, at the laughter mixed into the recording. |
| about 39 s | A red alert line, `someone said "Cindy"`, a desktop notification and a short chime. |
| about 40 s | The stand-in looks thoughtful, because Cindy was asked a question. |

Meanwhile, check the panel:
- **Left:** the camera preview and the live graphs. Room activity shows reaction ticks. Below are the decision model's probabilities, latency, frame lateness, CPU, memory and GPU.
- **Middle:** the live transcript and what the stand-in did, with the reason.
- **Right:** the settings.

Close the window when it finishes.

## 4. Tune it

Run `make run` and play a YouTube meeting recording or a podcast while you watch the panel. Try:

- Untick **laugh** under **REACTIONS**. Laughter now only ever gives a smile.
- Move **nod every at least/most** down to 3 and 6 seconds. Nods become more frequent during speech.
- Raise **smile at laughter** to 0.4. Only clear laughter triggers a reaction.
- Click a reaction button under the camera. It plays at once, even while **HOLD IDLE** is on.

Click **SAVE** to keep your changes in `~/.config/digital-standin/config.toml`.

## 5. Use it in a real Google Meet call

1. Run `make run`. Type your name under **YOUR NAMES** and click **SAVE**.
2. Open <https://meet.google.com/new> in Chrome. In the preview, choose **Digital Standin** as the camera, then join.
3. Join the same meeting from your phone, as the other participant. Keep the phone's camera on so you can see the laptop's tile.
4. Mute the laptop's speakers.
5. On the phone, talk for a while, tell a joke and laugh, then ask "What do you think, \<your name\>?".

You should see, on the phone: the stand-in nodding now and then, smiling or laughing when you laugh, and looking thoughtful after your question. On the laptop you should get a notification for your name and for the question.

Things to judge with your own eyes: whether the reactions feel natural, whether any transition looks wrong, and how Meet's own effects such as background blur treat the video.

## 6. Score it on recordings (optional)

```sh
make media   # once: downloads the test recordings and builds the labelled laughter mix
make eval    # about 15 minutes
```

`out/eval-summary.txt` should match the numbers in [Evaluation](evaluation.md). To see what it did, open `out/meeting-laughs-replay.mp4` in a video player with its `.srt` captions. Each reaction shows as a caption with its reason.

## 7. Record your own clips (optional)

Follow [Record your clips](recording-guide.md). Then point `manifest` under `[video]` in the settings file at your manifest, and restart.

## 8. Stop everything

```sh
make stop
```

You should see `stopped; GPU memory in use:` with a small number. Restart vLLM afterwards if you need it.
