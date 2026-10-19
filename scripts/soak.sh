#!/usr/bin/env bash
# Long-run check: plays a recording through the muted default sink into a headless stand-in and
# samples its memory, GPU memory and CPU every 30 s. Usage: scripts/soak.sh <audio> [config]
set -euo pipefail
cd "$(dirname "$0")/.."
AUDIO=$1
CONFIG=${2:-}
SECS=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$AUDIO" | cut -d. -f1)
OUT=out/soak
mkdir -p "$OUT"
WAS_MUTED=$(pactl get-sink-mute @DEFAULT_SINK@ | awk '{print $2}')
restore() { [ "$WAS_MUTED" = "no" ] && pactl set-sink-mute @DEFAULT_SINK@ 0 || true; }
trap restore EXIT
pactl set-sink-mute @DEFAULT_SINK@ 1
target/release/standin ${CONFIG:+--config "$CONFIG"} run --headless --seconds $((SECS + 30)) > "$OUT/events.log" 2> "$OUT/stderr.log" &
PID=$!
sleep 12
pw-play "$AUDIO" &
PLAY=$!
echo "t_s,rss_mb,gpu_mb,cpu_pct" > "$OUT/samples.csv"
START=$(date +%s)
while kill -0 $PID 2>/dev/null; do
  T=$(( $(date +%s) - START ))
  RSS=$(ps -o rss= -p $PID 2>/dev/null | awk '{print int($1/1024)}')
  GPU=$(nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader,nounits | awk -F', ' -v p=$PID '$1==p{print $2}')
  CPU=$(ps -o %cpu= -p $PID 2>/dev/null | tr -d ' ')
  echo "$T,${RSS:-0},${GPU:-0},${CPU:-0}" >> "$OUT/samples.csv"
  sleep 30
done
wait $PLAY 2>/dev/null || true
tail -3 "$OUT/events.log"
echo "samples in $OUT/samples.csv"
