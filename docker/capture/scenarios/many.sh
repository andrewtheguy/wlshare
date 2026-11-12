#!/usr/bin/env bash
# Many: two minutes of the small movements of a desk several at once, where
# small.sh plays them one at a time: three that are apart, four beside each
# other, four where what is cheap as stills is beside what is dear, then all
# eleven for a minute, with a small video, a notice, typing, the pointer and
# a terminal each beside them in turn. For a picture that settles to be
# measured on a desk where much moves a little: what is made a region's
# stream when the movements are neighbours, how many streams, and what stays
# lossless around them. The page is small.sh's (lib.sh, `small_page`), played
# by its other plan, and needs the same of the desktop: CAPTURE_KEYS, and
# 1280 by 820 points or more. LENGTH should be 120 or more. Each part's start
# goes to stderr with its second:
#
#     3  three apart: a spinner, a small spinner beside a word, a clock
#    18  four along the top: a stopwatch, a progress bar filling again and
#        again, a striped bar, a line of text sliding
#    33  four lower down: a chart, a thumbnail's video, a log, the clock
#    48  all eleven at once, from here to 114
#    63  and a small video of 320 by 180, a notice in and out, a section
#        opened and closed
#    81  and text typed into the field
#    88  and the pointer over the list, then a drag over the text
#    99  and a terminal over the page: a progress line rewritten ten times a
#        second, then a table redrawn once a second
#   115  still until the end
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
[[ -n "${CAPTURE_KEYS:-}" ]] || { log "many: the pointer comes through CAPTURE_KEYS, which is not set"; exit 1; }
((W >= 1280 && H >= 820)) || { log "many: the page is 1280 by 820 points, and the desktop ${W}x${H}"; exit 1; }
small_page many

part() { log "many $(((($(date +%s%N) - T0)) / 1000000000))s: $*"; }

PX=640 PY=560
pointer $PX $PY
ready
T0=$(date +%s%N)
# The key that starts the page's clock, typed nowhere.
press g

at 3; part "three apart"
at 18; part "four along the top"
at 33; part "four lower down"
at 48; part "all eleven at once"
at 63; part "and a small video, a notice and a section"

at 81
part "and text typed into the field"
click 240 58
type_slowly "the quick brown fox jumps over the lazy dog while the rest moves"
at 87
click 640 560

at 88
part "and the pointer over the list, then a drag over the text"
glide 240 190 0.3
glide 240 480 2
glide 484 212 0.4
pointer 484 212 1
glide 840 250 1.2 1
glide 700 400 1.6 1
pointer 700 400 0
at 96
click 640 560

at 99
part "and a terminal over the page"
foot --app-id=small-term --working-directory="$MATERIAL" >/dev/null 2>&1 &
await_window small-term
swaymsg -q '[app_id="small-term"] floating enable, resize set 640 400, move position 300 160'
at 101
type_slowly $'./progress\n'
at 108
type_slowly $'./table\n'
at 114.5
swaymsg -q '[app_id="small-term"] kill'

at 115
part "still until the end"
until_end
swaymsg -q '[app_id="^chrom"] kill' || true
