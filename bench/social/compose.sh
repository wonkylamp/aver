#!/bin/bash
# compose.sh <name> <caption> <left title> <right title>
# Joins /tmp/aver-social/<name>/{left,right}.gif side by side (the shorter one
# held on its last frame), lays the caption overlay on top, and writes
# assets/social/<name>.gif and .mp4.
set -euo pipefail
export PATH=/opt/homebrew/bin:$PATH
here=$(cd "$(dirname "$0")" && pwd)
name=$1 caption=$2 ltitle=$3 rtitle=$4
W=/tmp/aver-social/$name
out=$here/../../assets/social
mkdir -p "$out"

dur() { ffprobe -v error -show_entries format=duration -of csv=p=0 "$1"; }
l=$(dur "$W/left.gif") r=$(dur "$W/right.gif")
# Social clips stay at 15 s or under; past that only held frames are cut.
max=$(python3 -c "print(min(15, max($l, $r)))")
padl=$(python3 -c "print(round(max(0, $max - $l) + 0.05, 2))")
padr=$(python3 -c "print(round(max(0, $max - $r) + 0.05, 2))")

/opt/homebrew/bin/python3 "$here/header.py" "$W/overlay.png" "$caption" "$ltitle" "$rtitle"

graph="[0:v]tpad=stop_mode=clone:stop_duration=$padl,setpts=PTS-STARTPTS,scale=600:575[l];\
[1:v]tpad=stop_mode=clone:stop_duration=$padr,setpts=PTS-STARTPTS,scale=600:575[r];\
[l][r]hstack=inputs=2,trim=duration=$max,format=rgb24,pad=1200:675:0:100:color=0x16181d[s];\
[s][2:v]overlay=0:0,fps=15"

# A lossless intermediate, so the GIF never inherits the MP4's chroma blur.
# yuv420p needs an even height: the MP4 drops the bottom row (terminal padding).
ffmpeg -v error -y -i "$W/left.gif" -i "$W/right.gif" -i "$W/overlay.png" \
  -filter_complex "$graph" -c:v ffv1 -pix_fmt bgr0 "$W/joined.mkv"
ffmpeg -v error -y -i "$W/joined.mkv" -vf "crop=1200:674:0:0,format=yuv420p" -c:v libx264 -crf 18 \
  -preset slow -tune animation -movflags +faststart "$out/$name.mp4"
ffmpeg -v error -y -i "$W/joined.mkv" \
  -vf "palettegen=max_colors=256:stats_mode=full" "$W/palette.png"
ffmpeg -v error -y -i "$W/joined.mkv" -i "$W/palette.png" \
  -lavfi "[0:v][1:v]paletteuse=dither=none:diff_mode=rectangle" "$out/$name.gif"

for f in "$out/$name.gif" "$out/$name.mp4"; do
  printf '%s %s bytes, %.1f s\n' "$f" "$(stat -f %z "$f")" "$(dur "$f")"
done
