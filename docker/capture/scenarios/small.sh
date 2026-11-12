#!/usr/bin/env bash
# Small: four minutes of the small movements of a desk, one kind at a time
# with the desktop left still for three seconds or more between them, for a
# picture that settles to be measured on what moves a little: whether each
# is worth a region's stream, and what its settle costs after. A page in
# Chromium lays every part out at a fixed place and plays it by its
# own clock from the key that starts it; the pointer and the keys come
# through the session's client, as a person's do, so CAPTURE_KEYS must name
# what it reads (lib.sh, `press`), and the desktop must be 1280 by 820 points
# or more, the menu's last item ending 813 down. LENGTH should be 240 or more.
# many.sh plays the same page's parts several at once. Each part's start goes
# to stderr with its second:
#
#     3  text typed into a field, its caret blinking after
#    12  a spinner, 32 points
#    21  a spinner of 16 beside a word whose dots come and go
#    30  a stopwatch, ten times a second
#    39  a clock, once a second
#    50  a progress bar filling, 600 points wide
#    59  a striped bar, all of it moving
#    68  a line of text sliding sideways
#    77  the pointer over a list, each row lit as it passes
#    86  text selected by a drag, then let go
#    96  a small chart redrawn five times a second
#   105  a picture of 96 by 54 points that moves, as a thumbnail's video
#   114  one of 320 by 180, as a small video
#   123  a log a line is added to five times a second, scrolling once full
#   134  a section opened and closed, what is under it pushed down and back
#   144  a menu opened, its items passed over, closed
#   153  a notice sliding in and out
#   165  a slider dragged
#   172  a button passed over, its colour fading in and out
#   179  the page nudged by the wheel a notch at a time, and back
#   189  a terminal opened over the page: a command typed, a progress line
#        rewritten ten times a second, a table redrawn once a second, the
#        window nudged aside four points a step and back, and closed
#   220  the clock, the spinner and typing at once
#   231  still until the end
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
[[ -n "${CAPTURE_KEYS:-}" ]] || { log "small: the pointer comes through CAPTURE_KEYS, which is not set"; exit 1; }
((W >= 1280 && H >= 820)) || { log "small: the page is 1280 by 820 points, and the desktop ${W}x${H}"; exit 1; }
small_page small

part() { log "small $(((($(date +%s%N) - T0)) / 1000000000))s: $*"; }

PX=640 PY=560
pointer $PX $PY
ready
T0=$(date +%s%N)
# The key that starts the page's clock, typed nowhere.
press g

at 3
part "text typed into a field"
click 240 58
type_slowly "the quick brown fox jumps over the lazy"
at 11
click 640 560

at 12; part "a spinner"
at 21; part "a small spinner and a word"
at 30; part "a stopwatch"
at 39; part "a clock"
at 50; part "a progress bar"
at 59; part "a striped bar"
at 68; part "a line of text sliding"

at 77
part "the pointer over a list"
glide 240 190 0.3
glide 240 480 2.5
glide 260 205 2.5
glide 640 560 0.4

at 86
part "text selected by a drag"
glide 484 212 0.3
pointer 484 212 1
glide 840 250 1.2 1
glide 700 400 1.8 1
glide 560 330 1 1
pointer 560 330 0
at 93
click 640 560

at 96; part "a small chart"
at 105; part "a thumbnail's video"
at 114; part "a small video"
at 123; part "a log"
at 134; part "a section opened and closed"

at 144
part "a menu"
for _ in 1 2; do
	click 500 643
	sleep 0.4
	glide 540 670 0.3
	glide 540 790 1.2
	sleep 0.3
	click 500 643
	glide 640 560 0.3
	sleep 0.5
done

at 153; part "a notice"

at 165
part "a slider dragged"
glide 706 644 0.3
pointer 706 644 1
glide 840 644 1.5 1
glide 740 644 1.5 1
pointer 740 644 0
glide 640 560 0.3

at 172
part "a button passed over"
for _ in 1 2 3 4; do
	glide 75 705 0.2
	sleep 0.4
	glide 200 640 0.2
	sleep 0.4
done
glide 640 560 0.3

at 179
part "the page nudged by the wheel"
# Buttons 5 and 4 are the wheel down and up, a press and release a notch.
for mask in 16 8; do
	for _ in $(seq 8); do
		pointer $PX $PY $mask
		pointer $PX $PY 0
		sleep 0.4
	done
done

at 189
part "a terminal over the page"
# What the terminal runs is a short name each, so that typing one is over
# well before it is to start and the one before it has ended: the progress
# line runs six seconds from about 196, the table five from about 205.
foot --app-id=small-term --working-directory="$MATERIAL" >/dev/null 2>&1 &
await_window small-term
swaymsg -q '[app_id="small-term"] floating enable, resize set 640 400, move position 300 160'
at 191
type_slowly $'ls -la /usr/share/fonts | head\n'
at 195
type_slowly $'./progress\n'
at 204
type_slowly $'./table\n'
at 211.5
for step in $(seq 1 10) $(seq 9 -1 0); do
	swaymsg -q "[app_id=\"small-term\"] move position $((300 + step * 4)) 160"
	sleep 0.1
done
at 217
swaymsg -q '[app_id="small-term"] kill'

at 220
part "the clock, the spinner and typing at once"
click 240 58
type_slowly " dog, and types on while a clock ticks and a spinner turns beside it"
at 229
click 640 560

at 231
part "still until the end"
until_end
swaymsg -q '[app_id="^chrom"] kill' || true
