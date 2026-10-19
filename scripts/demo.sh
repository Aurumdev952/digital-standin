#!/usr/bin/env bash
# A 50-second demo: meeting talk with a mixed-in laugh at 17.7 s and "what do you think Cindy" at
# 39 s, played into the stand-in as the listener "Cindy". Nothing plays on your speakers.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -s test-media/meeting-laughs.wav ] || { scripts/fetch-test-media.sh; python3 -I scripts/make-laugh-mix.py; }
[ -s test-media/demo.wav ] || ffmpeg -loglevel error -y -ss 250 -t 50 -i test-media/meeting-laughs.wav test-media/demo.wav
exec scripts/run.sh --config test-media/personas/cindy.toml --audio-file test-media/demo.wav "$@"
