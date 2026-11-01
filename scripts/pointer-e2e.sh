#!/usr/bin/env bash
# The daemon's pointers against a real compositor, with no browser: a
# throwaway headless labwc or Sway with two outputs side by side, the daemon on
# HEADLESS-1 with no password on a port of its own, one video-sink as the client
# on the desktop and another as the display beside it (HEADLESS-2), and
# cursor-watch saying where every motion landed. Each scenario starts all of it
# afresh and passes when each output's pointer was remade as often as the
# scenario expects and its last motion landed where it was sent. The logs are
# kept when a scenario fails, or with KEEP_LOGS=1 when none does.
#
#   [COMPOSITOR=labwc|sway] [PORT=5999] [SCENARIOS="edge drag ..."] [KEEP_LOGS=1] ./scripts/pointer-e2e.sh
#
# Run on a Linux host with the compositor and wlr-randr installed, inside no
# particular session: the compositor gets a socket of its own under
# XDG_RUNTIME_DIR, and nothing is handed to the user's systemd manager. Where
# the reload also changes the layout, the daemon remakes its pointers for that,
# and the reload scenarios see nothing to repair, and say so.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
compositor="${COMPOSITOR:-labwc}"
port="${PORT:-5999}"
scenarios="${SCENARIOS:-edge drag resize scale fractional rotate reload reload-far}"
runtime="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
command -v "${compositor}" >/dev/null || { echo "no ${compositor} to run" >&2; exit 2; }
command -v wlr-randr >/dev/null || { echo "no wlr-randr to lay the outputs out with" >&2; exit 2; }
if ss -ltn 2>/dev/null | grep -q "127.0.0.1:${port} "; then
	echo "127.0.0.1:${port} is taken: PORT=... picks another" >&2
	exit 2
fi

cargo build --quiet --manifest-path "${here}/Cargo.toml" --bin wlshare --example video-sink --example cursor-watch
bin="${CARGO_TARGET_DIR:-${here}/target}/debug"
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
# the scenario's own arrangement.
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
		# names its outputs has them: a reload then changes no layout and
		# brings no done, which would remake the pointers by itself.
		{
			echo "exec echo \$WAYLAND_DISPLAY > ${dir}/display"
			echo "output HEADLESS-1 pos 0 0"
			echo "output HEADLESS-2 pos 1280 0"
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
# check NAME OUT REMAKES LAST: the pointer of output OUT was remade REMAKES
# times (a range, as 0 or 0-1), and the last position the watch heard on it
# is LAST.
check() {
	local name=$1 out=$2 want=$3 last=$4
	local remakes got
	remakes=$(grep "making it again" "${scenario_dir}/wlshare.log" | grep -c "${out}," || true)
	got=$(grep "^${out} position " "${scenario_dir}/watch.log" | tail -n 1 | cut -d' ' -f3 || true)
	local low=${want%-*} high=${want#*-}
	if (( remakes < low || remakes > high )) || [[ ${got} != "${last}" ]]; then
		echo "FAIL ${name}: ${out} remade ${remakes} times (want ${want}), last at ${got:-nowhere} (want ${last})"
		failed=1
	else
		echo "pass ${name}: ${out} remade ${remakes} times, last at ${got}"
	fi
}

finish() {
	sleep 1
	stop
}

for scenario in ${scenarios}; do
	echo "== ${scenario} (${compositor})"
	LAYOUT="" FIRST_FLAGS="" BESIDE_FLAGS="" FIRST_SIZE=""
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
		check edge HEADLESS-1 0 640,360
		check edge HEADLESS-2 0 640,360
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
		check drag HEADLESS-1 0 640,360
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
		check resize HEADLESS-1 0 800,450
		check resize HEADLESS-2 0 500,300
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
		check scale HEADLESS-1 0 640,360
		check scale HEADLESS-2 0 640,360
		;;
	fractional)
		# The first output at 3456x1802 and scale 2.75, laid out as 1256x655
		# layout units, which wlroots truncates it to: a motion lands short of
		# its pixel by up to two of them, and the middle at 2591,901.
		FIRST_SIZE=3456x1802 LAYOUT="--output HEADLESS-1 --scale 2.75 --output HEADLESS-2 --pos 1256,0"
		start "${scenario}"
		for y in 100 600 1100 1700; do
			for x in 2593 3000 3300 3400 3455; do a "${x}" "${y}"; done
			b 640 360
		done
		a 2593 901
		finish
		check fractional HEADLESS-1 0 2591,901
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
		check rotate HEADLESS-1 0 360,640
		check rotate HEADLESS-2 0 640,360
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
			check "${scenario}" HEADLESS-1 0-1 640,360
			check "${scenario}" HEADLESS-2 0-1 640,360
		else
			for x in 700 800 900 1000 1100 1000; do a "${x}" 360; done
			finish
			check "${scenario}" HEADLESS-1 0-1 1000,360
		fi
		if ! grep -q "making it again" "${scenario_dir}/wlshare.log"; then
			echo "     (no pointer was remade: the binding held through the reload, as the APT repository's patched wlroots keeps it, or a layout change the reload brought remade the pointers first)"
		fi
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
