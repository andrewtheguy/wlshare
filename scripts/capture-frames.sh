#!/usr/bin/env bash
# Build and run the capture container (docker/capture.Dockerfile): a headless
# sway, the daemon with `--capture-frames`, and the scripted desktop of
# docker/capture/run.sh, writing what the video encoder was handed into a
# directory on this host, one `<scenario>-<WxH>.wlcap.zst` per scenario
# and size, with a README.md beside them.
#
#   ./scripts/capture-frames.sh [--out DIR] [--video FILE] [--sizes "..."] [--scenarios "..."] [--seconds N] [--frames N]
#
# `--out` defaults to dist/captures. `--scenarios` takes the names
# docker/capture/run.sh lists or the groups `quiet`, `busy` and `all`; the
# sizes default to run.sh's. The captures are the source the encoders are
# measured on (scripts/capture-samples.sh cuts them shorter). `--video` is the video file the video and
# the walk play full screen, the window in a window and the mixed in both,
# handed to the container read-only; a run with any of them needs it.
#
# The container is docker's, or podman's where there is no docker or where
# `docker` is podman under that name. The desktop runs as whoever runs this,
# so that it can write the captures into `--out`: by their user and group id,
# except under a rootless podman, which maps the container's users into a
# range of its own and is told to make the image's user the one running this.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${here}/dist/captures"
env_args=()
mounts=()
while [[ $# -gt 0 ]]; do
	case "$1" in
		--out) out="$2"; shift 2 ;;
		--video)
			[[ -s "$2" ]] || { echo "$0: $2: no such video" >&2; exit 1; }
			video="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
			mounts+=(-v "$video:/video/$(basename "$2"):ro")
			env_args+=(-e "CAPTURE_VIDEO=/video/$(basename "$2")")
			shift 2 ;;
		--sizes) env_args+=(-e "SIZES=$2"); shift 2 ;;
		--scenarios) env_args+=(-e "SCENARIOS=$2"); shift 2 ;;
		--seconds) env_args+=(-e "LENGTH=$2"); shift 2 ;;
		--frames) env_args+=(-e "FRAMES=$2"); shift 2 ;;
		*) echo "$0: $1: not a flag" >&2; exit 2 ;;
	esac
done

engine=docker
command -v docker >/dev/null || engine=podman
command -v "$engine" >/dev/null || { echo "$0: neither docker nor podman is on the path" >&2; exit 1; }
run_args=()
if "$engine" --version | grep -qi podman && [[ "$("$engine" info --format '{{.Host.Security.Rootless}}')" == true ]]; then
	run_args+=(--userns=keep-id:uid=1000,gid=1000)
else
	run_args+=(--user "$(id -u):$(id -g)")
fi

"$engine" buildx build \
	--file "${here}/docker/capture.Dockerfile" \
	--load \
	--tag wlshare-capture \
	"${here}"

mkdir -p "$out"
"$engine" run --rm --init \
	--shm-size=1g \
	${run_args[@]+"${run_args[@]}"} \
	-v "$(cd "$out" && pwd):/captures" \
	${mounts[@]+"${mounts[@]}"} \
	${env_args[@]+"${env_args[@]}"} \
	wlshare-capture
echo "== $out"
cat "$out/README.md"
ls -la "$out"
