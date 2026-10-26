#!/usr/bin/env bash
# The capture container's run: a headless sway, the daemon started with
# `--capture-frames`, and vp9-sink connected to it once per scenario and size,
# so each capture in /captures is one session's worth of what the VP9 encoder
# was handed — the frames of one kind of content at one size — named
# `<scenario>-<WxH>[@2].wlcap.zst`. Settings come from the environment:
# SIZES (WxH, with `@2` for a desktop drawn at scale 2), SCENARIOS (names,
# or the groups `quiet`, `busy` and `all`), QUALITY (the dial, 90 by
# default), LENGTH (each scenario's seconds, 20 by default), FRAMES (the
# least frames the flood is played for, 150 by default) and CAPTURE_VIDEO
# (the video file the video and the walk play, which they need).
#
# The content is the scripts in scenarios/, each of which can be run by
# hand on any sway desktop a daemon is capturing; what this adds is the
# session around each — its windows opened and drawn before it, so that a
# capture begins with them and not with the desktop bare, and the sink's
# flags, which make the first one resize into the size, the video ask a
# keyframe midway, and the walk read late so the dial moves — and the file
# each session leaves.
#
#   quiet  terminal  a resize into the size, then a terminal typed into and scrolled
#          still     a page left alone
#          uncover   a window over a page moved aside, then closed
#   busy   flood     a terminal flooded with coloured random lines, every line new
#          browser   a long page scrolled by wheel and by key
#          drag      a floating terminal dragged across the desktop
#          video     a video playing full screen, with a keyframe asked midway
#          window    the video playing in a window over a page, the rest still,
#                    with a keyframe asked midway
#          walk      the video while the client reads late, so the dial walks
#                    down and back up, then paused, so the desktop settles on
#                    a frame of it
#   mixed  mixed     minutes at the desk, quiet and busy in turn, the video
#                    in a window and full screen among them; not in `all`,
#                    and its own length, MIXED_LENGTH (300 by default)
set -euo pipefail

# The sizes are four desktops at scale 1 and five at scale 2, the four again
# and a 1024x768 one. Eight are vp9-wasm's, sizes its benchmark has a quiet
# and a busy sample of under the names (`bench/run.sh` there) that
# scripts/vp9-samples.sh makes of these; 3200x2000 is here in place of its
# 3456x2168, so that each of the four is captured at both scales. A size is in
# pixels, so a desktop at scale 2 is half of it in points:
#   2048x1536@2   1024x768 points
#   2560x1600@2   1280x800 points
#   2880x1800@2   1440x900 points
#   3200x2000@2   1600x1000 points
#   3840x2160@2   1920x1080 points
SIZES="${SIZES:-1280x800 1440x900 1600x1000 1920x1080 2048x1536@2 2560x1600@2 2880x1800@2 3200x2000@2 3840x2160@2}"
SCENARIOS="${SCENARIOS:-all}"
QUALITY="${QUALITY:-90}"
LENGTH="${LENGTH:-20}"
FRAMES="${FRAMES:-150}"
OUT=/captures
RAW="$OUT/raw"
ADDR=127.0.0.1:5900
SCENARIOS_DIR=/opt/capture/scenarios
QUIET="terminal still uncover"
BUSY="flood browser drag video window walk"

export CAPTURE_MATERIAL=/tmp/capture-material
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" "$RAW"
chmod 700 "$XDG_RUNTIME_DIR"
# What the scenarios type goes through each session's sink, which reads it
# here (scenarios/lib.sh, `press`); held open both ways, so that neither a
# sink ending nor a key written finds nobody at the other end.
export CAPTURE_KEYS="$XDG_RUNTIME_DIR/capture-keys"
mkfifo "$CAPTURE_KEYS"
exec 3<>"$CAPTURE_KEYS"

log() { printf '%s %s\n' "$(date +%T)" "$*" >&2; }

# The groups spelled out, in order, each name once.
expand() {
	local name out=""
	for name in $1; do
		case "$name" in
			all) out="$out $QUIET $BUSY" ;;
			quiet) out="$out $QUIET" ;;
			busy) out="$out $BUSY" ;;
			*) out="$out $name" ;;
		esac
	done
	echo "$out" | tr ' ' '\n' | awk 'NF && !seen[$0]++' | tr '\n' ' '
}
SCENARIOS=$(expand "$SCENARIOS")
if [[ " $SCENARIOS " == *" video "* || " $SCENARIOS " == *" walk "* || " $SCENARIOS " == *" window "* || " $SCENARIOS " == *" mixed "* ]] && [[ ! -s "${CAPTURE_VIDEO:-}" ]]; then
	log "the video, the window, the walk and the mixed play CAPTURE_VIDEO, and it is '${CAPTURE_VIDEO:-}'"
	exit 1
fi

# ── The desktop ──────────────────────────────────────────────────────────────

sway -c /opt/capture/sway.config >"$OUT/sway.log" 2>&1 &
SWAY=$!
for _ in $(seq 100); do
	sock=$(ls "$XDG_RUNTIME_DIR"/wayland-? 2>/dev/null | head -n 1 || true)
	[[ -n "$sock" ]] && break
	sleep 0.2
done
[[ -n "${sock:-}" ]] || { log "sway did not come up"; cat "$OUT/sway.log" >&2; exit 1; }
export WAYLAND_DISPLAY
WAYLAND_DISPLAY=$(basename "$sock")
export SWAYSOCK
SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.* | head -n 1)
log "sway on $WAYLAND_DISPLAY"

wlshare --config /opt/capture/wlshare.toml --capture-frames "$RAW" >"$OUT/wlshare.log" 2>&1 &
WLSHARE=$!
for _ in $(seq 100); do
	grep -q "listening on" "$OUT/wlshare.log" 2>/dev/null && break
	sleep 0.2
done
log "wlshare listening"

cleanup() {
	kill "$WLSHARE" "$SWAY" 2>/dev/null || true
}
trap cleanup EXIT

# ── The session around each scenario ─────────────────────────────────────────

# Connect vp9-sink for the scenario's length at WxH, with any further flags,
# in the background; `finish` waits for it and names the capture it made.
sink() {
	vp9-sink "$ADDR" --size "$W"x"$H" --quality "$QUALITY" --seconds "$LENGTH" --keys "$@" <&3 2>>"$OUT/sink.log" &
	SINK=$!
	# Its first frame, which opens the capture: a keyframe, then the content.
	sleep 1.5
}

# Run a scenario script and the session that captures it, with any flags for
# the sink. The script opens its windows and says it is ready
# (scenarios/lib.sh), under a session of their own at the size the capture
# begins at (START, WxH unless set): the daemon's framebuffer takes its size
# from a client alone and keeps its picture only while one is connected,
# which is what a client connecting is first sent, so this one leaves it at
# that size with the windows drawn, and ends, its capture thrown away. Then
# the sink connects, and at its first frame the script is told to go, for
# the session's length (SCRIPT_LENGTH, if the script is to play longer). A
# script that fails, or is not ready in a minute, did not play what its
# capture would be named for.
READY="$XDG_RUNTIME_DIR/scenario"
BROKEN=""
FAILED=""
play() {
	local script=$1 before waited=0
	shift
	rm -f "$READY".*
	vp9-sink "$ADDR" --size "${START:-${W}x${H}}" --quality "$QUALITY" --seconds 600 </dev/null 2>>"$OUT/sink.log" &
	before=$!
	sleep 1.5
	CAPTURE_READY="$READY" "$SCENARIOS_DIR/$script.sh" "${SCRIPT_LENGTH:-$LENGTH}" 2>>"$OUT/scenarios.log" &
	SCENARIO=$!
	until [[ -e "$READY.ready" ]] || ! kill -0 "$SCENARIO" 2>/dev/null || ((++waited > 600)); do
		sleep 0.1
	done
	kill "$before" 2>/dev/null || true
	wait "$before" 2>/dev/null || true
	sleep 1
	rm -f "$RAW"/*.wlcap
	SINK=""
	if [[ ! -e "$READY.ready" ]]; then
		BROKEN="the scenario was not ready"
		return
	fi
	sink "$@"
	touch "$READY.go"
}

# Wait for the sink, stop the script, close every window, and move the
# session's capture to the scenario's name. A script still playing past the
# session is stopped; one that ended before it ended as it would have. A
# session whose scenario or sink failed leaves no capture, and the run ends
# in failure once the others are done.
finish() {
	local name=$1
	if [[ -n "$SINK" ]]; then
		wait "$SINK" || BROKEN="vp9-sink ended with $?"
	fi
	if kill -0 "$SCENARIO" 2>/dev/null; then
		pkill -P "$SCENARIO" 2>/dev/null || true
		kill "$SCENARIO" 2>/dev/null || true
		wait "$SCENARIO" 2>/dev/null || true
	else
		wait "$SCENARIO" || BROKEN="${BROKEN:-the scenario ended with $?}"
	fi
	swaymsg -q '[app_id=".*"] kill' || true
	sleep 1
	local newest
	newest=$(ls -t "$RAW"/*.wlcap 2>/dev/null | head -n 1 || true)
	[[ -n "$newest" ]] || BROKEN="${BROKEN:-no capture was written}"
	if [[ -n "$BROKEN" ]]; then
		log "$name-$LABEL: $BROKEN"
		FAILED="$FAILED $name-$LABEL"
		BROKEN=""
		[[ -z "$newest" ]] || rm -f "$newest"
		return
	fi
	mv "$newest" "$OUT/$name-$LABEL.wlcap"
	# Compressed at once, over one a run before left, so that a run of many
	# sizes never holds them all uncompressed.
	zstd -T0 -q -f --rm "$OUT/$name-$LABEL.wlcap"
	log "$name-$LABEL: $(du -h "$OUT/$name-$LABEL.wlcap.zst" | cut -f1)"
}

scenario_terminal() {
	# Starts at the size before this one and resizes into it, so the resize
	# and the keyframe it brings are captured too.
	START=$PREV play terminal --size "$PREV" --resize 2:"$W"x"$H"
	finish terminal
}

scenario_still() {
	play still
	finish still
}

scenario_uncover() {
	play uncover
	finish uncover
}

scenario_flood() {
	# Every frame of it is the whole picture new, the slowest there is to
	# code, and a large desktop gets a few a second: the session runs on
	# past its length until FRAMES have come, so that there are enough of
	# them to cut a sample from, and the flood until the session ends.
	SCRIPT_LENGTH=$((LENGTH * 10)) play flood --frames "$FRAMES"
	finish flood
}

scenario_browser() {
	play browser
	finish browser
}

scenario_drag() {
	play drag
	finish drag
}

scenario_video() {
	play video --keyframe 10
	finish video
}

scenario_window() {
	play window --keyframe 10
	finish window
}

scenario_walk() {
	# The sink reads late for six seconds, a step down the dial a second,
	# then promptly, for the steps back up; the video pauses seven seconds
	# before the end, partway up, so the desktop goes quiet below the
	# ceiling on a frame of the video and the settle of that frame is
	# captured: at five, a dial that climbed quickly was back at the
	# ceiling first and there was no settle. Best with LENGTH at 20 or more.
	PAUSE_BEFORE=7 play video --slow 4:6:200
	finish walk
}

scenario_mixed() {
	# Minutes long, so the session and the script get a length of their own.
	LENGTH=${MIXED_LENGTH:-300} play mixed
	finish mixed
}

# ── The run ──────────────────────────────────────────────────────────────────

PREV=1280x720
for size in $SIZES; do
	LABEL=$size
	scale=1
	if [[ "$size" == *@2 ]]; then
		scale=2
		size=${size%@2}
	fi
	W=${size%x*}
	H=${size#*x}
	swaymsg -q "output HEADLESS-1 scale $scale"
	log "== $LABEL"
	for scenario in $SCENARIOS; do
		"scenario_$scenario"
	done
	PREV=$size
done

shopt -s nullglob
captures=("$OUT"/*.wlcap.zst)
shopt -u nullglob
cp "$CAPTURE_MATERIAL"/*.log "$OUT/" 2>/dev/null || true
if [[ ${#captures[@]} -eq 0 ]]; then
	log "no captures were written"
	exit 1
fi
rmdir "$RAW" 2>/dev/null || true
{
	echo "# Frame captures"
	echo
	echo "What a wlshare session handed its VP9 encoder, exact, written by \`wlshare --capture-frames\` in the capture container (\`docker/capture/\`) on $(date -u +%Y-%m-%d): the capture file of \`crates/wlshare-vp9/src/capture.rs\`, \`zstd\` compressed. One file per scenario and size, \`<scenario>-<WxH>[@2]\`, where \`@2\` is a desktop drawn at scale 2. Quality $QUALITY, $LENGTH seconds each; the content is \`docker/capture/scenarios/\`, whose windows are open and drawn when a session begins."
	echo
	echo "Quiet:"
	echo "- terminal: a resize into the size from the size before, then a terminal typed into one key at a time and scrolled"
	echo "- still: a page in Chromium left alone"
	echo "- uncover: a terminal over a page moved aside, then closed"
	echo
	echo "Busy:"
	echo "- flood: a terminal flooded with coloured random lines, every line new, for as long past the $LENGTH seconds as $FRAMES frames took"
	echo "- browser: a long page in Chromium scrolled by wheel, by key, and back"
	echo "- drag: a floating terminal dragged around the desktop"
	echo "- video: \`$(basename "${CAPTURE_VIDEO:-none}")\` playing full screen from its start, cropped to fill it, with a keyframe asked at 10 s"
	echo "- window: the video playing in a floating 16:9 window 55% of the desktop's width, centred over the page, from its start, the rest of the desktop still, with a keyframe asked at 10 s"
	echo "- walk: the video while the client reads 200 ms late from 4 s to 10 s, so the dial walks down and back up, then paused 7 s before the end, partway back up, so the desktop settles on a frame of the video"
	echo
	echo "$(sway --version), $(wlshare --version)"
} >"$OUT/README.md"
ls -la "$OUT" >&2
if [[ -n "$FAILED" ]]; then
	log "failed, and left no capture:$FAILED"
	exit 1
fi
log "done"
