#!/usr/bin/env bash
# Busy, in part: the video CAPTURE_VIDEO names playing in a floating window
# over the page, from its start when the scenario goes, the rest of the
# desktop still. The window is 16:9 and 55% of the desktop's width,
# centred, so it fits at every size; mpv letterboxes nothing, the window
# having the clip's shape.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
material
browser
# The kiosk page is fullscreen, and sway draws a fullscreen window over a
# floating one: tiled, it fills the desktop and the video is over it.
swaymsg -q '[app_id="chromium"] fullscreen disable'
mkdir -p "$MATERIAL"
mpv --vo=wlshm --hwdec=no --no-audio --loop=inf --no-border --no-osc --osd-level=0 --pause --input-ipc-server="$MATERIAL/mpv.sock" "$CAPTURE_VIDEO" >/dev/null 2>&1 &
await_window mpv
# 55% of the width, or less where the desktop is too short for it at 16:9.
VW=$((W * 55 / 100))
((VW * 9 / 16 <= H)) || VW=$((H * 16 / 9))
VH=$((VW * 9 / 16))
swaymsg -q "[app_id=\"mpv\"] floating enable, resize set $VW $VH, move position $(((W - VW) / 2)) $(((H - VH) / 2))"
ready
unpause
until_end
close mpv chromium
