#!/usr/bin/env bash
# Downloads the public test recordings and cuts the 16 kHz mono excerpts the labels refer to.
set -euo pipefail
cd "$(dirname "$0")/../test-media"
ytdlp() { uvx --from "yt-dlp[default]@latest" yt-dlp -q --no-progress -f bestaudio "$@"; }
[ -s meeting_full.webm ] || ytdlp -o "meeting_full.%(ext)s" "https://www.youtube.com/watch?v=lBVtvOpU80Q"
[ -s laughs_full.webm ] || ytdlp -o "laughs_full.%(ext)s" "https://www.youtube.com/watch?v=-tQWkiltH8s"
ffmpeg -loglevel error -y -ss 60 -t 300 -i meeting_full.webm -ac 1 -ar 16000 meeting.wav
ffmpeg -loglevel error -y -t 240 -i laughs_full.webm -ac 1 -ar 16000 laughs.wav
echo "test media ready"
