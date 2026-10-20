#!/usr/bin/env bash
# Stops the stand-in and its sidecars, freeing the GPU.
for pid in $(ps -eo pid,comm | awk '$2=="standin"{print $1}'); do kill "$pid"; done
for pid in $(ps -eo pid,args | awk '/sidecar\/(decider|diarizer)\/server.py/ && !/awk/ {print $1}'); do kill "$pid"; done
sleep 2
rm -f "${XDG_RUNTIME_DIR:-/tmp}"/digital-standin-{decider,diarizer}.sock
echo "stopped; GPU memory in use: $(nvidia-smi --query-gpu=memory.used --format=csv,noheader)"
