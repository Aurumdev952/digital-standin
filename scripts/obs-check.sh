#!/usr/bin/env bash
# Records the Digital Standin camera with OBS Studio on a private virtual display, using a throwaway
# OBS profile (your own OBS settings are not touched), and reports what OBS captured.
# Needs the stand-in running (make run or make play), obs-studio and xvfb. Usage: scripts/obs-check.sh [seconds]
set -euo pipefail
cd "$(dirname "$0")/.."
SECS=${1:-15}
O=$(mktemp -d)
trap 'kill -9 $(jobs -p) 2>/dev/null || true; rm -rf "$O"' EXIT
mkdir -p "$O/cfg/obs-studio/basic/profiles/Check" "$O/cfg/obs-studio/basic/scenes" "$O/rec" out
printf '[General]\nFirstRun=true\n[Basic]\nProfile=Check\nProfileDir=Check\nSceneCollection=Check\nSceneCollectionFile=Check\n' > "$O/cfg/obs-studio/global.ini"
printf '[General]\nName=Check\n[Output]\nMode=Simple\n[SimpleOutput]\nFilePath=%s\nRecFormat2=mkv\nRecQuality=Small\nRecEncoder=x264\n[Video]\nBaseCX=1280\nBaseCY=720\nOutputCX=1280\nOutputCY=720\nFPSType=0\nFPSCommon=30\n' "$O/rec" > "$O/cfg/obs-studio/basic/profiles/Check/basic.ini"
cat > "$O/cfg/obs-studio/basic/scenes/Check.json" <<'EOF'
{"current_scene":"Scene","current_program_scene":"Scene","scene_order":[{"name":"Scene"}],"name":"Check","groups":[],"transitions":[],
 "sources":[
  {"id":"v4l2_input","versioned_id":"v4l2_input","name":"Standin","uuid":"7b0f2a6e-1111-4c3e-9a1e-000000000001","enabled":true,"settings":{"device_id":"/dev/video10","input":0}},
  {"id":"scene","versioned_id":"scene","name":"Scene","uuid":"7b0f2a6e-1111-4c3e-9a1e-000000000002","enabled":true,"settings":{"id_counter":1,"items":[{"name":"Standin","source_uuid":"7b0f2a6e-1111-4c3e-9a1e-000000000001","visible":true,"pos":{"x":0.0,"y":0.0},"scale":{"x":1.0,"y":1.0},"align":5,"bounds_type":0,"id":1}]}}
 ]}
EOF
DISPLAY_NUM=:$((90 + RANDOM % 9))
Xvfb $DISPLAY_NUM -screen 0 1600x1000x24 >/dev/null 2>&1 &
sleep 1
(cd "$O" && DISPLAY=$DISPLAY_NUM XDG_CONFIG_HOME="$O/cfg" LIBGL_ALWAYS_SOFTWARE=1 obs --startrecording --disable-shutdown-check \
  --collection Check --profile Check --scene Scene > "$O/obs.log" 2>&1) &
sleep $((SECS + 8))
grep -E "v4l2-input: (Start capture|Resolution|Pixelformat|Framerate)" "$O/obs.log" || { tail -20 "$O/obs.log"; exit 1; }
# OBS buffers the file under load; wait until what it has written parses.
for _ in $(seq 1 30); do
  F=$(ls "$O"/rec/*.mkv 2>/dev/null | head -1)
  STATS=$([ -n "$F" ] && ffprobe -v quiet -count_frames -select_streams v -show_entries stream=width,height,nb_read_frames -of csv=p=0 "$F" || true)
  [ -n "$STATS" ] && [ "$(echo "$STATS" | cut -d, -f3)" != "N/A" ] && break
  sleep 1
done
[ -n "$STATS" ] || { echo "OBS wrote no readable recording"; exit 1; }
echo "$STATS" | awk -F, '{printf "OBS recorded %s frames at %sx%s\n", $3, $1, $2}'
ffmpeg -loglevel quiet -y -i "$F" -vf "fps=1,scale=320:-1,tile=4x2" -frames:v 1 out/obs-check.png
echo "snapshot: out/obs-check.png"
