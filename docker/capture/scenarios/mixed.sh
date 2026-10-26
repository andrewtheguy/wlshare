#!/usr/bin/env bash
# Mixed: a few minutes at the desk, scripted, the quiet and the busy in
# turn as a person's are, for a coder to be measured over a whole session
# rather than one kind of content: a terminal listing folders; a page in a
# Chromium window read with quick flicks of the wheel and pauses; a second
# page opened, read and closed; the browser hidden and shown again; a
# terminal over the page printing a file, scrolled back through and typed
# into; that terminal dragged aside and back, hidden and shown; the focus
# moved between the windows; the video CAPTURE_VIDEO names in a window, then
# full screen, then a window again; the page once more. About 300 seconds:
# LENGTH should be 300 or more, and the rest of it is the desktop left still.
# Each part's start goes to stderr with its second.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
[[ -s "${CAPTURE_VIDEO:-}" ]] || { log "no video: CAPTURE_VIDEO is '${CAPTURE_VIDEO:-}'"; exit 1; }
material

part() { log "mixed $(($(date +%s) - STARTED))s: $*"; }

chrome() {
	chromium --ozone-platform=wayland --no-sandbox --disable-gpu --disable-dev-shm-usage --no-first-run --password-store=basic "$@" >>"$MATERIAL/chromium.log" 2>&1 &
}

# The pointer to X,Y: wlrctl moves it by an amount, so first into the
# top-left corner, where the desktop stops it.
point() {
	wlrctl pointer move -100000 -100000
	wlrctl pointer move "$1" "$2"
}

# A flick: a few fast notches of the wheel where the pointer is.
flick() {
	local _
	for _ in $(seq 6); do
		wlrctl pointer scroll "${1:-15}" 0
		sleep 0.03
	done
}

# Read a page: three quick flicks then a pause to read, N times.
read_page() {
	local _
	for _ in $(seq "$1"); do
		flick; sleep 0.4; flick; sleep 0.4; flick
		sleep 5
	done
}

# Wait for a window whose title starts with the argument.
await_title() {
	local _
	for _ in $(seq 40); do
		swaymsg -t get_tree | grep -q "\"name\": \"$1" && return 0
		sleep 0.2
	done
	log "no window titled $1 appeared"
	return 1
}

TW=$((W < 1000 ? W : 1000)) TH=$((H < 640 ? H : 640))
TX=$(((W - TW) / 2)) TY=$(((H - TH) / 2))
terminal() {
	foot --app-id="$1" >/dev/null 2>&1 &
	await_window "$1"
	swaymsg -q "[app_id=\"$1\"] floating enable, resize set $TW $TH, move position $TX $TY"
	sleep 0.5
}

# The page in a window filling the desktop, tab strip and address bar drawn.
chrome --new-window "file://$MATERIAL/page.html"
await_window chromium
sleep 4
point $((W / 2)) $((H / 2))
ready

part "folders listed in a terminal"
terminal files
for dir in ~ /usr/share /usr/share/doc /usr/share/fonts/truetype /etc; do
	type_slowly "ls -la $dir | head -40"$'\n'
	sleep 2.5
done
type_slowly $'exit\n'
sleep 2

part "the page read with flicks"
point $((W / 2)) $((H / 2))
read_page 4

part "a second page opened, read and closed"
# Full screen over the page, as a link followed fills the window it is in:
# tiled, sway would put it beside the page. Closed, the page is back.
# At 1920x1080 a first try once opened nothing: tried again, up to three.
for try in 1 2 3; do
	chrome --new-window "file://$MATERIAL/second.html"
	await_title second && break
	((try < 3)) || exit 1
done
swaymsg -q '[title="^second"] fullscreen enable'
# The pointer enters a window that opened under it only when it moves:
# still, the wheel would go nowhere.
point $((W / 2)) $((H / 2))
sleep 3
read_page 2
flick -15
sleep 3
swaymsg -q '[title="^second"] kill'
sleep 2

part "the browser hidden and shown"
swaymsg -q '[app_id="chromium"] move scratchpad'
sleep 3
swaymsg -q '[app_id="chromium"] scratchpad show, floating disable'
sleep 2

part "a file printed in a terminal over the page, read back and typed into"
terminal work
type_slowly "cat $MATERIAL/page.html | fold -w 110"$'\n'
sleep 1.5
point $((TX + TW / 2)) $((TY + TH / 2))
for _ in 1 2 3 4; do
	for _ in $(seq 8); do wlrctl pointer scroll -15 0; sleep 0.04; done
	sleep 2.5
done
type_slowly $'clear\n'
type_slowly $'cat > /tmp/fib.py <<\'END\'\ndef fib(n):\n    a, b = 0, 1\n    for _ in range(n):\n        a, b = b, a + b\n    return a\n\nprint([fib(i) for i in range(20)])\nEND\n'
sleep 1
type_slowly $'wc -l /tmp/fib.py && head /tmp/fib.py\n'
sleep 2

part "the terminal dragged aside and back"
for step in $(seq 1 30); do
	swaymsg -q "[app_id=\"work\"] move position $((TX + step * 12)) $TY"
	sleep 0.033
done
sleep 1.5
for step in $(seq 29 -1 0); do
	swaymsg -q "[app_id=\"work\"] move position $((TX + step * 12)) $TY"
	sleep 0.033
done
sleep 1.5

part "the terminal hidden and shown"
swaymsg -q '[app_id="work"] move scratchpad'
sleep 2.5
swaymsg -q '[app_id="work"] scratchpad show'
swaymsg -q "[app_id=\"work\"] resize set $TW $TH, move position $TX $TY"
sleep 2

part "the focus moved between the windows"
for app in chromium work chromium work; do
	swaymsg -q "[app_id=\"$app\"] focus"
	sleep 1.5
done
swaymsg -q '[app_id="work"] move scratchpad'
sleep 1.5

part "the video in a window"
mpv --vo=wlshm --hwdec=no --no-audio --loop=inf --no-osc --osd-level=0 --pause --input-ipc-server="$MATERIAL/mpv.sock" "$CAPTURE_VIDEO" >/dev/null 2>&1 &
await_window mpv
VW=$((W < 1040 ? W : 1040)) VH=$((H < 660 ? H : 660))
swaymsg -q "[app_id=\"mpv\"] floating enable, resize set $VW $VH, move position $(((W - VW) / 2)) $(((H - VH) / 2))"
sleep 1
unpause
sleep 40

part "the video full screen"
swaymsg -q '[app_id="mpv"] fullscreen enable'
sleep 40

part "the video in a window again"
swaymsg -q '[app_id="mpv"] fullscreen disable'
swaymsg -q "[app_id=\"mpv\"] resize set $VW $VH, move position $(((W - VW) / 2)) $(((H - VH) / 2))"
sleep 10
close mpv
sleep 2

part "the page again, and the terminal closed"
swaymsg -q '[app_id="chromium"] focus'
point $((W / 2)) $((H / 2))
read_page 1
swaymsg -q '[app_id="work"] scratchpad show'
sleep 1
swaymsg -q '[app_id="work"] kill'
sleep 3

part "still until the end"
until_end
close chromium
