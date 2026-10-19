#!/usr/bin/env bash
# Builds the 160 ms speech model. The published ONNX export uses 560 ms chunks; this re-exports
# NVIDIA's checkpoint with a right attention context of 1 (2 frames of 80 ms). Words then arrive a
# median 0.16 s sooner with the same transcript (98% identical words on the test meeting).
# Needs about 8 GB of disk for NeMo and runs on the CPU in a few minutes.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=models/nemotron-160ms
[ -s $OUT/encoder.onnx ] && { echo "$OUT already exists"; exit 0; }
cd tools/nemo-export
[ -x .venv/bin/python ] || { uv venv -q --python 3.12 .venv; uv pip install -q --python .venv/bin/python "nemo_toolkit[asr]" onnx onnxruntime soundfile; }
cd ../..
hf download nvidia/nemotron-speech-streaming-en-0.6b --include "*.nemo" --local-dir models/nemotron-nemo
tools/nemo-export/.venv/bin/python tools/nemo-export/export_nemotron_streaming.py \
  models/nemotron-nemo/nemotron-speech-streaming-en-0.6b.nemo $OUT --right-context 1
echo "built $OUT; the app now uses it"
