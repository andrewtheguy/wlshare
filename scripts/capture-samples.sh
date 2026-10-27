#!/usr/bin/env bash
# Cut the captures scripts/capture-frames.sh left into shorter captures of the
# same names, for an encoder measured on a part of each as vp9-wasm's decoder
# is on the samples of scripts/vp9-samples.sh, which reads a directory of
# them as it reads the captures whole (`--captures DIR`).
#
#   ./scripts/capture-samples.sh [--captures DIR] [--out DIR] [--frames N] [--clean]
#
# For every capture in `--captures` (dist/captures), `--out`
# (dist/capture-samples) gets `--frames` (120) frames of it under its name,
# cut by `vp9cap trim --frames` from the capture's longest run of a size: a
# quiet scenario's (terminal, still, uncover) first N, as vp9-samples.sh cuts
# the terminal, and a busy one's the N in a row whose frames said the most
# pixels changed, as it cuts the video, the earliest where several tie. The
# cut's first frame is a whole picture with a keyframe asked, the picture as
# it stood, and the rest are as the session handed them to its encoder. A
# capture whose longest run is N frames or fewer is copied whole, and so are
# walk, whose dial walks down, back up and settles over all of it, a cut
# keeping only the walk down, and mixed, minutes at the desk of which a cut
# would keep only the busiest, the video full screen. A README.md says what
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
# The scenarios copied whole, whatever their length.
whole=" walk mixed "

log() { printf '%s %s\n' "$(date +%T)" "$*" >&2; }

command -v zstd >/dev/null || { echo "$0: zstd is not on the path" >&2; exit 1; }
shopt -s nullglob
all=("$captures"/*.wlcap.zst)
shopt -u nullglob
[[ ${#all[@]} -gt 0 ]] || { echo "$0: no *.wlcap.zst in $captures: run scripts/capture-frames.sh" >&2; exit 1; }

cargo build --release --manifest-path "${here}/Cargo.toml" -p wlshare-vp9 --example vp9cap
vp9cap="${here}/target/release/examples/vp9cap"

mkdir -p "$out"
[[ -z "$clean" ]] || rm -f "$out"/*.wlcap.zst
rows=""
for capture in "${all[@]}"; do
	name="$(basename "$capture")"
	scenario="${name%%-*}"
	# `WxH: frames A..B, ...` for each run of a size.
	info="$("$vp9cap" info "$capture")"
	total="$(head -n 1 <<<"$info" | cut -d' ' -f1)"
	longest="$(sed -n 's/.*: frames \([0-9]*\)\.\.\([0-9]*\),.*/\1 \2/p' <<<"$info" | awk '{ n = $2 - $1 + 1; if (n > most) most = n } END { print most }')"
	if [[ "$longest" -le "$frames" || "$whole" == *" $scenario "* ]]; then
		cp "$capture" "$out/$name.part" && mv "$out/$name.part" "$out/$name"
		window="all $total"
	else
		[[ "$quiet" == *" $scenario "* ]] && how=() || how=(--busiest)
		said="$(mktemp)"
		"$vp9cap" trim "$capture" --out "$out/$name" --frames "$frames" "${how[@]}" >/dev/null 2>"$said" || { cat "$said" >&2; rm -f "$said"; exit 1; }
		# `vp9cap: trim: frames A..B`
		window="$(sed -n 's/.*: frames \([0-9]*\)\.\.\([0-9]*\)$/\1–\2/p' "$said") of $total"
		rm -f "$said"
	fi
	rows+="| \`$name\` | $window | $(du -h "$out/$name" | cut -f1) |"$'\n'
	log "$name: $window"
done
{
	echo "# Capture samples"
	echo
	echo "The captures of what a wlshare session handed its VP9 encoder, cut to $frames frames each by \`scripts/capture-samples.sh\` in the wlshare repository on $(date -u +%Y-%m-%d), under the names of the captures they are cut from, for an encoder measured on a part of each. What the scenarios are is in the README.md beside the captures."
	echo
	echo "Each is cut from the capture's longest run of a size: a quiet scenario's (terminal, still, uncover) first $frames frames, and a busy one's the $frames in a row whose frames said the most pixels changed, the earliest where several tie. Its first frame is a whole picture with a keyframe asked, the picture as it stood, its times are counted from it, and every frame after is as captured. A capture whose longest run is $frames frames or fewer is here whole, and so are walk, whose dial walks down, back up and settles over all of it, and mixed, minutes at the desk."
	echo
	echo "| Capture | Frames | Size |"
	echo "|---|---|---|"
	printf '%s' "$rows"
} >"$out/README.md"
log "== $out"
