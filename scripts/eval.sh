#!/usr/bin/env bash
# Replays every labelled recording through the whole pipeline and collects the scores.
# Starts the decision model when it is not already running, and stops it again afterwards.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=target/release/standin
cargo build --release -q -p standin
mkdir -p out
SOCK="${XDG_RUNTIME_DIR:-/tmp}/digital-standin-decider.sock"
if ! python3 -c "import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1])" "$SOCK" 2>/dev/null; then
  (cd sidecar/decider && uv sync -q)
  sidecar/decider/.venv/bin/python sidecar/decider/server.py --socket "$SOCK" > out/eval-decider.log 2>&1 &
  SIDECAR=$!
  trap 'kill $SIDECAR 2>/dev/null || true' EXIT
  until grep -q "ready on" out/eval-decider.log 2>/dev/null; do sleep 2; kill -0 $SIDECAR || { cat out/eval-decider.log; exit 1; }; done
fi
SUMMARY=out/eval-summary.txt
: > "$SUMMARY"
run() {
  local name=$1; shift
  echo "=== $name" | tee -a "$SUMMARY"
  if ! "$BIN" "$@" 2> out/eval-last.err | tee -a "$SUMMARY"; then
    tail -5 out/eval-last.err
    exit 1
  fi
}
run "laughter mixed into a meeting" replay test-media/meeting-laughs.wav
run "plain meeting" replay test-media/meeting.wav
run "talk show (caption labels)" replay test-media/laughs.wav
run "speech with silence and room noise" replay test-media/gappy.wav
for p in cindy brian sam; do
  run "meeting heard by $p" --config test-media/personas/$p.toml replay test-media/meeting.wav \
    --labels test-media/personas/$p.labels.toml --out out/meeting-$p
done
echo "summary in $SUMMARY"
