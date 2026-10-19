#!/usr/bin/env bash
# Synthetic placeholder clips: a labelled card with a "head" disc whose motion encodes the state.
# Every clip starts and ends with the disc at the same neutral position, like real recordings must.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=clips/dev
mkdir -p "$OUT"
FONT=$(fc-match -f '%{file}' 'sans:bold')
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

ffmpeg -loglevel error -y -f lavfi -i "color=c=black@0:s=220x220,format=rgba" -frames:v 1 \
  -vf "geq=r='235':g='196':b='160':a='if(lte(hypot(X-110,Y-110),105),255,0)'" "$TMP/head.png"

# name | seconds | x offset expr | y offset expr   (t in seconds, D = duration)
make() {
  local name=$1 dur=$2 dx=$3 dy=$4
  local fx=${dx//D/$dur} fy=${dy//D/$dur}
  ffmpeg -loglevel error -y \
    -f lavfi -i "color=c=0x2b3a4a:s=1280x720:r=30:d=$dur" -loop 1 -framerate 30 -i "$TMP/head.png" \
    -filter_complex "[0][1]overlay=x='530+($fx)':y='200+($fy)':shortest=1,
      drawbox=x=440:y=430:w=400:h=290:color=0x3d5a73:t=fill,
      drawtext=fontfile=$FONT:text='$name':fontsize=56:fontcolor=white:x=40:y=40,
      drawtext=fontfile=$FONT:text='%{eif\:t*30\:d}':fontsize=28:fontcolor=0x9fb3c8:x=40:y=110" \
    -t "$dur" -c:v libx264 -pix_fmt yuv420p -crf 18 "$OUT/$name.mp4"
  echo "$OUT/$name.mp4"
}

PI=3.14159265
make idle     8   "0"                                   "4*sin(2*$PI*t/D)"
make idle_b   6   "6*sin(2*$PI*t/D)"                    "3*sin(4*$PI*t/D)"
make nod      2   "0"                                   "if(between(t,0.3,1.5),28*sin(2*$PI*(t-0.3)/0.6),0)"
make nod_soft 1.6 "0"                                   "if(between(t,0.3,1.3),12*sin(2*$PI*(t-0.3)/0.5),0)"
make thinking 3.5 "-70*sin($PI*t/D)"                    "-40*sin($PI*t/D)"
make smile    2.5 "0"                                   "-6*sin($PI*t/D)"
make laugh    3   "0"                                   "10*sin(2*$PI*5*t)*sin($PI*t/D)"
make confused 2.5 "35*sin($PI*t/D)"                     "6*sin($PI*t/D)"
