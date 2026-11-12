#!/usr/bin/env bash
# Small: four minutes of the small movements of a desk, one kind at a time
# with the desktop left still for three seconds or more between them, for a
# picture that settles to be measured on what moves a little: whether each
# is worth a region's stream, and what its settle costs after. A page in
# Chromium lays every part out at a fixed place and plays it by its
# own clock from the key that starts it; the pointer and the keys come
# through the session's client, as a person's do, so CAPTURE_KEYS must name
# what it reads (lib.sh, `press`), and the desktop must be 1280 by 820 points
# or more, the menu's last item ending 813 down. LENGTH should be 240 or more. Each part's start goes to stderr
# with its second:
#
#     3  text typed into a field, its caret blinking after
#    12  a spinner, 32 points
#    21  a spinner of 16 beside a word whose dots come and go
#    30  a stopwatch, ten times a second
#    39  a clock, once a second
#    50  a progress bar filling, 600 points wide
#    59  a striped bar, all of it moving
#    68  a line of text sliding sideways
#    77  the pointer over a list, each row lit as it passes
#    86  text selected by a drag, then let go
#    96  a small chart redrawn five times a second
#   105  a picture of 96 by 54 points that moves, as a thumbnail's video
#   114  one of 320 by 180, as a small video
#   123  a log a line is added to five times a second, scrolling once full
#   134  a section opened and closed, what is under it pushed down and back
#   144  a menu opened, its items passed over, closed
#   153  a notice sliding in and out
#   165  a slider dragged
#   172  a button passed over, its colour fading in and out
#   179  the page nudged by the wheel a notch at a time, and back
#   189  a terminal opened over the page: a command typed, a progress line
#        rewritten ten times a second, a table redrawn once a second, the
#        window nudged aside four points a step and back, and closed
#   220  the clock, the spinner and typing at once
#   231  still until the end
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
[[ -n "${CAPTURE_KEYS:-}" ]] || { log "small: the pointer comes through CAPTURE_KEYS, which is not set"; exit 1; }
((W >= 1280 && H >= 820)) || { log "small: the page is 1280 by 820 points, and the desktop ${W}x${H}"; exit 1; }
mkdir -p "$MATERIAL"

cat >"$MATERIAL/small.html" <<'END'
<!doctype html><html><head><meta charset="utf-8"><title>small</title><style>
html{scroll-behavior:auto}
body{margin:0;height:2400px;font-family:"DejaVu Sans",sans-serif;font-size:15px;color:#222;background:#fff;overflow-x:hidden}
.at{position:absolute}
#field{left:40px;top:40px;width:400px;height:36px;font:17px "DejaVu Sans",sans-serif;box-sizing:border-box;padding:0 8px;border:1px solid #888;border-radius:4px}
.spin{border-radius:50%;border-style:solid;border-color:#3b82f6 #dbe4f0 #dbe4f0 #dbe4f0;animation:turn .8s linear infinite;animation-play-state:paused}
.on{animation-play-state:running !important}
@keyframes turn{to{transform:rotate(360deg)}}
#spin32{left:500px;top:40px;width:24px;height:24px;border-width:4px}
#spin16{left:560px;top:50px;width:12px;height:12px;border-width:2px}
#loading{left:584px;top:46px}
#stopwatch{left:700px;top:36px;font:28px "DejaVu Sans Mono",monospace}
#clock{left:1000px;top:36px;font:28px "DejaVu Sans Mono",monospace}
.bar{left:40px;width:600px;height:16px;background:#e5e7eb;border-radius:8px;overflow:hidden}
#bar{top:110px}#fill{height:100%;width:0;background:#22c55e}
#percent{left:650px;top:108px}
#stripes{top:150px;background:repeating-linear-gradient(45deg,#60a5fa 0 12px,#bfdbfe 12px 24px);background-size:34px 100%;animation:slide .6s linear infinite;animation-play-state:paused}
@keyframes slide{to{background-position:34px 0}}
#ticker{left:720px;top:110px;width:520px;height:28px;overflow:hidden;white-space:nowrap;border:1px solid #ccc;line-height:28px}
#ticker span{display:inline-block;padding-left:100%;animation:run 12s linear infinite;animation-play-state:paused}
@keyframes run{to{transform:translateX(-100%)}}
#list{left:40px;top:200px;width:400px;margin:0;padding:0;list-style:none;border:1px solid #ccc}
#list li{height:27px;line-height:27px;padding:0 10px;border-bottom:1px solid #eee}
#list li:hover{background:#dbeafe}
#text{left:480px;top:200px;width:380px;height:280px;margin:0;font:15px/1.5 "DejaVu Serif",serif;overflow:hidden}
#chart{left:900px;top:200px;border:1px solid #ccc}
#thumb{left:900px;top:320px}
#movie{left:900px;top:390px}
#log{left:40px;top:500px;width:400px;height:180px;margin:0;overflow:hidden;font:12px/15px "DejaVu Sans Mono",monospace;background:#111827;color:#d1d5db;box-sizing:border-box;padding:4px}
#fold{left:480px;top:500px;width:380px}
#fold h3{margin:0;padding:6px 10px;background:#f3f4f6;font-size:15px}
#inside{height:0;overflow:hidden;transition:height .3s;background:#fef3c7}
#inside.open{height:70px}
#under{padding:6px 10px}
#menu{left:480px;top:630px}
#menu b{display:inline-block;padding:4px 12px;border:1px solid #888;border-radius:4px;font-weight:normal}
#items{display:none;position:absolute;left:0;top:30px;width:180px;margin:0;padding:4px 0;list-style:none;background:#fff;border:1px solid #888;box-shadow:0 4px 12px rgba(0,0,0,.25)}
#items.open{display:block}
#items li{padding:3px 12px}#items li:hover{background:#dbeafe}
#slider{left:680px;top:634px;width:180px}
#button{left:40px;top:690px;padding:5px 18px;border:1px solid #888;border-radius:4px;background:#f3f4f6;transition:background .3s,color .3s}
#button:hover{background:#2563eb;color:#fff}
#toast{left:940px;top:600px;width:280px;height:70px;box-sizing:border-box;padding:12px;background:#1f2937;color:#fff;border-radius:8px;opacity:0;transform:translateY(40px);transition:opacity .4s,transform .4s}
#toast.open{opacity:1;transform:none}
#below{left:40px;top:760px;width:1200px;font:15px/1.5 "DejaVu Serif",serif}
</style></head><body>
<input id="field" class="at" autocomplete="off" spellcheck="false">
<div id="spin32" class="at spin"></div>
<div id="spin16" class="at spin"></div><span id="loading" class="at">Loading</span>
<div id="stopwatch" class="at">00:00.0</div>
<div id="clock" class="at">00:00:00</div>
<div id="bar" class="at bar"><div id="fill"></div></div><span id="percent" class="at">0%</span>
<div id="stripes" class="at bar"></div>
<div id="ticker" class="at"><span>The quick brown fox jumps over the lazy dog &#8212; pack my box with five dozen liquor jugs &#8212; how vexingly quick daft zebras jump &#8212; sphinx of black quartz, judge my vow</span></div>
<ul id="list" class="at"></ul>
<p id="text" class="at"></p>
<canvas id="chart" class="at" width="300" height="100"></canvas>
<canvas id="thumb" class="at" width="96" height="54"></canvas>
<canvas id="movie" class="at" width="320" height="180"></canvas>
<pre id="log" class="at"></pre>
<div id="fold" class="at"><h3>Details</h3><div id="inside">A section that opens, 70 points of it, and pushes what is under it down.</div><div id="under">What is under the section: a line of text that moves with it.</div></div>
<div id="menu" class="at"><b>File</b><ul id="items"><li>New window</li><li>Open&#8230;</li><li>Save</li><li>Save as&#8230;</li><li>Print&#8230;</li><li>Close</li></ul></div>
<input id="slider" class="at" type="range" min="0" max="100" value="10">
<div id="button" class="at">Send</div>
<div id="toast" class="at"><b>Build finished</b><br>12 tests passed in 3.4 s</div>
<div id="below" class="at"></div>
<script>
const $ = id => document.getElementById(id);
for (let i = 1; i <= 10; i++) $('list').insertAdjacentHTML('beforeend', `<li>report-2026-${String(i).padStart(2, '0')}.pdf &#8212; ${i * 37 + 120} kB</li>`);
const lorem = 'Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. ';
$('text').textContent = lorem.repeat(3);
$('below').innerHTML = Array.from({length: 40}, (_, i) => `<p>${i + 1}. ${lorem}</p>`).join('');
const two = n => String(n).padStart(2, '0');
// A picture that moves as a video's does: a plasma, every pixel new a frame.
function plasma(canvas, t) {
  const c = canvas.getContext('2d'), w = canvas.width, h = canvas.height, image = c.createImageData(w, h), d = image.data;
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    const v = Math.sin(x / 23 + t * 2) + Math.sin(y / 17 - t * 3) + Math.sin((x + y) / 31 + t) + Math.sin(Math.hypot(x - w / 2, y - h / 2) / 13 - t * 4);
    const i = (y * w + x) * 4;
    d[i] = 128 + 127 * Math.sin(v * Math.PI / 2); d[i + 1] = 128 + 127 * Math.sin(v * Math.PI / 2 + 2); d[i + 2] = 128 + 127 * Math.sin(v * Math.PI / 2 + 4); d[i + 3] = 255;
  }
  c.putImageData(image, 0, 0);
}
plasma($('thumb'), 0); plasma($('movie'), 0);
const points = Array.from({length: 60}, (_, i) => 50 + 30 * Math.sin(i / 5));
function chart() {
  const c = $('chart').getContext('2d');
  c.fillStyle = '#fff'; c.fillRect(0, 0, 300, 100); c.strokeStyle = '#dc2626'; c.lineWidth = 2; c.beginPath();
  points.forEach((p, i) => i ? c.lineTo(i * 5, 100 - p) : c.moveTo(0, 100 - p)); c.stroke();
}
chart();
// Each part runs from its second to its end by the page's own clock, which
// starts at the key that is not typed into the field.
const every = (from, to, ms, f) => setTimeout(() => { const begun = performance.now(), id = setInterval(() => { const t = (performance.now() - begun) / 1000; if (t > to - from) clearInterval(id); else f(t); }, ms); }, from * 1000);
const frames = (from, to, f) => setTimeout(() => { const begun = performance.now(); const go = () => { const t = (performance.now() - begun) / 1000; if (t > to - from) return; f(t); requestAnimationFrame(go); }; go(); }, from * 1000);
const during = (from, to, el) => { setTimeout(() => el.classList.add('on', 'open'), from * 1000); setTimeout(() => el.classList.remove('on', 'open'), to * 1000); };
const clock = (from, to) => every(from, to, 1000, t => { const s = Math.floor(t) + 41400; $('clock').textContent = `${two(Math.floor(s / 3600))}:${two(Math.floor(s / 60) % 60)}:${two(s % 60)}`; });
let lines = 0;
function start() {
  during(12, 18, $('spin32'));
  during(21, 27, $('spin16'));
  every(21, 27, 400, t => { $('loading').textContent = 'Loading' + '.'.repeat(Math.floor(t / 0.4) % 4); });
  every(30, 36, 100, t => { $('stopwatch').textContent = `00:${two(Math.floor(t))}.${Math.floor(t * 10) % 10}`; });
  clock(39, 47);
  frames(50, 56, t => { const p = Math.min(100, t / 6 * 100); $('fill').style.width = p + '%'; $('percent').textContent = Math.round(p) + '%'; });
  during(59, 65, $('stripes'));
  during(68, 74, $('ticker').firstChild);
  every(96, 102, 200, t => { points.shift(); points.push(50 + 30 * Math.sin(t * 3) + 15 * Math.sin(t * 11)); chart(); });
  frames(105, 111, t => plasma($('thumb'), t));
  frames(114, 120, t => plasma($('movie'), t));
  every(123, 131, 200, t => { $('log').textContent += `${two(Math.floor(t))}.${Math.floor(t * 10) % 10} worker-${lines % 4} handled request ${1000 + lines * 7} in ${(lines * 13) % 90 + 4} ms\n`; lines++; $('log').scrollTop = $('log').scrollHeight; });
  during(134, 137, $('inside')); during(139, 141, $('inside'));
  during(153, 157, $('toast')); during(158, 162, $('toast'));
  clock(220, 228); during(220, 228, $('spin32'));
}
let started = false;
addEventListener('keydown', e => { if (!started && e.target !== $('field')) { started = true; e.preventDefault(); start(); } }, true);
$('menu').firstChild.addEventListener('click', () => $('items').classList.toggle('open'));
</script></body></html>
END

# The page as an application's window, which has no tab strip or address bar
# and, tiled, fills the desktop: its points are the desktop's. Not a kiosk's,
# which is fullscreen, and sway draws a fullscreen window over a floating
# one, the terminal's. `--test-type` keeps the bar that warns of
# `--no-sandbox` off the top of it, which a kiosk does not draw.
# What the terminal part runs: a progress line rewritten ten times a second,
# and a table redrawn once a second.
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

chromium --ozone-platform=wayland --no-sandbox --disable-gpu --disable-dev-shm-usage --no-first-run --password-store=basic --test-type --app="file://$MATERIAL/small.html" >>"$MATERIAL/chromium.log" 2>&1 &
await_window chrom
sleep 4

part() { log "small $(((($(date +%s%N) - T0)) / 1000000000))s: $*"; }

# Sleep until that many seconds, which may have a fraction, from the start.
at() {
	local due now
	due=$((T0 + $(awk -v s="$1" 'BEGIN { printf "%d", s * 1000000000 }')))
	now=$(date +%s%N)
	if ((due > now)); then
		sleep "$(awk -v n=$((due - now)) 'BEGIN { printf "%.3f", n / 1000000000 }')"
	fi
}

# The pointer from where it is to X,Y in steps of about a thirtieth of a
# second over SECONDS, its buttons held as given.
PX=640 PY=560
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

pointer $PX $PY
ready
T0=$(date +%s%N)
# The key that starts the page's clock, typed nowhere.
press g

at 3
part "text typed into a field"
click 240 58
type_slowly "the quick brown fox jumps over the lazy"
at 11
click 640 560

at 12; part "a spinner"
at 21; part "a small spinner and a word"
at 30; part "a stopwatch"
at 39; part "a clock"
at 50; part "a progress bar"
at 59; part "a striped bar"
at 68; part "a line of text sliding"

at 77
part "the pointer over a list"
glide 240 190 0.3
glide 240 480 2.5
glide 260 205 2.5
glide 640 560 0.4

at 86
part "text selected by a drag"
glide 484 212 0.3
pointer 484 212 1
glide 840 250 1.2 1
glide 700 400 1.8 1
glide 560 330 1 1
pointer 560 330 0
at 93
click 640 560

at 96; part "a small chart"
at 105; part "a thumbnail's video"
at 114; part "a small video"
at 123; part "a log"
at 134; part "a section opened and closed"

at 144
part "a menu"
for _ in 1 2; do
	click 500 643
	sleep 0.4
	glide 540 670 0.3
	glide 540 790 1.2
	sleep 0.3
	click 500 643
	glide 640 560 0.3
	sleep 0.5
done

at 153; part "a notice"

at 165
part "a slider dragged"
glide 706 644 0.3
pointer 706 644 1
glide 840 644 1.5 1
glide 740 644 1.5 1
pointer 740 644 0
glide 640 560 0.3

at 172
part "a button passed over"
for _ in 1 2 3 4; do
	glide 75 705 0.2
	sleep 0.4
	glide 200 640 0.2
	sleep 0.4
done
glide 640 560 0.3

at 179
part "the page nudged by the wheel"
# Buttons 5 and 4 are the wheel down and up, a press and release a notch.
for mask in 16 8; do
	for _ in $(seq 8); do
		pointer $PX $PY $mask
		pointer $PX $PY 0
		sleep 0.4
	done
done

at 189
part "a terminal over the page"
# What the terminal runs is a short name each, so that typing one is over
# well before it is to start and the one before it has ended: the progress
# line runs six seconds from about 196, the table five from about 205.
foot --app-id=small-term --working-directory="$MATERIAL" >/dev/null 2>&1 &
await_window small-term
swaymsg -q '[app_id="small-term"] floating enable, resize set 640 400, move position 300 160'
at 191
type_slowly $'ls -la /usr/share/fonts | head\n'
at 195
type_slowly $'./progress\n'
at 204
type_slowly $'./table\n'
at 211.5
for step in $(seq 1 10) $(seq 9 -1 0); do
	swaymsg -q "[app_id=\"small-term\"] move position $((300 + step * 4)) 160"
	sleep 0.1
done
at 217
swaymsg -q '[app_id="small-term"] kill'

at 220
part "the clock, the spinner and typing at once"
click 240 58
type_slowly " dog, and types on while a clock ticks and a spinner turns beside it"
at 229
click 640 560

at 231
part "still until the end"
until_end
swaymsg -q '[app_id="^chrom"] kill' || true
