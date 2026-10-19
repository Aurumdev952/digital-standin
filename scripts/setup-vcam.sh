#!/usr/bin/env bash
# Loads v4l2loopback as /dev/video10 "Digital Standin". Pass --persist to load it on every boot.
set -euo pipefail
OPTS='video_nr=10 card_label="Digital Standin" exclusive_caps=1'
if ! [ -e /dev/video10 ]; then
  sudo modprobe v4l2loopback video_nr=10 card_label="Digital Standin" exclusive_caps=1
fi
if [ "${1:-}" = "--persist" ]; then
  echo v4l2loopback | sudo tee /etc/modules-load.d/digital-standin.conf >/dev/null
  echo "options v4l2loopback $OPTS" | sudo tee /etc/modprobe.d/digital-standin.conf >/dev/null
fi
v4l2-ctl -d /dev/video10 --info | grep 'Card type'
