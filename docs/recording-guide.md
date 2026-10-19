# How to record your stand-in clips

The stand-in cuts between short clips of you. Transitions look seamless only when every clip shares the same camera, light and starting pose. Record everything in one session, in one long take, and cut it afterwards.

## Set up once

1. Put the camera on a tripod or a fixed mount at your usual meeting height and distance. Do not touch it again until you finish.
2. Turn off auto exposure, auto white balance and autofocus. A phone camera app with a "pro" mode works. A webcam needs `v4l2-ctl` or its vendor tool.
3. Use lamps, not a window. Daylight drifts within minutes and shows up as a jump between clips.
4. Wear what you wear in meetings. You will reuse these clips for weeks.
5. Record at 1280×720 or 1920×1080, 30 fps. The stand-in scales to 1280×720.

## Learn the neutral pose

Every clip must start and end in the same neutral pose: eyes on the lens, relaxed face, shoulders still. Hold it for one full second at the start and at the end of each reaction. The player crossfades for about 270 ms between clips (`crossfade frames`, default 8). Crossfading between two different head positions shows a ghosted double outline, so a reaction that returns to neutral is what keeps the cut invisible.

## Record the reactions

Record each item below in the take, with two seconds of neutral between items. Count out loud before each one if it helps you find them later. The audio is discarded.

| State | How many | Length | What to do |
|---|---|---|---|
| `idle` | 3 | 20 to 30 s each | Listen attentively. Blink, breathe, shift slightly. No big movements. |
| `nod` | 3 | 2 to 3 s | A small nod, a double nod, and an "mm-hm" nod. |
| `thinking` | 2 | 3 to 4 s | Look up or aside as if weighing a question, then come back to the lens. |
| `smile` | 2 | 2 to 3 s | A warm smile and a smirk at something mildly funny. |
| `laugh` | 2 | 3 s | A light laugh and a fuller one, mouth closed or nearly closed. |
| `confused` | 1 | 2 to 3 s | A slight frown or head tilt. |
| optional | any | any | Taking a note, a sip of water, adjusting posture. These add variety in long meetings. |

Keep reactions smaller than feels natural. On a call, a face fills the frame and big motions read as theatrical.

## Cut and register the clips

1. Write a timestamps file with one line per clip: state, start and end in seconds.

	```
	idle      12.0  38.5
	nod       41.0  43.6
	thinking  47.2  51.0
	```

2. Cut the clips with ffmpeg, one command per line:

	```sh
	ffmpeg -ss 41.0 -to 43.6 -i take.mp4 -an -vf fps=30,scale=1280:720 -c:v libx264 -crf 16 clips/me/nod.mp4
	```

3. Copy `clips/manifest.toml` to `clips/manifest-me.toml` and point each `[[clips]]` entry at your files. Add an entry per clip. Several clips for one state are picked at random, which hides repetition.
4. Set `manifest = "clips/manifest-me.toml"` under `[video]` in `~/.config/digital-standin/config.toml`.
5. Run `standin play` and type state names to check each transition before your first meeting.

## Tune the descriptions

Each state in the manifest has a `description`. The decision model reads it to choose a reaction, so describe when the reaction fits, not what it looks like. You can add states such as `writing_notes` with their own description and clips. The decision model picks them up with no code change.
