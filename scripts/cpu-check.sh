#!/usr/bin/env bash
# Live CPU and GPU use of the stand-in with nothing playing, then with meeting speech played through
# the muted default speakers. Usage: scripts/cpu-check.sh [config]
cd "$(dirname "$0")/.."
mkdir -p out
CONFIG=${1:-}
WAS=$(pactl get-sink-mute @DEFAULT_SINK@ | awk '{print $2}')
trap '[ "$WAS" = no ] && pactl set-sink-mute @DEFAULT_SINK@ 0' EXIT
pactl set-sink-mute @DEFAULT_SINK@ 1
target/release/standin ${CONFIG:+--config "$CONFIG"} run --headless --no-camera --seconds 150 > out/cpu-check.log 2>&1 &
P=$!
sleep 15
measure() {
  local a=$(ps -o cputimes= -p $P | tr -d ' ')
  local g=0
  for i in $(seq 1 12); do g=$((g + $(nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits))); sleep 5; done
  local b=$(ps -o cputimes= -p $P | tr -d ' ')
  echo "$1: process CPU $(( (b - a) * 100 / 60 ))% of one core, GPU utilisation avg $((g / 12))%"
}
measure "silence (nothing playing)"
pw-play test-media/meeting.wav & PL=$!
sleep 3
measure "meeting speech playing"
kill $PL 2>/dev/null
wait $P
grep "speech recognition ran" out/cpu-check.log
