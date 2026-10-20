#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p models

hf download altunenes/parakeet-rs --include 'nemotron-speech-streaming-en-0.6b/*' --local-dir models/parakeet
hf download perplexity-ai/pplx-decider-flash-v1.2 --local-dir models/pplx-decider-flash-v1.2
# The diarizer sidecar loads this from the Hugging Face cache.
hf download nvidia/Nemotron-3-Diarization

fetch() { [ -s "$2" ] || curl -fL --retry 3 -o "$2" "$1"; }
# CED-small runs the laughter reflex; CED-mini is only the reference for the parity test.
for m in ced-small ced-mini; do
  if [ ! -d models/$m ]; then
    fetch https://github.com/k2-fsa/sherpa-onnx/releases/download/audio-tagging-models/sherpa-onnx-$m-audio-tagging-2024-04-19.tar.bz2 models/$m.tar.bz2
    tar -xjf models/$m.tar.bz2 -C models && mv models/sherpa-onnx-$m-audio-tagging-2024-04-19 models/$m && rm models/$m.tar.bz2
  fi
done
echo "models ready"
