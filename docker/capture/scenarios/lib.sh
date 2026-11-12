#!/usr/bin/env bash
# What the scenario scripts share: the desktop's size, the material they
# show, and the few actions they are made of. Sourced, not run.
#
# A scenario script opens and lays out its windows, says it is `ready`, then
# plays one kind of content on the sway desktop it is run in, for LENGTH
# seconds (its one argument, 20 by default) from then, and closes what it
# opened. Run one by hand on any sway desktop a daemon is capturing —
# `wlshare --capture-frames DIR` — or let docker/capture/run.sh run them all.
# They need swaymsg, wlrctl (the pointer), foot, chromium, mpv and socat, for the
# keys either CAPTURE_KEYS or wtype (`press`, below), and for video.sh a
# video file in CAPTURE_VIDEO.
#
#   quiet: terminal.sh, still.sh, uncover.sh    little changes a frame, most of it still
#   busy:  flood.sh, browser.sh, drag.sh, video.sh, window.sh    most of the picture new, or the window's
#   mixed: mixed.sh    minutes at the desk, quiet and busy in turn
#   small: small.sh    the small movements of a desk, one kind at a time
#   many:  many.sh     the same movements, several at once and then all of them
#
# busy-lines.sh is the flood itself, for any terminal.

set -euo pipefail

LENGTH="${1:-20}"
# Where the page is made, once.
MATERIAL="${CAPTURE_MATERIAL:-${XDG_RUNTIME_DIR:-/tmp}/capture-material}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

log() { printf '%s %s\n' "$(date +%T)" "$*" >&2; }

# The desktop's size as sway lays it out, from the output's rectangle: the
# units windows are placed and the pointer moved in, which at scale 2 are
# half the mode's pixels.
desktop_size() {
	local rect
	rect=$(swaymsg -t get_outputs | tr -d ' \n' | sed -n 's/.*"rect":{"x":-\{0,1\}[0-9]*,"y":-\{0,1\}[0-9]*,"width":\([0-9]*\),"height":\([0-9]*\).*/\1 \2/p')
	W=${rect% *}
	H=${rect#* }
	[[ -n "$W" && -n "$H" ]] || { log "no output size from swaymsg"; exit 1; }
	# The scale as sway says it, its fraction kept: 1.5 is 1.5.
	SCALE=$(swaymsg -t get_outputs | tr -d ' \n' | sed -n 's/.*"scale":\([0-9][0-9.]*\).*/\1/p')
	SCALE=${SCALE:-1}
}

# The page, made where MATERIAL says if it is not there.
material() {
	mkdir -p "$MATERIAL"
	if [[ ! -s "$MATERIAL/page.html" ]]; then
		# A long page: headings, paragraphs, a table and coloured boxes.
		{
			echo '<!doctype html><html><head><meta charset="utf-8"><title>page</title><style>body{font-family:"DejaVu Serif",serif;font-size:17px;line-height:1.5;max-width:70%;margin:40px auto;color:#222;background:#fff}h2{font-family:"DejaVu Sans",sans-serif}table{border-collapse:collapse;margin:1em 0}td,th{border:1px solid #999;padding:4px 10px}.box{display:inline-block;width:120px;height:60px;margin:4px}</style></head><body>'
			local i r c
			for i in $(seq 1 60); do
				echo "<h2>Section $i: the quick brown fox jumps over the lazy dog</h2>"
				for _ in 1 2 3; do
					echo "<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum. <code>fn main() { println!(\"$i\"); }</code></p>"
				done
				echo "<table><tr><th>name</th><th>bytes</th><th>ms</th><th>dB</th></tr>"
				for r in 1 2 3 4; do echo "<tr><td>sample-$i-$r</td><td>$((i * 7919 + r * 131))</td><td>$((i % 23)).$r</td><td>4$r.$i</td></tr>"; done
				echo "</table>"
				for c in 1 2 3 4 5 6; do echo "<div class=box style=\"background:hsl($(((i * 37 + c * 61) % 360)),70%,55%)\"></div>"; done
			done
			echo '</body></html>'
		} >"$MATERIAL/page.html"
	fi
	if [[ ! -s "$MATERIAL/second.html" ]]; then
		# The page a link leads to: another article, its photographs drawn
		# as gradients, for mixed.sh.
		{
			echo '<!doctype html><html><head><meta charset="utf-8"><title>second</title><style>body{font-family:"DejaVu Sans",sans-serif;font-size:16px;line-height:1.6;max-width:60%;margin:30px auto;color:#1a1a1a;background:#fbfaf7}h1,h2{font-family:"DejaVu Serif",serif}.photo{height:320px;margin:1em 0;border-radius:6px}</style></head><body><h1>The second article</h1>'
			local i
			for i in $(seq 1 40); do
				echo "<h2>Part $i</h2>"
				echo "<p>Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo inventore veritatis et quasi architecto beatae vitae dicta sunt explicabo. Nemo enim ipsam voluptatem quia voluptas sit aspernatur aut odit aut fugit. Part $i of 40.</p>"
				echo "<div class=photo style=\"background:radial-gradient(circle at $(((i * 29) % 100))% $(((i * 53) % 100))%,hsl($(((i * 47) % 360)),80%,70%),hsl($(((i * 47 + 140) % 360)),60%,25%))\"></div>"
				echo "<p>Neque porro quisquam est, qui dolorem ipsum quia dolor sit amet, consectetur, adipisci velit, sed quia non numquam eius modi tempora incidunt ut labore et dolore magnam aliquam quaerat voluptatem.</p>"
			done
			echo '</body></html>'
		} >"$MATERIAL/second.html"
	fi
}

# Press and let go of one key: a character, or `-k` and Return or Page_Down.
# Through the session's client where CAPTURE_KEYS names what it reads keys
# from (`video-sink --keys`: a keysym in hexadecimal a line), so that they come
# as a person's do, on the daemon's keyboard; by wtype otherwise. Not by both
# on one desktop: wtype's keyboard is gone when it exits, and with the
# daemon's beside it, which has pressed nothing, sway is left with a seat
# that has a keyboard and no keymap, which Chromium does not survive starting
# on.
press() {
	if [[ -z "${CAPTURE_KEYS:-}" ]]; then
		if [[ "$1" == -k ]]; then wtype -k "$2"; else wtype -- "$1"; fi
		return
	fi
	local keysym
	case "$1 ${2:-}" in
		"-k Return") keysym=ff0d ;;
		"-k Page_Down") keysym=ff56 ;;
		-k*) log "press: no keysym for $2"; return 1 ;;
		*) keysym=$(printf '%x' "'$1") ;;
	esac
	echo "$keysym" >"$CAPTURE_KEYS"
}

# The pointer at X,Y in the desktop's points with BUTTONS held, RFB's mask
# (1 the left button, 8 and 16 the wheel up and down), through the session's
# client as `press`'s keys are (`video-sink --keys`: `@X,Y,BUTTONS`, in
# pixels), so CAPTURE_KEYS must name what it reads. The pixel is the point
# times the scale, which may have a fraction, rounded to the nearest.
pointer() {
	awk -v x="$1" -v y="$2" -v scale="$SCALE" -v buttons="${3:-0}" \
		'BEGIN { printf "@%d,%d,%d\n", x * scale + 0.5, y * scale + 0.5, buttons }' >"$CAPTURE_KEYS"
}

# Type text a character at a time, as a person does.
type_slowly() {
	local text=$1 i c
	for ((i = 0; i < ${#text}; i++)); do
		c=${text:i:1}
		if [[ "$c" == $'\n' ]]; then press -k Return; else press "$c"; fi
		sleep 0.05
	done
}

# Wait for a window whose app_id starts with the argument.
await_window() {
	local _
	for _ in $(seq 50); do
		swaymsg -t get_tree | grep -q "\"app_id\": \"$1" && return 0
		sleep 0.2
	done
	log "no window $1 appeared"
	return 1
}

# A script's own clock, for one that plays its parts by the second: sleep
# until that many seconds, which may have a fraction, from T0, the nanosecond
# the script set as it went.
at() {
	local due now
	due=$((T0 + $(awk -v s="$1" 'BEGIN { printf "%d", s * 1000000000 }')))
	now=$(date +%s%N)
	if ((due > now)); then
		sleep "$(awk -v n=$((due - now)) 'BEGIN { printf "%.3f", n / 1000000000 }')"
	fi
}

# The pointer from where it is, PX,PY, which a script sets where it first
# puts it, to X,Y in steps of about a thirtieth of a second over SECONDS, its
# buttons held as given; and a click of the left button at X,Y.
PX=0 PY=0
glide() {
	local x=$1 y=$2 seconds=$3 buttons=${4:-0} steps i
	steps=$(awk -v s="$seconds" 'BEGIN { n = int(s * 30); print n < 1 ? 1 : n }')
	for ((i = 1; i <= steps; i++)); do
		pointer $((PX + (x - PX) * i / steps)) $((PY + (y - PY) * i / steps)) "$buttons"
		sleep 0.033
	done
	PX=$x PY=$y
}
click() {
	pointer "$1" "$2" 0
	pointer "$1" "$2" 1
	sleep 0.05
	pointer "$1" "$2" 0
	PX=$1 PY=$2
}

# The page of the small movements (small.html), which lays every part out at
# a fixed place and plays them by its own clock from the first key not typed
# into its field, by PLAN: `small`, one kind at a time, or `many`, several at
# once. As an application's window, which has no tab strip or address bar
# and, tiled, fills the desktop: its points are the desktop's. Not a kiosk's,
# which is fullscreen, and sway draws a fullscreen window over a floating
# one, the terminal's. `--test-type` keeps the bar that warns of
# `--no-sandbox` off the top of it, which a kiosk does not draw. And in
# MATERIAL what the terminal over it runs: `progress`, a line rewritten ten
# times a second for six, and `table`, redrawn once a second for five.
small_page() {
	mkdir -p "$MATERIAL"
	cat >"$MATERIAL/progress" <<'END'
#!/usr/bin/env bash
for i in $(seq 60); do
	printf '\rcopying %3d%% ' $((i * 100 / 60))
	sleep 0.1
done
echo
END
	cat >"$MATERIAL/table" <<'END'
#!/usr/bin/env bash
for _ in 1 2 3 4 5; do
	clear
	ps aux --sort=-%cpu | head -12 | cut -c1-76
	sleep 1
done
END
	chmod +x "$MATERIAL/progress" "$MATERIAL/table"
	chromium --ozone-platform=wayland --no-sandbox --disable-gpu --disable-dev-shm-usage --no-first-run --password-store=basic --test-type --app="file://$HERE/small.html?$1" >>"$MATERIAL/chromium.log" 2>&1 &
	await_window chrom
	sleep 4
}

# The page in a kiosk Chromium, software rendered. Its first start in a
# fresh profile sometimes dies at once, so it is started again when no
# window comes; what it says goes to chromium.log beside the material.
browser() {
	local _
	for _ in 1 2 3; do
		chromium --ozone-platform=wayland --no-sandbox --disable-gpu --disable-dev-shm-usage --no-first-run --password-store=basic --kiosk "file://$MATERIAL/page.html" >>"$MATERIAL/chromium.log" 2>&1 &
		if await_window chromium; then
			sleep 4
			return 0
		fi
	done
	return 1
}

# The video CAPTURE_VIDEO names, full screen and cropped to fill it, so that
# every pixel of the desktop is the video's, software rendered. Paused at
# its first frame until `unpause`, so that it plays from its start once the
# scenario goes; `pause` holds it on the frame it is at.
video() {
	[[ -s "${CAPTURE_VIDEO:-}" ]] || { log "no video: CAPTURE_VIDEO is '${CAPTURE_VIDEO:-}'"; return 1; }
	mkdir -p "$MATERIAL"
	mpv --vo=wlshm --hwdec=no --no-audio --loop=inf --no-border --no-osc --osd-level=0 --panscan=1.0 --pause --input-ipc-server="$MATERIAL/mpv.sock" "$CAPTURE_VIDEO" >/dev/null 2>&1 &
	await_window mpv
	swaymsg -q '[app_id="mpv"] fullscreen enable'
}

unpause() {
	echo '{ "command": ["set_property", "pause", false] }' | socat - "UNIX-CONNECT:$MATERIAL/mpv.sock" >/dev/null
}

pause() {
	echo '{ "command": ["set_property", "pause", true] }' | socat - "UNIX-CONNECT:$MATERIAL/mpv.sock" >/dev/null
}

# Close the windows a scenario opened: the app ids it names.
close() {
	local app
	for app in "$@"; do
		swaymsg -q "[app_id=\"$app\"] kill" 2>/dev/null || true
	done
	pkill -f busy-lines.sh 2>/dev/null || true
}

# The scenario's clock: sleep until its LENGTH is up, from its start.
STARTED=$(date +%s)

# The windows are open: once they are drawn, say so where CAPTURE_READY
# names, and wait there for the word to go, which docker/capture/run.sh
# gives once the session that captures the scenario has its first frame, so
# that the capture begins with the scenario's windows and not with their
# opening. Without CAPTURE_READY it goes at once. The scenario's clock starts
# here.
GO="${CAPTURE_READY:+$CAPTURE_READY.go}"
ready() {
	sleep 1
	if [[ -n "$GO" ]]; then
		touch "$CAPTURE_READY.ready"
		while [[ ! -e "$GO" ]]; do sleep 0.05; done
	fi
	STARTED=$(date +%s)
}

until_end() {
	local now
	now=$(date +%s)
	if ((STARTED + LENGTH > now)); then
		sleep $((STARTED + LENGTH - now))
	fi
}

desktop_size
