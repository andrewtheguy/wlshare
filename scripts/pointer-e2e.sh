#!/usr/bin/env bash
# The daemon's pointers against a real compositor, with no browser: a
# throwaway headless labwc or Sway with two outputs side by side, the daemon on
# HEADLESS-1 with no password on a port of its own, one video-sink as the client
# on the desktop and another as the display beside it (HEADLESS-2), and
# cursor-watch saying where every motion landed. Each scenario starts all of it
# afresh and passes when the last motion on each output landed where it was
# sent. The logs are kept when a scenario fails, or with KEEP_LOGS=1 when none
# does.
#
#   [COMPOSITOR=labwc|sway] [PORT=5999] [SCENARIOS="edge drag ..."] [KEEP_LOGS=1] [BIN=DIR] ./scripts/pointer-e2e.sh
#
# Run on a Linux host with the compositor and wlr-randr installed, inside no
# particular session: the compositor gets a socket of its own under
# XDG_RUNTIME_DIR, and nothing is handed to the user's systemd manager.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
compositor="${COMPOSITOR:-labwc}"
port="${PORT:-5999}"
scenarios="${SCENARIOS:-edge drag resize scale fractional fractional-left rotate reload reload-far inexact inexact-reload}"
runtime="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
command -v "${compositor}" >/dev/null || { echo "no ${compositor} to run" >&2; exit 2; }
command -v wlr-randr >/dev/null || { echo "no wlr-randr to lay the outputs out with" >&2; exit 2; }
if ss -ltn 2>/dev/null | grep -q "127.0.0.1:${port} "; then
	echo "127.0.0.1:${port} is taken: PORT=... picks another" >&2
	exit 2
fi

# BIN names a directory holding wlshare, examples/video-sink and
# examples/cursor-watch built elsewhere, for a host with the compositor and no
# toolchain; without it they are built here.
if [[ -n ${BIN:-} ]]; then
	bin="${BIN}"
else
	cargo build --quiet --manifest-path "${here}/Cargo.toml" --bin wlshare --example video-sink --example cursor-watch
	bin="${CARGO_TARGET_DIR:-${here}/target}/debug"
fi
work="$(mktemp -d)"
echo "== logs in ${work}"
pids=()

stop() {
	for pid in "${pids[@]}"; do
		kill "${pid}" 2>/dev/null || true
	done
	for pid in "${pids[@]}"; do
		wait "${pid}" 2>/dev/null || true
	done
	pids=()
	exec 7>&- 8>&-
}
trap stop EXIT

# start NAME: the compositor, the daemon, the watch and both sinks, the outputs
# 1280x720 (HEADLESS-1 at FIRST_SIZE if given) side by side, HEADLESS-1 left.
# Whatever LAYOUT holds is then passed to wlr-randr, after the sinks' sizes, as
# the scenario's own arrangement; PRELAYOUT is passed before the daemon starts,
# as what an earlier run or another client left; and EXTRA_CONFIG goes into
# Sway's configuration as a line of its own.
start() {
	local dir="${work}/$1"
	mkdir -p "${dir}/cfg"
	rm -f "${dir}/display"
	case "${compositor}" in
	labwc)
		echo "echo \"\$WAYLAND_DISPLAY\" > ${dir}/display" > "${dir}/cfg/autostart"
		env -u WAYLAND_DISPLAY -u DISPLAY -u LABWC_UPDATE_ACTIVATION_ENV XDG_RUNTIME_DIR="${runtime}" \
			WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=2 WLR_LIBINPUT_NO_DEVICES=1 \
			labwc -C "${dir}/cfg" > "${dir}/compositor.log" 2>&1 &
		;;
	sway)
		# The outputs where the scenarios lay them out, as a configuration that
		# names its outputs has them: a reload then changes no layout, and
		# tests the pointer alone.
		{
			echo "exec echo \$WAYLAND_DISPLAY > ${dir}/display"
			echo "output HEADLESS-1 pos 0 0"
			echo "output HEADLESS-2 pos 1280 0"
			echo "${EXTRA_CONFIG:-}"
		} > "${dir}/cfg/sway"
		env -u WAYLAND_DISPLAY -u DISPLAY -u SWAYSOCK XDG_RUNTIME_DIR="${runtime}" \
			WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=2 WLR_LIBINPUT_NO_DEVICES=1 \
			sway -c "${dir}/cfg/sway" > "${dir}/compositor.log" 2>&1 &
		;;
	*) echo "COMPOSITOR is labwc or sway" >&2; exit 2 ;;
	esac
	comp=$!
	pids+=("${comp}")
	for _ in $(seq 100); do [[ -s "${dir}/display" ]] && break; sleep 0.1; done
	[[ -s "${dir}/display" ]] || { echo "${compositor} did not start: ${dir}/compositor.log" >&2; exit 1; }
	export WAYLAND_DISPLAY="$(cat "${dir}/display")" XDG_RUNTIME_DIR="${runtime}"
	if [[ ${compositor} == sway ]]; then
		export SWAYSOCK="$(ls -t "${runtime}"/sway-ipc.*."${comp}".sock | head -n 1)"
	fi
	if [[ -n ${PRELAYOUT:-} ]]; then
		wlr-randr ${PRELAYOUT}
		sleep 0.5
	fi
	printf 'listen = "127.0.0.1:%s"\noutput = "HEADLESS-1"\n' "${port}" > "${dir}/config.toml"
	RUST_LOG=info,wlshare::input=trace,wlshare::compositor=trace,wlshare::cursor=trace,wlshare::outputs=debug \
		"${bin}/wlshare" -c "${dir}/config.toml" > "${dir}/wlshare.log" 2>&1 &
	pids+=($!)
	for _ in $(seq 100); do grep -q listening "${dir}/wlshare.log" 2>/dev/null && break; sleep 0.1; done
	"${bin}/examples/cursor-watch" > "${dir}/watch.log" 2>&1 &
	pids+=($!)
	mkfifo "${dir}/first" "${dir}/beside"
	"${bin}/examples/video-sink" "127.0.0.1:${port}" --size "${FIRST_SIZE:-1280x720}" --seconds 60 --keys ${FIRST_FLAGS:-} < "${dir}/first" > "${dir}/first.log" 2>&1 &
	pids+=($!)
	exec 7> "${dir}/first"
	sleep 1
	"${bin}/examples/video-sink" "127.0.0.1:${port}" --size 1280x720 --seconds 60 --keys --beside ${BESIDE_FLAGS:-} < "${dir}/beside" > "${dir}/beside.log" 2>&1 &
	pids+=($!)
	exec 8> "${dir}/beside"
	sleep 1.5
	wlr-randr --output HEADLESS-1 --pos 0,0 --output HEADLESS-2 --pos 1280,0 ${LAYOUT:-}
	sleep 0.5
	scenario_dir="${dir}"
}

# a X Y [BUTTONS], b X Y [BUTTONS]: a motion on the first display's or the
# display beside's, a little after the last.
a() { echo "@$1,$2,${3:-0}" >&7; sleep "${PACE:-0.12}"; }
b() { echo "@$1,$2,${3:-0}" >&8; sleep "${PACE:-0.12}"; }

reload() {
	case "${compositor}" in
	labwc) kill -HUP "${comp}" ;;
	sway) swaymsg reload >/dev/null || { echo "swaymsg reload failed, SWAYSOCK=${SWAYSOCK:-}" >&2; exit 1; } ;;
	esac
	sleep 0.8
}

failed=0
# check NAME OUT LAST: the last position the watch heard on output OUT is LAST.
check() {
	local name=$1 out=$2 last=$3
	local got
	got=$(grep "^${out} position " "${scenario_dir}/watch.log" | tail -n 1 | cut -d' ' -f3 || true)
	if [[ ${got} != "${last}" ]]; then
		echo "FAIL ${name}: ${out} last at ${got:-nowhere} (want ${last})"
		failed=1
	else
		echo "pass ${name}: ${out} last at ${got}"
	fi
}

finish() {
	sleep 1
	stop
}

for scenario in ${scenarios}; do
	echo "== ${scenario} (${compositor})"
	LAYOUT="" PRELAYOUT="" FIRST_FLAGS="" BESIDE_FLAGS="" FIRST_SIZE="" EXTRA_CONFIG=""
	case "${scenario}" in
	edge)
		# Across the edge the outputs share, back and forth between the
		# displays, a motion past the edge among them.
		start "${scenario}"
		for y in 40 200 360 520 680; do
			a 1270 "${y}"; a 1279 "${y}"; a 1300 "${y}"; b 0 "${y}"; b 3 "${y}"; a 1279 "$((y + 5))"; b 1 "$((y + 5))"
		done
		a 640 360; b 640 360
		finish
		check edge HEADLESS-1 640,360
		check edge HEADLESS-2 640,360
		;;
	drag)
		# A button held from the first display past its edge, which the page
		# sends as positions past the extent, and let go there.
		start "${scenario}"
		a 1200 300 1
		for x in 1240 1270 1279 1300 1500 2000 2500; do a "${x}" 300 1; done
		a 2500 300 0
		a 640 360
		finish
		check drag HEADLESS-1 640,360
		;;
	resize)
		# Both outputs resized under motions that go on throughout, up and
		# down, one past the extent as the first grows.
		FIRST_FLAGS="--resize 4:1600x900" BESIDE_FLAGS="--resize 5:1000x600"
		start "${scenario}"
		for i in $(seq 0 40); do
			a "$(( (i * 61) % 1280 ))" "$(( (i * 37) % 720 ))"; b "$(( (i * 53) % 1000 ))" "$(( (i * 29) % 600 ))"
			(( i == 20 )) && a 1280 100
		done
		sleep 1
		a 800 450; b 500 300
		finish
		check resize HEADLESS-1 800,450
		check resize HEADLESS-2 500,300
		;;
	scale)
		# The first output at scale 2, so its 1280x720 pixels are 640x360 of
		# the layout, the second beside it there.
		LAYOUT="--output HEADLESS-1 --scale 2 --output HEADLESS-2 --pos 640,0"
		start "${scenario}"
		for y in 60 360 660; do
			a 1279 "${y}"; a 1300 "${y}"; b 0 "${y}"; a 600 "${y}"; b 700 "${y}"
		done
		a 640 360; b 640 360
		finish
		check scale HEADLESS-1 640,360
		check scale HEADLESS-2 640,360
		;;
	fractional)
		# The first output at 3456x1802 and scale 2.75, declared by the client
		# as zDesk declares a browser's density, so the scale is the daemon's
		# own, and laid out as 1256x655 layout units, which wlroots truncates
		# it to: a motion lands on its pixel, the middle at 2593,901, and the
		# last two pixels across, which are on no output, at the box's far
		# edge.
		FIRST_SIZE=3456x1802 FIRST_FLAGS="--density 2.75" LAYOUT="--output HEADLESS-2 --pos 1256,0"
		start "${scenario}"
		for y in 100 600 1100 1700; do
			for x in 2593 3000 3300 3400 3455; do a "${x}" "${y}"; done
			b 640 360
		done
		a 2593 901
		finish
		check fractional HEADLESS-1 2593,901
		;;
	fractional-left)
		# The same, with the output already at 2.75 before the daemon starts,
		# as an earlier run or another client leaves it: the scale reads the
		# same but is not known to be the daemon's own, so the declaration
		# sets it again, and a motion lands on its pixel.
		FIRST_SIZE=3456x1802 PRELAYOUT="--output HEADLESS-1 --custom-mode 3456x1802 --scale 2.75" FIRST_FLAGS="--density 2.75" LAYOUT="--output HEADLESS-2 --pos 1256,0"
		start "${scenario}"
		for y in 100 1100; do
			for x in 2593 3400; do a "${x}" "${y}"; done
			b 640 360
		done
		a 2593 901
		finish
		check fractional-left HEADLESS-1 2593,901
		if ! grep -q "following client" "${scenario_dir}/wlshare.log"; then
			echo "FAIL fractional-left: the declaration set nothing"
			failed=1
		fi
		;;
	rotate)
		# The first output turned a quarter, so its upright place is 720x1280
		# while its framebuffer is 1280x720: the middle lands in the middle.
		LAYOUT="--output HEADLESS-1 --transform 90 --output HEADLESS-2 --pos 720,0"
		start "${scenario}"
		for p in "100 100" "1000 600" "50 700" "1270 20" "1300 300" "640 360"; do
			a ${p}
		done
		b 640 360
		finish
		check rotate HEADLESS-1 360,640
		check rotate HEADLESS-2 640,360
		;;
	reload|reload-far)
		# A reload, and then motions on both displays: across the whole of
		# each, or with reload-far on the first alone and only in the half its
		# stretched pointer puts on the second output.
		start "${scenario}"
		a 640 360; b 640 360
		reload
		if [[ ${scenario} == reload ]]; then
			for x in 100 400 700 1000 1200; do a "${x}" 360; b "${x}" 360; done
			a 640 360; b 640 360
			finish
			check "${scenario}" HEADLESS-1 640,360
			check "${scenario}" HEADLESS-2 640,360
		else
			for x in 700 800 900 1000 1100 1000; do a "${x}" 360; done
			finish
			check "${scenario}" HEADLESS-1 1000,360
		fi
		;;
	inexact)
		# A scale from the configuration, 1.1 and then 1.7, which wlroots lays
		# the first output's 1280x720 pixels out by, as 1163x654 and 752x423,
		# and says over wlr-output-management a 1/256 step short, as
		# 1.09765625 and 1.69921875: a motion is placed by what the wire's
		# step and the box leave, not by the scale the wire says. Sway alone
		# takes a scale from its configuration here.
		if [[ ${compositor} != sway ]]; then
			echo "     (skipped: a configured scale is Sway's)"
			continue
		fi
		for pair in 1.1:1163 1.7:752; do
			EXTRA_CONFIG="output HEADLESS-1 scale ${pair%:*}" LAYOUT="--output HEADLESS-2 --pos ${pair#*:},0"
			start "${scenario}-${pair%:*}"
			for y in 100 400 700; do
				a 1279 "${y}"; a 1300 "${y}"; b 0 "${y}"; a 600 "${y}"
			done
			a 640 360; b 640 360
			finish
			check "inexact ${pair%:*}" HEADLESS-1 640,360
			check "inexact ${pair%:*}" HEADLESS-2 640,360
		done
		;;
	inexact-reload)
		# The client declares a scale that is no eighth, 1.1 on 1280x720 and
		# 1.7 on 1920x1080, which Sway rounds to its 1/120 and so applies as
		# declared while the wire says 1.09765625 and 1.69921875, and a reload
		# then applies the configuration's own, the same: the scale is placed
		# by what the wire and the box leave throughout, never taken as the
		# wire's, which would put 1088 at 1.7 on 1089, and the scale the
		# compositor announces at the reload forgets what the daemon set.
		if [[ ${compositor} != sway ]]; then
			echo "     (skipped: a configured scale is Sway's)"
			continue
		fi
		for spec in "1.1 1280x720 1166 600,100" "1.7 1920x1080 1129 1088,400"; do
			read -r s size pos last <<< "${spec}"
			EXTRA_CONFIG="output HEADLESS-1 scale ${s}" FIRST_SIZE="${size}" FIRST_FLAGS="--density ${s}" LAYOUT="--output HEADLESS-2 --pos ${pos},0"
			start "${scenario}-${s}"
			a 600 100; a 1088 400; b 640 360
			reload
			a 600 100; a 1088 400; b 640 360; a "${last%,*}" "${last#*,}"
			finish
			check "inexact-reload ${s}" HEADLESS-1 "${last}"
			check "inexact-reload ${s}" HEADLESS-2 640,360
		done
		;;
	*)
		echo "${scenario}: no such scenario" >&2
		exit 2
		;;
	esac
done

if (( failed )); then
	echo "== failed: the logs are in ${work}"
	exit 1
fi
echo "== all passed"
if [[ -n ${KEEP_LOGS:-} ]]; then
	echo "== the logs are in ${work}"
else
	rm -rf "${work}"
fi
