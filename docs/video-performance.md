# The video's encoder: libx264 through its own API

Measured on 2026-10-29, when the video stream's quantizer came to walk frame
by frame ([The video encoding](architecture.md#the-video-encoding)) and
libx264 came to be reached through its own API rather than libavcodec's
wrapper, which fixes the quantizer when it opens. The question was what the
switch costs the processor, at the same quantizer; the hardware's encoder is
the GPU's, and its cost to the processor is the conversion and the copy onto
its surface, which did not change. Its preset and motion search came later
([The preset and the search](#the-preset-and-the-search)), and the
retired VP9 was measured against it as a software encoder
([Against the retired VP9](#against-the-retired-vp9)).

**How.** lab1 (8 vCPUs of an AMD EPYC Rome, 4 cores with 2 threads each,
16 GB), idle and alone, swap off, libx264 by `--libx264`, so the encoder had
6 threads. Each sample is 120 frames of `frame-samples` (the 2026-10-29
recut): browser, drag, terminal and video at 1920x1080, 2880x1800@2 and
3840x2160@2. Each run alternated the two builds on every sample, twice:
`motion-bench` for the CPU time a frame of `WLSV`, held at wlshare's dial
(90, QP 18), and of the tiles picture, held; and `walk-sim` at 1000 Mbit/s,
where the walk stays at the ceiling, for the wall time of each encode. The
builds are 0.0.107 (`9125621`), libavcodec's wrapper at a constant QP 18;
`a6ac7e1`, x264's API under CRF with every picture's QP forced; and the same
with a constant quantizer whose I/P and P/B ratios are 512, so that its
bounds take any forced quantizer, which was not kept. The figures are each
build's mean over the two rounds; 0.0.107's moved by up to 15% between the
two runs, so a build is compared with the 0.0.107 of its own run alone. The
runs, their logs and the script are in the artifacts'
`bench/x264-api-20261029/`.

**What it showed.** At the same quantizer, the same quality — the page's
mean squared error over the run within a tenth, the terminal's the most
off, 0.42 to 0.50 at 2880x1800, its first picture at QP 18 where 0.0.107's
constant quantizer coded an I picture three finer — and on every sample but
browser and drag about the same bytes. CRF took 4–23% more of
the processor a frame than 0.0.107, its wall time within a millisecond
either way, and coded a dragged window in a fifth of the bytes and the
browser in 8% fewer: it has x264 make the half-size picture its rate is
estimated on and search it for motion, which seeds the full search, and
`ultrafast`'s own search does not follow a window dragged far in a frame.
The constant quantizer took 0.0.107's processor time or less — 25% less at
most, 5% more at worst — with 0.0.107's bytes, a 4K drag's 14% more: what x264's API saves is
libavcodec's copy of every picture. CRF was kept: a 4K drag at 30 frames a
second is about 3 Mbit/s with it and 18 without, which the walk would pay
for in quality on a link short of them. That was `ultrafast`'s search; once
the search reached 128 pixels a drag no longer needed the half-size
picture's motion to start from, and the constant quantizer came back
([below](#the-encoders-settings-again)). The tiles' regions are the same
encoder; their cost moved with `WLSV`'s, less, since most of a tiles
picture's time is its PNGs.

Further down the walk the bytes fall and the processor time stays, or falls
where the picture moves all over: a frame of video at 4K took 43 ms at 90,
37 at 50 and 34 at 20.

### 0.0.107 against CRF

| Sample | CPU ms a frame | p95 | wall ms, median (p95) | KB a frame | Tiles' CPU ms a frame |
|---|---|---|---|---|---|
| `browser-1920x1080` | 10.7 → 12.2 (+15%) | 12.5 → 13.9 | 3.4 (3.9) → 4.0 (4.5) | 174.2 → 159.2 | 5.6 → 5.8 (+3%) |
| `browser-2880x1800@2` | 23.7 → 26.8 (+13%) | 27.2 → 30.4 | 8.3 (9.0) → 8.7 (10.0) | 354.5 → 315.9 | 15.6 → 16.1 (+3%) |
| `browser-3840x2160@2` | 34.1 → 40.1 (+17%) | 39.2 → 45.5 | 13.6 (15.2) → 13.0 (14.7) | 434.0 → 401.5 | 30.0 → 30.5 (+1%) |
| `drag-1920x1080` | 6.8 → 7.7 (+13%) | 7.6 → 8.4 | 1.9 (2.2) → 2.4 (2.8) | 21.6 → 4.1 | 1.8 → 1.8 (+1%) |
| `drag-2880x1800@2` | 16.6 → 17.2 (+4%) | 20.5 → 19.4 | 5.2 (6.2) → 5.7 (6.7) | 113.1 → 20.6 | 10.8 → 10.8 (+0%) |
| `drag-3840x2160@2` | 22.5 → 25.4 (+13%) | 26.1 → 27.2 | 9.5 (15.8) → 8.2 (9.7) | 74.8 → 12.0 | 7.5 → 7.5 (-0%) |
| `terminal-1920x1080` | 5.8 → 6.5 (+13%) | 6.8 → 7.6 | 1.3 (1.7) → 1.7 (2.1) | 5.0 → 4.9 | 0.3 → 0.3 (-2%) |
| `terminal-2880x1800@2` | 13.0 → 13.1 (+1%) | 16.0 → 15.7 | 3.1 (4.2) → 3.6 (4.4) | 11.9 → 11.3 | 1.1 → 1.1 (+2%) |
| `terminal-3840x2160@2` | 18.6 → 20.9 (+12%) | 21.9 → 23.6 | 6.7 (8.5) → 5.8 (7.3) | 14.5 → 14.3 | 1.4 → 1.4 (+1%) |
| `video-1920x1080` | 11.4 → 12.3 (+8%) | 12.7 → 14.7 | 3.2 (3.7) → 3.8 (4.6) | 82.6 → 82.4 | 12.4 → 12.8 (+3%) |
| `video-2880x1800@2` | 24.6 → 29.0 (+18%) | 29.4 → 35.1 | 9.1 (10.2) → 9.4 (10.6) | 179.4 → 178.8 | 29.5 → 31.9 (+8%) |
| `video-3840x2160@2` | 35.0 → 43.0 (+23%) | 43.9 → 50.6 | 14.8 (17.0) → 13.8 (16.5) | 202.1 → 201.1 | 39.3 → 44.3 (+13%) |

### 0.0.107 against constant QP

| Sample | CPU ms a frame | p95 | wall ms, median (p95) | KB a frame | Tiles' CPU ms a frame |
|---|---|---|---|---|---|
| `browser-1920x1080` | 12.6 → 11.6 (-8%) | 14.4 → 13.7 | 3.8 (4.5) → 3.4 (3.8) | 174.2 → 172.5 | 6.4 → 6.3 (-2%) |
| `browser-2880x1800@2` | 28.2 → 24.6 (-13%) | 32.8 → 29.8 | 10.0 (11.1) → 7.7 (8.9) | 354.5 → 355.9 | 17.3 → 16.7 (-4%) |
| `browser-3840x2160@2` | 35.9 → 35.2 (-2%) | 42.1 → 41.5 | 15.3 (16.6) → 11.2 (12.9) | 434.0 → 433.7 | 31.2 → 30.8 (-1%) |
| `drag-1920x1080` | 7.3 → 7.6 (+5%) | 8.6 → 8.7 | 2.4 (2.7) → 1.9 (2.3) | 21.6 → 21.5 | 1.8 → 1.8 (+0%) |
| `drag-2880x1800@2` | 18.0 → 16.6 (-8%) | 21.2 → 18.6 | 5.6 (6.8) → 5.0 (6.1) | 113.1 → 114.6 | 11.4 → 11.3 (-1%) |
| `drag-3840x2160@2` | 24.7 → 21.7 (-12%) | 28.2 → 24.7 | 10.2 (11.7) → 6.0 (7.8) | 74.8 → 85.3 | 7.7 → 7.6 (-1%) |
| `terminal-1920x1080` | 6.3 → 5.9 (-7%) | 7.0 → 6.2 | 1.4 (1.6) → 1.2 (1.4) | 5.0 → 4.9 | 0.4 → 0.4 (-3%) |
| `terminal-2880x1800@2` | 14.8 → 12.1 (-18%) | 16.9 → 14.2 | 3.3 (4.8) → 2.7 (3.2) | 11.9 → 11.7 | 1.1 → 1.1 (+1%) |
| `terminal-3840x2160@2` | 21.6 → 16.2 (-25%) | 23.4 → 19.6 | 7.2 (8.5) → 3.9 (4.5) | 14.5 → 14.3 | 1.4 → 1.4 (-2%) |
| `video-1920x1080` | 10.7 → 9.8 (-8%) | 12.5 → 11.7 | 3.3 (3.7) → 2.9 (3.3) | 82.6 → 82.6 | 11.6 → 11.2 (-4%) |
| `video-2880x1800@2` | 24.6 → 23.7 (-4%) | 28.9 → 28.6 | 9.1 (10.5) → 7.6 (8.8) | 179.4 → 179.4 | 28.8 → 28.0 (-3%) |
| `video-3840x2160@2` | 36.7 → 34.2 (-7%) | 43.5 → 40.9 | 14.6 (16.6) → 11.0 (12.8) | 202.1 → 202.1 | 41.0 → 39.1 (-5%) |

### CRF down the walk

| Sample | at 90 (QP 18) | at 50 (QP 28) | at 20 (QP 35) |
|---|---|---|---|
| `browser-1920x1080` | 12.2 ms, 159.2 KB | 12.2 ms, 107.3 KB | 12.6 ms, 71.0 KB |
| `browser-2880x1800@2` | 26.8 ms, 315.9 KB | 27.3 ms, 209.2 KB | 27.3 ms, 142.1 KB |
| `browser-3840x2160@2` | 40.1 ms, 401.5 KB | 41.7 ms, 273.1 KB | 41.5 ms, 185.7 KB |
| `drag-1920x1080` | 7.7 ms, 4.1 KB | 7.8 ms, 2.8 KB | 7.5 ms, 2.3 KB |
| `drag-2880x1800@2` | 17.2 ms, 20.6 KB | 18.1 ms, 12.9 KB | 17.7 ms, 9.7 KB |
| `drag-3840x2160@2` | 25.4 ms, 12.0 KB | 25.7 ms, 9.8 KB | 26.1 ms, 7.2 KB |
| `terminal-1920x1080` | 6.5 ms, 4.9 KB | 6.1 ms, 3.4 KB | 6.3 ms, 2.3 KB |
| `terminal-2880x1800@2` | 13.1 ms, 11.3 KB | 13.1 ms, 7.7 KB | 13.4 ms, 5.5 KB |
| `terminal-3840x2160@2` | 20.9 ms, 14.3 KB | 20.1 ms, 9.6 KB | 19.8 ms, 6.6 KB |
| `video-1920x1080` | 12.3 ms, 82.4 KB | 11.2 ms, 26.9 KB | 10.1 ms, 9.8 KB |
| `video-2880x1800@2` | 29.0 ms, 178.8 KB | 25.9 ms, 56.5 KB | 23.3 ms, 20.7 KB |
| `video-3840x2160@2` | 43.0 ms, 201.1 KB | 37.3 ms, 66.1 KB | 33.6 ms, 25.4 KB |

### The walk on the hardware

`walk-sim` over the `video-1280x800` capture on testdesktop2, its UHD 630's
low-power encoder coding every frame, each encode's time fixed at 5 ms
(`--encode 5`) so that the walk followed the link and the hardware's bytes
alone. On 50 Mbit/s the walk stood at the ceiling: 120 frames, every one at
90. On 3 Mbit/s it walked the running stream down, 90 to 70 to 50, the
quantizer moving between frames with no keyframe but the first: 70 frames,
26 at 90, 7 at 70 and 37 at 50.

## The preset and the search

Measured on 2026-11-03 on lab1 (8 vCPUs of an AMD EPYC Rome, the instance
rebuilt that day), idle and alone, swap off. The question was which of
libx264's presets and motion searches sends the fewest bytes for the
processor's time, with the daemon's rate control held: CRF, every picture's
QP forced. `ultrafast`'s diamond search reaches 16 pixels from its
prediction, and the half-size search CRF seeds it with follows a window
dragged but not a page scrolled.

**How.** Debian trixie's `x264` 0.164.3108 command line, 6 threads, sliced,
`zerolatency`, no B-frames, a keyframe only at the start, each picture's QP
forced by a qpfile under `--crf`, which is `i_qpplus1` under CRF. Each sample
is `frame-samples`' 120 frames (uncover's 48–49), each captured frame once,
converted to 4:2:0 as the daemon converts: terminal, uncover, drag, browser
and video at 1920x1080, 2880x1800@2 and 3840x2160@2. Every setting at QP 18,
22, 26, 30 and 35, twice, the lower CPU time kept; each stream decoded by
FFmpeg against its source, PSNR of Y, U and V weighted 6:1:1. Bytes at equal
PSNR is the Bjøntegaard rate over the range both settings' curves cover; CPU
is at the PSNR `ultrafast` reaches at QP 26. Flood is not among the samples.
The runs, their scripts and the tables are the artifacts'
`bench/codec420-20261103/`.

**What it showed.** Geometric means over the 15 samples, against `ultrafast`:

| Setting | Bytes at equal PSNR | screen (12) | video (3) | CPU |
|---|---:|---:|---:|---:|
| `ultrafast` | 1.00 | 1.00 | 1.00 | 1.00 |
| `ultrafast`, `umh` over 128 | 0.56 | 0.49 | 0.99 | 1.22 |
| `ultrafast`, `umh` over 256 | 0.49 | 0.41 | 0.99 | 1.39 |
| `superfast` | 0.71 | 0.75 | 0.56 | 1.57 |
| `superfast`, `umh` over 64 | 0.45 | 0.42 | 0.56 | 1.63 |
| `superfast`, `umh` over 128 | 0.40 | 0.36 | 0.56 | 1.67 |
| the same, adaptive quantization off | 0.40 | 0.38 | 0.51 | 1.68 |
| `superfast`, `umh` over 256 | 0.35 | 0.31 | 0.56 | 1.87 |
| `veryfast` | 0.61 | 0.63 | 0.53 | 2.19 |
| `veryfast`, `umh` over 256 | 0.31 | 0.27 | 0.53 | 3.19 |

The scroll is what the search is for: the browser at 1080p and QP 26's
quality is 119 KB a frame at `ultrafast` and 18.6 KB with the multi-hexagon
over 128, at 2880x1800@2 235 against 67, at 4K 302 against 135. On video
it finds nothing a short search does not, and costs 1.5 to 1.7 times
`ultrafast`'s processor time there. What halves video's bytes is the rest of
`superfast`: CABAC, the deblocking filter, the intra partitions and the 8×8
transform. `superfast` over 128 is where the bytes stop falling faster than
the processor time rises: over 256 the browser at 2× scale is sent in about half the bytes
(4K, 52 KB a frame against 95) and video costs a third more of the
processor (4K, 85 ms a frame against 64) for the same bytes. Wall time a 4K
frame, at 6 threads, is at most 9 ms on the terminal, 23 on the browser.

That is the daemon's libx264 since 2026-11-03: `superfast`, the multi-hexagon over 128,
adaptive quantization off, which `superfast` would leave on and which would
move a macroblock's quantizer off its picture's. Its stream is High profile.
`motion-bench --libx264` at wlshare's dial, on the desktop (bytes do not
depend on the processor), gave what the command line did, within 6%: `WLSV`
of the browser at 1080p 154.5 KB a frame before and 22.8 after, at
2880x1800@2 306.1 and 66.5, the drag 3.1 and 1.4, the video 84.3 and 68.2,
its mean squared error 2.86 and 2.39.

## Against the retired VP9

Measured on 2026-11-04 on lab2 (8 vCPUs of an AMD EPYC Rome, the instance
rebuilt the night before), idle and alone, swap off. The question was
whether VP9 4:2:0, coded as the daemon coded it before VP9 was retired
(`70730c7`), would serve as the software encoder, H.264 kept for the
hardware, on the hope that it costs no more of the processor for about the
same bytes.

**How.** Each encoder through its real code path in `motion-bench`, `WLSV`
alone (the tiles not coded): this libx264 by `--libx264`, and the
`wlshare-vp9` crate of `70730c7^` (libvpx 1.16, its AVX2 kernels chosen at
run time) by `--threads 6 --wlsv-chroma 420`: screen tuning, an active map
of the changed blocks, tile columns by width, 6 threads. Its loop filter,
which the crate turned off from 2048 wide for the 4:4:4 stream's software
decoder, was kept on at every width, and its speed, 7 or 9 by how much of
the picture a frame changes, was fixed at each of 5 to 9 in turn. Every
`frame-samples` scenario but still at 1920x1080, 2880x1800@2 and
3840x2160@2 (120 frames, walk whole) and the mixed session at 1920x1080,
held at dials 90, 75, 60, 42 and 20 (QP 18 to 35). Every frame decoded as
the page would, PSNR over B, G and R against the capture; bytes at equal
PSNR is the Bjøntegaard rate, CPU at the PSNR this libx264 reaches at dial
60, its processor time a frame. Flood is not counted. H.264 is the lower of
two passes; VP9's speeds one pass, which the result does not hang on. The
runs, their scripts and the tables are the artifacts'
`bench/h264-vp9-20261104/`.

**What it showed.** Geometric means over 22 samples, against this libx264:

| VP9 4:2:0 speed | Bytes at equal PSNR | screen (12) | video (3) | CPU |
|---:|---:|---:|---:|---:|
| 5 | 0.86 | 0.94 | 0.72 | 2.94 |
| 6 | 1.06 | 1.33 | 0.76 | 2.45 |
| 7 | 1.12 | 1.46 | 0.76 | 2.49 |
| 8 | 1.27 | 1.67 | 0.80 | 1.97 |
| 9 | 1.38 | 1.93 | 0.80 | 1.71 |

VP9 sends fewer bytes on video and a scroll at every speed (video 0.72–0.80,
the browser at speed 5 0.40–0.48), and more on a window dragged over a
still desktop at every speed (2–10 times at speeds 6 to 9, 1.3–2.9 at 5)
and on one uncovered at 2× scale (1.5–3.6 at speeds 6 to 9, 1.0–1.2 at 5). Its processor time is more at every speed on
every sample but the terminal: at speed 9, the cheapest, 1.5–2.1 times on
video and walk, at speed 5, the only one that saves bytes overall, 4–6
times on video (4K, 240 ms a frame against 56). Software stays libx264.

## The stream at 4:4:4

Measured on 2026-11-10 and 2026-11-11. The question was whether `WLSV` as
High 4:4:4 Predictive, a colour sample for every pixel, was good enough
to take the place of the walked tiles, every tile RemoteFX Progressive,
the picture for a link that is not always wide and text that is to stay
crisp ([Progressive tiles](tiles-performance.md#progressive-tiles)).
It did, in protocol 13 (0.0.125), and was taken out again in 0.0.126, the
progressive tiles back in its place: on a large desktop its encoder took
two to four times their share of a core, as the table below shows, and a
browser decodes it in software if at all. The encoder's changes made for
it ([What did not change](#what-did-not-change),
[the settings](#the-encoders-settings-again),
[the settle frame](#the-settle-frame)) stayed, for the 4:2:0 stream and the
tiles' regions. RDP's AVC444, the same chroma as two 4:2:0
pictures a client puts back together, was weighed first and left: it codes
on any 4:2:0 encoder and decodes on any decoder, but the page would have to
copy both pictures' samples out of the decoder to combine them, which costs
it more than the decode.

**How.** `walk-sim` and `tiles-walk-sim` over the window captures at
1920x1080, 2880x1800@2 and 3840x2160@2 and the mixed sessions at 1920x1080
and 2880x1800@2, at 5, 10 and 25 Mbit/s, libx264 by `--libx264`: on lab1
(four vCPUs of an AMD EPYC Rome, two cores of two threads, 8 GB, idle and
alone, swap off) with its own encode times, so libx264 had two threads, and
on the desktop with `--encode 20`, which is the same every run. Each update
is decoded as a page would and compared with the desktop it was coded from,
the PSNR over B, G and R: the mean over the updates while the desktop
moves, and the page once nothing more is owed. Flood is left out. The runs,
their scripts and every table are the artifacts' `bench/wlsv-444-20261110/`.

**What it showed.** lab1's walks, the progressive tiles then the 4:4:4
stream as 0.0.125 coded it:

| Capture | Link | Frames/s | dB moving | dB at rest | Mbit/s | Encoder |
|---|---|---|---|---|---|---|
| window-1920x1080 | 5 | 24.1 → 23.4 | 38.0 → 43.7 | 45.7 → 47.4 | 3.96 → 3.80 | 10 → 10% |
| window-1920x1080 | 10 | 25.6 → 25.2 | 41.1 → 45.2 | 45.7 → 45.5 | 5.75 → 6.69 | 11 → 11% |
| window-1920x1080 | 25 | 25.6 → 25.3 | 41.2 → 45.2 | 45.7 → 45.4 | 5.75 → 6.70 | 11 → 12% |
| window-2880x1800@2 | 5 | 18.7 → 19.7 | 37.2 → 44.7 | 48.4 → 49.3 | 4.04 → 3.76 | 14 → 28% |
| window-2880x1800@2 | 10 | 24.4 → 21.9 | 40.7 → 47.1 | 48.4 → 49.3 | 6.75 → 6.92 | 17 → 33% |
| window-2880x1800@2 | 25 | 24.0 → 23.5 | 43.8 → 48.4 | 48.4 → 50.0 | 8.98 → 10.19 | 18 → 38% |
| window-3840x2160@2 | 5 | 7.2 → 18.6 | 36.0 → 42.2 | 47.7 → 50.1 | 3.33 → 3.56 | 12 → 47% |
| window-3840x2160@2 | 10 | 20.6 → 20.2 | 36.1 → 45.5 | 47.7 → 50.2 | 7.91 → 6.77 | 23 → 54% |
| window-3840x2160@2 | 25 | 23.0 → 22.1 | 43.8 → 48.9 | 47.7 → 50.1 | 14.23 → 14.97 | 27 → 62% |
| mixed-1920x1080 | 5 | 12.3 → 14.8 | 38.4 → 44.4 | 47.0 → 51.8 | 2.06 → 1.90 | 7 → 11% |
| mixed-1920x1080 | 10 | 15.6 → 15.1 | 39.1 → 46.3 | 47.0 → 51.6 | 3.48 → 3.35 | 9 → 13% |
| mixed-1920x1080 | 25 | 16.0 → 15.9 | 41.3 → 47.1 | 47.0 → 50.9 | 5.36 → 5.39 | 11 → 14% |
| mixed-2880x1800@2 | 5 | 6.4 → 12.6 | 40.5 → 42.4 | 47.6 → 49.9 | 2.12 → 1.99 | 10 → 31% |
| mixed-2880x1800@2 | 10 | 11.3 → 13.6 | 38.8 → 44.8 | 47.6 → 49.9 | 4.17 → 3.65 | 17 → 35% |
| mixed-2880x1800@2 | 25 | 15.0 → 13.9 | 41.8 → 47.8 | 47.6 → 50.2 | 9.00 → 8.03 | 23 → 40% |

- **While the desktop moves** it is 1.9 to 9.3 dB closer, at 0.86 to 1.17
  times the bytes, and holds its frame rate on a slow link and a large
  desktop, where the progressive tiles' fell: the 4K window at 5 Mbit/s at
  18.6 frames a second against 7.2, the mixed session at 2880x1800 at 12.6
  against 6.4. With room on the link it is level or up to 2.5 frames a
  second under, an encode being 4 to 6 ms at the median at 1920x1080 and
  15 to 17 at 3840x2160 where a progressive update took 2 and 5 to 8.
- **At rest** it ends from 0.3 dB under to 4.8 over, the two walks it ends
  under the window at 1920x1080 with room on the link; with the fixed
  encode time, 0.2 under to 5.1 over. Before the settle frame it ended as
  the walk had left it, up to 9.2 dB under on a slow link.
- **The processor**: the encoder's share of a core is level with the
  progressive tiles' at 1920x1080 on the window, 1.3 to 1.6 times theirs on
  the mixed session there, about twice at 2880x1800, and an update 1.6 to
  2.4 times at 3840x2160. Most of it is the whole picture's whatever
  changed: a keystroke at 4K is 15 ms of the processor where the
  progressive tiles, which touched only what changed, took a tenth of one.
- **Coloured text**, on the flood, which is in no other figure: at the
  ceiling at 1920x1080, 4:2:0 shows 24.6 dB, 4:4:4 36.6, and the
  progressive tiles 25.9 moving and 34.8 settled.
- **4:2:0 beside it**, as first measured, before the settings below: 4:4:4
  took 1.3 to 1.7 times its share of a core for 0.87 to 1.20 times its
  bytes, from 1.1 dB under to 4.3 over while moving.

**A browser decodes it in software**, where it decodes it at all. Chrome 156
on Windows (an i5-8500T, six cores, a UHD 630), through WebCodecs, the
samples' streams at the ceiling, each frame drawn through WebGL2, a frame:

| Stream | 4:2:0, hardware | 4:2:0, software | 4:4:4, software |
|---|---:|---:|---:|
| browser 1920x1080 | 2.8 ms | 7.9 | 9.6 |
| video 1920x1080 | 2.7 | 14.4 | 14.5 |
| browser 2880x1800@2 | 6.6 | 18.3 | 23.2 |
| video 2880x1800@2 | 6.9 | 23.2 | 34.5 |
| browser 3840x2160@2 | 10.1 | 26.4 | 34.4 |
| video 3840x2160@2 | 9.8 | 28.9 | 45.0 |

Chrome offers no hardware decoder for it. Chrome 156 and 157, on Windows
and a Mac, decode it through WebCodecs with a colour for every pixel;
Safari 27.2 on a Mac and Safari on an iPhone (iOS 18.7) refuse it there,
though their video element plays it; Firefox 160 refuses both. So the
gateway asked its page's decoder, and offered the stream where the answer
was yes.

### What did not change

A profile of the 4:4:4 stream on the 4K terminal sample, where a frame is a
keystroke, had
about nine tenths of libx264's time on macroblocks that had not changed:
the half-pel planes of all three planes, and each macroblock visited,
transformed and quantized to find it could be skipped, then copied forward.
Telling x264 which macroblocks the reads left alone
(`X264_MBINFO_CONSTANT`) took the second of those away, at either chroma.
On the samples at 3840x2160, the 4:4:4 stream's processor time a frame:

| Sample | Before | Told | Told, and the settings below | Progressive tiles |
|---|---:|---:|---:|---:|
| terminal | 25.5 ms | 19.2 | 15.1 | 1.9 |
| drag | 30.1 | 24.7 | 17.7 | 4.9 |
| uncover | 33.9 | 29.7 | 23.0 | 7.2 |
| browser | 65.7 | 63.3 | 51.0 | 23.0 |
| video | 86.4 | 84.2 | 78.3 | 34.3 |

The picture is as it was, to 0.2 dB, as long as a macroblock is flagged
only once two frames have coded it as it stands. Flagged from the frame
after its change it stayed as its first coding left it: the terminal 2.3 to
5.4 dB coarser, for the same saving.

### The encoder's settings, again

The progressive tiles had been tuned over weeks and the 4:4:4 stream ran on
what the 4:2:0 one had been given, so libx264's settings were swept for it:
on lab1, the samples at the ceiling at 1920x1080 and 3840x2160, each
setting twice and the lower processor time kept, as geometric means over
terminal, drag, uncover, browser and video against the settings before it.

| Setting | CPU a frame | Bytes | dB moving |
|---|---:|---:|---:|
| weighted prediction off | 0.92 | 0.99 | +0.04 |
| and a constant quantizer, in CRF's place | 0.81 of that | 1.03 | −0.01 |
| no sub-pixel search | 1.04 | 1.03 | −0.05 |
| chroma three steps coarser than luma | 1.00 | 0.98 | −0.19 |
| chroma six coarser, x264's own for 4:4:4 | 1.02 | 0.97 | −0.54 |
| the deblocking filter off | 1.00 | 0.99 | +0.31 |
| no intra partitions | 0.99 | 1.05 | −0.25 |
| no 8×8 transform | 1.01 | 0.99 | +0.18 |
| `ultrafast` | 0.86 | 1.27 | −0.26 |
| four threads where two | 1.52 | 1.05 | −0.07 |

The first two were taken, for both chromas, the encoder being one: at
4:2:0 the same pair is 0.69 of the processor time for 0.98 of the bytes.
Every picture's quantizer is still forced; under the constant quantizer,
with nothing to weigh, x264 makes no half-size picture and estimates no
rate on one, and with the search over 128 pixels a dragged window is a
quarter more of very few bytes where under `ultrafast`'s search it was five
times. The filter was left on, its 0.3 dB being this dial's and not looked
at lower. Four threads on four processors code a 4K video frame in 29 ms
where two take 44, for half as much processor time again: the daemon's
rule, the machine's less two, was left as it is.

### The settle frame

`WLSV` sent nothing once the desktop stopped, so a page was left as the
walk had left it. It is now sent two frames more, half a second on, of no
damage, at the dial's fine end. At wlshare's own dial one frame ended 0.2
to 0.3 dB short of two and a third added nothing; at the fine end, QP 16
where a picture that moves is at 18, the page ends 0.7 to 1.9 dB closer
again, for a frame of 12 to 73 KB at 1920x1080. Over the mixed session at
2880x1800 at 5 Mbit/s they are 78 frames in five minutes, 2 MB, the largest
206 KB, and the changes shown are as late as they were: 245 ms at the 95th
percentile where 234. What a settle can add to a picture already at its
quantizer is little, x264 dropping a residual that small.
