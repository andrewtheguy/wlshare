#!/usr/bin/env bash
# Busy: a full-screen terminal flooded with coloured random lines
# (busy-lines.sh), every frame the whole screen new. The terminal is open
# and empty until the scenario goes.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
foot -- bash -c 'if [[ -n "$1" ]]; then while [[ ! -e "$1" ]]; do sleep 0.05; done; fi; exec "$2"' _ "$GO" "$HERE/busy-lines.sh" >/dev/null 2>&1 &
await_window foot
swaymsg -q '[app_id="foot"] fullscreen enable'
ready
until_end
close foot
