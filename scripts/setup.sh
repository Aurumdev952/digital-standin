#!/usr/bin/env bash
# One-time setup: system packages, virtual camera, models, decision-model environment, build and clips.
# Safe to re-run: every step skips work that is already done.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n==> %s\n' "$*"; }

step "System packages (needs sudo)"
PKGS=(libpipewire-0.3-dev libspa-0.2-dev libclang-dev v4l2loopback-dkms v4l-utils ffmpeg pkg-config)
dpkg -s libcudnn9-cuda-13 >/dev/null 2>&1 || PKGS+=(libcudnn9-cuda-13)
MISSING=()
for p in "${PKGS[@]}"; do dpkg -s "$p" >/dev/null 2>&1 || MISSING+=("$p"); done
if [ ${#MISSING[@]} -gt 0 ]; then sudo apt-get install -y "${MISSING[@]}"; else echo "all present"; fi

step "Tools"
for t in cargo uv hf nvidia-smi; do command -v "$t" >/dev/null || { echo "missing: $t"; exit 1; }; done
echo "cargo, uv, hf, nvidia-smi found"

step "Virtual camera"
scripts/setup-vcam.sh

step "Models (about 11 GB; needs a Hugging Face token with access to perplexity-ai/pplx-decider-flash-v1.2)"
scripts/fetch-models.sh

step "Decision-model environment"
(cd sidecar/decider && uv sync)

step "Build"
cargo build --release

step "Placeholder clips"
[ -s clips/dev/idle.mp4 ] || scripts/make-dev-clips.sh
[ -s clips/real/idle.mp4 ] || scripts/make-real-dev-clips.sh

step "Check"
target/release/standin doctor || true
echo
echo "Setup done. Start it with: make run"
