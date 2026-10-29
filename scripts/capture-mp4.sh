#!/usr/bin/env bash
# An H.264 MP4 of each capture, for watching what a session was handed:
# `wlcap frames` writes the capture's longest run of a size as raw pictures
# at a constant rate, its own timing kept, and ffmpeg's libx264 codes them.
# Nothing passes through the daemon's encoder, so the picture is the capture's
# but for libx264 at --crf (18 by default) and 4:2:0. Runs where the daemon
# builds; it needs ffmpeg with libx264, and zstd for a `.zst` capture.
#
#   ./scripts/capture-mp4.sh [--out DIR] [--fps N] [--crf N] CAPTURE...
#
# `--out` defaults to the directory of each capture; each MP4 is the
# capture's name without `.wlcap.zst`.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="" fps=30 crf=18
captures=()
while [[ $# -gt 0 ]]; do
	case "$1" in
		--out) out="$2"; shift 2 ;;
		--fps) fps="$2"; shift 2 ;;
		--crf) crf="$2"; shift 2 ;;
		-*) echo "$0: $1: not a flag" >&2; exit 2 ;;
		*) captures+=("$1"); shift ;;
	esac
done
[[ ${#captures[@]} -gt 0 ]] || { echo "$0: no capture" >&2; exit 2; }

cargo build -q --release --manifest-path "$here/Cargo.toml" -p wlshare --example wlcap
wlcap="$here/target/release/examples/wlcap"
for capture in "${captures[@]}"; do
	name=$(basename "$capture"); name=${name%.zst}; name=${name%.wlcap}
	dir=${out:-$(dirname "$capture")}
	mkdir -p "$dir"
	# Its longest run, the earliest of the longest, as `wlcap info` lists
	# the runs: its first frame and its size.
	read -r from size < <("$wlcap" info "$capture" | sed -nE 's/^([0-9]+x[0-9]+): frames ([0-9]+)\.\.([0-9]+),.*/\2 \3 \1/p' | awk '{ print $2 - $1, $1, $3 }' | sort -k1,1nr -k2,2n | head -n 1 | cut -d' ' -f2-)
	[[ "$from" =~ ^[0-9]+$ && "$size" =~ ^[0-9]+x[0-9]+$ ]] || { echo "$0: $capture: no run from wlcap info" >&2; exit 1; }
	# libx264's 4:2:0 takes even sides: an odd one is padded by a pixel.
	"$wlcap" frames "$capture" --from "$from" --fps "$fps" 2>/dev/null |
		ffmpeg -nostdin -loglevel error -y -f rawvideo -pix_fmt bgr0 -s "$size" -r "$fps" -i - \
			-vf 'pad=ceil(iw/2)*2:ceil(ih/2)*2' \
			-c:v libx264 -preset medium -crf "$crf" -pix_fmt yuv420p -movflags +faststart "$dir/$name.mp4"
	echo "$dir/$name.mp4 $size $(du -h "$dir/$name.mp4" | cut -f1)"
done
