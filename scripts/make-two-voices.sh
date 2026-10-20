#!/usr/bin/env bash
# Builds test-media/two-voices.wav: 8 s turns alternating between two speakers, A B A B.
set -euo pipefail
cd "$(dirname "$0")/.."
ffmpeg -v error -y -ss 17.6 -t 8 -i test-media/meeting.wav -ss 46.6 -t 8 -i test-media/laughs.wav \
  -ss 41.5 -t 8 -i test-media/meeting.wav -ss 76.8 -t 8 -i test-media/laughs.wav \
  -filter_complex "[0][1][2][3]concat=n=4:v=0:a=1" -ac 1 -ar 16000 test-media/two-voices.wav
echo "wrote test-media/two-voices.wav"
