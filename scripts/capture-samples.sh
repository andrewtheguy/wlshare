#!/usr/bin/env bash
# Cut the captures scripts/capture-frames.sh left into shorter captures of the
# same names, for an encoder measured on a part of each.
#
#   ./scripts/capture-samples.sh [--captures DIR] [--out DIR] [--frames N] [--clean]
#
# For every capture in `--captures` (dist/captures), `--out`
# (dist/capture-samples) gets `--frames` (120) frames of it under its name,
# cut by `wlcap trim --frames` from the capture's longest run of a size,
# past the whole pictures that open it where nothing follows them for over a
# second: a quiet scenario's (still, uncover) first N, and a busy one's the N
# in a row whose frames said the most pixels changed, a whole picture
# counting as none, the earliest where
# several tie. The terminal's, the video's and the window's N are where
# they were chosen (`chosen` below): the terminal's by eye, from just before
# a 40-line output to a second and a half after a page printed across the
# screen; the video's and the window's the N that end before the clip's
# first cut to black, at 10 s, or the keyframe asked then, the first
# explosion at its peak, where its frames are the least black (every frame
# of a video says the same of the picture changed, the whole of it full
# screen or the window's, so the busiest would be the earliest, the
# explosion growing out of black). The
# cut's first frame is a whole picture with a keyframe asked, the picture as
# it stood, and the rest are as the session handed them to its encoder. A
# capture whose longest run is N frames or fewer is copied whole, and so are
# walk, whose frames slow and come back over all of it, a cut keeping only
# the slowing, mixed, minutes at the desk of which a cut
# would keep only the busiest, the video full screen, and small and many,
# whose parts are found by their seconds. A README.md says what
# this run put there. Nothing in `--out` is removed unless `--clean` is given,
# which first empties it of captures. Needs cargo and zstd on the path.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
captures="${here}/dist/captures"
out="${here}/dist/capture-samples"
frames=120
clean=""
while [[ $# -gt 0 ]]; do
	case "$1" in
		--captures) captures="$2"; shift 2 ;;
		--out) out="$2"; shift 2 ;;
		--frames) frames="$2"; shift 2 ;;
		--clean) clean=1; shift ;;
		*) echo "$0: $1: not a flag" >&2; exit 2 ;;
	esac
done
# The quiet scenarios of docker/capture/run.sh; every other is busy.
quiet=" terminal still uncover "
# The cuts chosen, `name from`: the frame their N start at, which the
# terminal's 2026-10-13 captures put 3.5 s before the for loop's output, and
# the video's and the window's 120 before the clip's first black frame at
# the cut at 10 s or the keyframe asked at 10 s, whichever comes first.
chosen="
terminal-1280x800 82
terminal-1440x900 85
terminal-1600x1000 83
terminal-1920x1080 84
terminal-2048x1536@2 83
terminal-2560x1600@2 83
terminal-2880x1800@2 82
terminal-3200x2000@2 82
terminal-3456x2168@2 81
terminal-3840x2160@2 79
video-1280x800 88
video-1440x900 92
video-1600x1000 90
video-1920x1080 64
video-2048x1536@2 95
video-2560x1600@2 85
video-2880x1800@2 75
video-3200x2000@2 64
video-3456x2168@2 64
video-3840x2160@2 64
window-1280x800 123
window-1440x900 120
window-1600x1000 119
window-1920x1080 115
window-2048x1536@2 112
window-2560x1600@2 108
window-2880x1800@2 98
window-3200x2000@2 94
window-3456x2168@2 92
window-3840x2160@2 90
"
# The scenarios copied whole, whatever their length.
whole=" walk mixed small many "

log() { printf '%s %s\n' "$(date +%T)" "$*" >&2; }

command -v zstd >/dev/null || { echo "$0: zstd is not on the path" >&2; exit 1; }
shopt -s nullglob
all=("$captures"/*.wlcap.zst)
shopt -u nullglob
[[ ${#all[@]} -gt 0 ]] || { echo "$0: no *.wlcap.zst in $captures: run scripts/capture-frames.sh" >&2; exit 1; }

cargo build --release --manifest-path "${here}/Cargo.toml" -p wlshare --example wlcap
wlcap="${here}/target/release/examples/wlcap"

mkdir -p "$out"
[[ -z "$clean" ]] || rm -f "$out"/*.wlcap.zst
rows=""
for capture in "${all[@]}"; do
	name="$(basename "$capture")"
	scenario="${name%%-*}"
	# `WxH: frames A..B, ...` for each run of a size.
	info="$("$wlcap" info "$capture")"
	total="$(head -n 1 <<<"$info" | cut -d' ' -f1)"
	longest="$(sed -n 's/.*: frames \([0-9]*\)\.\.\([0-9]*\),.*/\1 \2/p' <<<"$info" | awk '{ n = $2 - $1 + 1; if (n > most) most = n } END { print most }')"
	if [[ "$longest" -le "$frames" || "$whole" == *" $scenario "* ]]; then
		cp "$capture" "$out/$name.part" && mv "$out/$name.part" "$out/$name"
		window="all $total"
	else
		from="$(awk -v name="${name%.wlcap.zst}" '$1 == name { print $2 }' <<<"$chosen")"
		if [[ -n "$from" ]]; then
			how=(--from "$from")
		elif [[ "$quiet" == *" $scenario "* ]]; then
			how=()
		else
			how=(--busiest)
		fi
		said="$(mktemp)"
		"$wlcap" trim "$capture" --out "$out/$name" --frames "$frames" "${how[@]}" >/dev/null 2>"$said" || { cat "$said" >&2; rm -f "$said"; exit 1; }
		# `wlcap: trim: frames A..B`
		window="$(sed -n 's/.*: frames \([0-9]*\)\.\.\([0-9]*\)$/\1–\2/p' "$said") of $total"
		rm -f "$said"
	fi
	rows+="| \`$name\` | $window | $(du -h "$out/$name" | cut -f1) |"$'\n'
	log "$name: $window"
done
{
	echo "# Capture samples"
	echo
	echo "The captures of what a wlshare session handed its video encoder, cut to $frames frames each by \`scripts/capture-samples.sh\` in the wlshare repository on $(date -u +%Y-%m-%d), under the names of the captures they are cut from, for an encoder measured on a part of each. What the scenarios are is in the README.md beside the captures."
	echo
	echo "Each is cut from the capture's longest run of a size, past the whole pictures that open it where nothing follows them for over a second: a quiet scenario's (still, uncover) first $frames frames, a busy one's the $frames in a row whose frames said the most pixels changed, a whole picture counting as none, the earliest where several tie, the terminal's where they were chosen by eye, from just before the for loop's 40 lines of output to a second and a half after the page printed across the screen, and the video's and the window's the $frames that end before the clip's first cut to black, at 10 s, or the keyframe asked then, the first explosion at its peak (every frame of a video says the same of the picture changed, the whole of it full screen or the window's, so the busiest would be the earliest, the explosion growing out of black). Its first frame is a whole picture with a keyframe asked, the picture as it stood, its times are counted from it, and every frame after is as captured. A capture whose longest run is $frames frames or fewer is here whole, and so are walk, whose frames slow and come back over all of it, mixed, minutes at the desk, and small and many, the small movements of a desk one kind at a time and several at once, their parts found by their seconds."
	echo
	echo "| Capture | Frames | Size |"
	echo "|---|---|---|"
	printf '%s' "$rows"
} >"$out/README.md"
log "== $out"
