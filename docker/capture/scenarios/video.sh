#!/usr/bin/env bash
# Busy: the video CAPTURE_VIDEO names playing full screen, cropped to fill
# it, from its start when the scenario goes. With STOP_BEFORE set,
# the video is stopped that many seconds before the end and the desktop
# left still, so a dial the walk lowered gets its settle.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
video
ready
unpause
if [[ -n "${STOP_BEFORE:-}" ]]; then
	sleep $((LENGTH > STOP_BEFORE + 2 ? LENGTH - STOP_BEFORE : 2))
	close mpv
fi
until_end
close mpv
