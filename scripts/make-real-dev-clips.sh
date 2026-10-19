#!/usr/bin/env bash
# Realistic development clips cut from one free Mixkit portrait (clip 43834, Mixkit free licence):
# one person, one locked camera, so crossfades can be judged on a real face. Thinking and confused
# reuse the head-tilt segment; your own recordings replace all of this.
set -euo pipefail
cd "$(dirname "$0")/../clips"
mkdir -p src real
[ -s src/mixkit-43834.mp4 ] || curl -fsSL -A "Mozilla/5.0" -o src/mixkit-43834.mp4 https://assets.mixkit.co/videos/43834/43834-720.mp4
SRC=src/mixkit-43834.mp4
cut() { ffmpeg -loglevel error -y -ss "$2" -to "$3" -i "$SRC" -an -vf "fps=30,scale=1280:720" -c:v libx264 -crf 16 -pix_fmt yuv420p "real/$1.mp4"; }
pingpong() {
  ffmpeg -loglevel error -y -ss "$2" -to "$3" -i "$SRC" -an \
    -filter_complex "[0]fps=30,scale=1280:720,split[a][b];[b]reverse[r];[a][r]concat=n=2:v=1[v]" -map "[v]" \
    -c:v libx264 -crf 16 -pix_fmt yuv420p "real/$1.mp4"
}
pingpong idle   2.6 4.0
pingpong idle_b 5.0 6.1
cut smile    0.0 2.8
pingpong laugh  0.7 1.9
cut nod      3.6 6.1
pingpong thinking 3.9 5.0
pingpong confused 4.2 5.6
ls real
