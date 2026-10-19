#!/usr/bin/env bash
# Starts the stand-in after checking the camera and the GPU. Extra arguments go to `standin run`.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -x target/release/standin ] || cargo build --release
[ -e /dev/video10 ] || scripts/setup-vcam.sh
FREE=$(nvidia-smi --query-gpu=memory.free --format=csv,noheader,nounits | head -1)
if [ "$FREE" -lt 12000 ]; then
  echo "Only ${FREE} MiB of GPU memory is free; about 12 GB is needed. Stop vLLM or other GPU jobs first:"
  nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader
  exit 1
fi
exec target/release/standin run "$@"
