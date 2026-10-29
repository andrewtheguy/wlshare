# The video's encoder: libx264 through its own API

Measured on 2026-10-29, when the video stream's quantizer came to walk frame
by frame ([The video encoding](architecture.md#the-video-encoding)) and
libx264 came to be reached through its own API rather than libavcodec's
wrapper, which fixes the quantizer when it opens. The question was what the
switch costs the processor, at the same quantizer; the hardware's encoder is
the GPU's, and its cost to the processor is the conversion and the copy onto
its surface, which did not change.

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
for in quality on a link short of them. The tiles' regions are the same
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
