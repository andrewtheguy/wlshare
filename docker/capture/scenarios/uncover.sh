#!/usr/bin/env bash
# Quiet: a page with a terminal over it; the terminal is moved aside, then
# closed, and what it covered is uncovered. Still otherwise.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
material
browser
# The kiosk page is fullscreen, and sway draws a fullscreen window over a
# floating one: tiled, it fills the desktop as before and the terminal is
# over it.
swaymsg -q '[app_id="chromium"] fullscreen disable'
foot -- bash -c 'seq 1 2000 | column -c 100; sleep 1000' >/dev/null 2>&1 &
await_window foot
swaymsg -q '[app_id="foot"] floating enable, resize set 900 700, move position 100 100'
ready
sleep 2
for x in $(seq 100 20 1000); do
	swaymsg -q "[app_id=\"foot\"] move position $x 100"
	sleep 0.1
done
sleep 2
swaymsg -q '[app_id="foot"] kill'
until_end
close chromium
