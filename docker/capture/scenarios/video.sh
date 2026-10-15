#!/usr/bin/env bash
# Busy: the video CAPTURE_VIDEO names playing full screen, cropped to fill
# it, from its start when the scenario goes. With PAUSE_BEFORE set,
# the video is paused where it stands that many seconds before the end and
# the desktop left still on that frame of it, so a dial the walk lowered
# gets its settle, and of a picture of the video: stopped, it left the
# desktop black.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
video
ready
unpause
if [[ -n "${PAUSE_BEFORE:-}" ]]; then
	sleep $((LENGTH > PAUSE_BEFORE + 2 ? LENGTH - PAUSE_BEFORE : 2))
	pause
fi
until_end
close mpv
