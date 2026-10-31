# Tiles performance

What the tiles encoding ([The tiles encoding](architecture.md#the-tiles-encoding))
costs the processor and the link, held and walked, against the `WLSV` stream,
and what its rules were chosen by. `examples/motion-bench.rs` in the daemon's
crate measured it, over the kept captures (`frame-samples/`; see [Capturing
what the encoder is handed](architecture.md#capturing-what-the-encoder-is-handed)),
and `examples/tiles-walk-sim.rs` the walk on a slow link, over whole sessions.
Flood is in the figures for reference and is not a target.

The walked picture of every section before [Progressive
tiles](#progressive-tiles) is the one wlshare sent until 2026-10-28, zDesk's
Adaptive tiles: every still a lossy WebP at the dial, and a region that
moves an H.264 stream walked with it. Progressive tiles replaced it, and
its figures stay as what they were measured against; motion-bench's
`--walk-at`, which coded it, went with it.

`WLSV` in every figure here is the VP9 stream it was until 2026-10-29,
4:4:4 or 4:2:0, its dial walked on the link, which H.264 replaced, its
dial walked on the link as VP9's was ([The video encoding](architecture.md#the-video-encoding)); its
figures stay as what the tiles were measured against, and the dials and
`--quality` the runs name are the options the lists then had.

## The three pictures

Measured 2026-10-26 on an eight-core AMD EPYC (Milan) VM, idle and with no
swap, the regions by libx264 and `WLSV` on four threads, over the samples from
1280×800 to 4K at a ceiling of 90, summed over the scenarios, flood left out:

- **Held**, every still a PNG: 22% of `WLSV` 4:4:4's CPU for 3.2 times its
  bytes, from 1.9 times on a terminal to 21 on a drag, every capture ending
  lossless where `WLSV` ends at 42–50 dB.
- **Walked, standing at the ceiling**: 1.4 times held's CPU — 1.2 to 1.3 times
  where most of the desktop moves (video, a scroll, the mixed session), 2.4 to
  4.5 where most of it is stills, since a whole still costs a WebP about four
  times the fast PNG (99 ms against 23 at 1080p) — for 69% of held's bytes:
  the stills 256 MB of WebP where held sends 1,047 of PNG, the regions the
  same. It ends at 43–49 dB, text included.
- **Walked to 50 and to 20**: 1.2 and 1.1 times held's CPU, a quarter of
  `WLSV`'s, for 1.1 and 0.7 times `WLSV`'s bytes: under it on video (63 and
  30 MB against 117) and the mixed session, and still 3 to 11 times it on a
  scroll and a drag, where the stills are most of what is sent, before a
  scroll was a copy ([below](#copies)). The picture
  while it walks is, at 1080p, 38–42 dB at 50 and 34–37 at 20, and it settles
  at the ceiling's.

Over the mixed session at 1920×1080, 3,515 frames:

| Picture | CPU | Bytes | Ends at |
|---|---:|---:|---|
| `WLSV`, 4:4:4 | 99.0 s | 114 MB | 49 dB |
| `WLSV`, 4:2:0 | 78.4 s | 98 MB | 43 dB |
| Tiles, held | 21.7 s | 262 MB | lossless |
| Tiles, walked, at the ceiling | 26.2 s | 217 MB | 44 dB |
| Tiles, walked to 50 | 22.4 s | 93 MB | 44 dB |
| Tiles, walked to 20 | 20.9 s | 51 MB | 44 dB |

A lower ceiling takes most off `WLSV`: over both mixed sessions its bytes are
356 MB at 90, 93 at 50 and 31 at 20, where the held tiles' are 794, 431 and
313, their stills lossless whatever the dial, and the walked tiles' at the
ceiling 654, 279 and 151. Every capture settles within 13 s of its last frame,
and `check` passes on all 92.

## Copies

Measured 2026-10-27 on the same VM in the same way, the tiles alone
(`--tiles-only`) against the tiles above, once a scroll and a dragged window
are copies of what the client holds ([What moved is a
copy](architecture.md#the-tiles-encoding)) and a stream that ends sends what
it had not carried, held and walked at the ceiling and to 20, summed over the
scenarios, flood left out:

| Picture | CPU | Bytes | Ends at |
|---|---:|---:|---|
| Tiles, held | −8% | −15% | lossless |
| Tiles, walked, at the ceiling | −2% | −25% | 46.35 dB, as before |
| Tiles, walked to 20 | −3% | −35% | 46.35 dB, as before |

A page scrolled sends 65% fewer bytes held, 78% walked at the ceiling and 68%
walked to 20, for 41% less CPU held, where its regions no longer stream; a
dragged window 23–26% fewer, an uncovered one 12–15%, and the mixed session
10%, 18% and 34%. Video, a video in a window and the walk find no copy and
send the same, their CPU within the 4% two runs of the same build differ by
on this VM. Walked to 20, a scroll is 0.6–1.3 times `WLSV`'s bytes, where it
was 2.1–4.3, and a drag 0.4–10.7 times, where it was 4.0–12.7: from
2048×1536 up, few of a drag's frames are found as copies, and it still
streams. While a capture runs the held picture is 49.2 dB where it was 45.9,
a scrolled page's 76 dB. Every capture settles within 13 s of its last frame,
and `check` passes on all 82.

Over the mixed session at 1920×1080:

| Picture | CPU | Bytes | Ends at |
|---|---:|---:|---|
| Tiles, held | 20.0 s | 237 MB | lossless |
| Tiles, walked, at the ceiling | 25.6 s | 184 MB | 44 dB |
| Tiles, walked to 20 | 19.7 s | 36 MB | 44 dB |

A stream that ends sending what it had not carried, measured alone with the
copies off, changes the bytes by at most 1% (the walk's), and lifts the held
picture while a capture runs: a drag's from 48.5 to 52.4 dB, the walk's from
39.6 to 44.1, where the client held a stream's last frame until the settle
reached it.

The copies' rules were chosen on these samples. A probe found at more than 4
places was dropped at first: at 2× on a page of text nearly every one is, so
a scroll was missed, the tiles it missed streamed, and the streams restarted
as they grew, and at 32 a page scrolled sends half the bytes again. A copy
that draws no changed tile whole is not sent, which took the 9% that one
uncovering a window had cost back off. Walked, a copy owes the ceiling,
without which a dragged window ended up to 3.8 dB under the picture without
copies, a patchwork of the strips that were sent, for 0.4–3% of the bytes. A
scan of more than 16 times the tiles looked among found a copy in 6 of 18 in
the copying scenarios and none elsewhere; stopping there would take about 4%
of the CPU off video and the walk for 3.6% more bytes on a drag at 2×, and
the limit is 32.

## What the settle sends again

Out of the same walked runs, what is sent at the ceiling once the desktop has
been still is 32 MB of the 361 sent walked to 20, 8.9%, and 20 MB of the 1,323
walked at the ceiling, where it is what the streams carried and the copies
drew: the stills owed for being sent below the ceiling are at most the 12 MB
between the two, about 3% of what is sent walked to 20. A scrolled page's is
7%, a drag's 4%, the mixed session's 7%, an uncovered window's 10% and a
video in a window's 21%; only a terminal and the still desktop, which send
little else, go above 40%. A still is owed again only while what it drew is
on screen: a scrolled-away tile is sent again when it changes, at the dial,
and the debt is the last one. So each pause of 2 s or more on a slow link
sends one screen again at the ceiling, from 130 KB at 1280×800 to 520 KB at
3456×2168 for a scrolled page, while nothing else is sent. Owing less, by a
distance from the ceiling or a level the link decides, would buy at most that
3% for a picture at rest below the ceiling, and the picture at rest stays the
ceiling's.

Nor is a stream's frame at the ceiling worth leaving owed nothing on its
own: that was measured on lab1 on 2026-10-27, a walked list's
stream at the ceiling paying its tiles' debts, against main at `6ebb6cb`,
both in one session (`bench/settle-ceiling-20261027/` in the artifacts).
Walked at 90, summed over the scenarios, flood out, it sent 1,253 MB where
main sent 1,265, 1% less, for 2% less CPU, inside the 4% two runs differ
by, and the settle's tail fell from 58.5 s to 44.4. But the picture ends at
45.38 dB where it ended at 46.35: video at 42.74 where it was 44.57, the walk
41.91 where it was 44.51, a video in a window 45.12 where it was 45.99, while
drag (49.37 from 48.59) and uncover (48.48 from 47.69) end higher. At the same
dial the region's H.264 is not the WebP's picture: on a video the settle's
WebPs are about 2 dB better, for 1% of the bytes. Over the walks of
`tiles-walk-sim` nothing moved beyond what two runs of one build differ by,
and where the dial stays at 90, over 25 and 50 Mbit/s, the bytes moved 1% at
most. So the stream's tiles stay owed. That is not what RDP does with its
H.264 either: it never leaves a still region at the quantizer it moved at,
nor sends it again in another codec, but refines it inside the stream, to QP
33 and then 25 after 41, and leaves it there. A stream that refines itself
that way, below the ceiling where its frames are coarse, was on the roadmap
until the walked picture's streams went.

## The settle

Measured 2026-10-27 on the same VM in the same way, the tiles alone against
the build above, its settle 40 tiles a tick, once a tick sends the 320 tiles
owed longest at most, the oldest runs first until 256 KiB have gone, and 40
tiles while the walk is below the ceiling. From the last frame until nothing
is owed, the longest of the 82 captures and the mean:

| Picture | Longest | Mean | Longest tick | Bytes | CPU |
|---|---:|---:|---:|---:|---:|
| Tiles, held | 12.8 → 4.3 s | 2.5 → 1.0 s | 7 ms, as before | the same | +2.6% |
| Tiles, walked, at the ceiling | 12.8 → 2.3 s | 3.1 → 0.7 s | 7 → 42 ms | +0.3% | +1.9% |
| Tiles, walked to 20 | 13.2 s, as before | 4.9 s, as before | 10 ms | the same | +2.1% |

The CPU is within the 4% two runs of the same build differ by, and the
pictures end where they did; `check` passes on all 82, the longest settling
in 4.3 s where it took 12.7. A tick of 40 tiles sent 8–13 KB of WebP walked,
0.25–0.43 Mbit/s, and up to 130 KB of PNG held. The bytes bound a held tick,
whose PNGs are large and cost little to code, and the tile count a walked
one, whose WebPs are small and cost more: 160 tiles and 128 KiB took the
longest to 7.8 s held and 3.8 walked at the ceiling, in ticks of up to 32 ms,
and 320 and 256 KiB to 4.3 and 2.3, in ticks of up to 42 ms, under half the
99 ms a whole 1080p desktop costs a WebP. Below the ceiling the settle was as fast at
first, and sent 2.5–3.2% more bytes walked to 20, up to 5.9% on the mixed
session: paid sooner in a pause, more tiles were sent at the ceiling before
the next scroll changed them again. There the link is short of bytes, and
256 KiB a tick is 8 Mbit/s that the next update waits behind, which the
bench, with no link, does not show, so below the ceiling the settle keeps
40 tiles a tick and sends the same bytes.

## The walk's verdicts

What the walk ([The video encoding](architecture.md#the-video-encoding)) makes
of the tiles' updates on a slow link, by `examples/tiles-walk-sim.rs` in the
daemon's crate: `walk-sim`'s model of the browser's link and the gateway
(`examples/sim`) played over the tiles, each update coded as the session codes it, the regions
by libx264, and each of its rectangles a record of the gateway's own, its
body taking its share of the budget. Measured 2026-10-27 on the eight-core
AMD EPYC (Milan) VM, idle and with no swap, with this machine's encode
times, on the whole window and mixed captures at 1920×1080, 2880×1800@2 and
3840×2160@2, the dial's ceiling 90, over links of 5, 10, 25 and 50 Mbit/s
and one that falls from 50 to 5 for 8 s, each walked and held, against
`WLSV` on the same links.

Two of the session's rules kept the walk from judging the tiles as it judges
`WLSV`'s frames:

- **A stream's START was a keyframe.** An update that started a region's
  stream, or restarted it at a moved dial, held the walk's verdicts for 2 s.
  A window's video starts a stream about every half second, so over the
  window capture the walk never left the ceiling: at 4K over 10 Mbit/s, 23
  keyframes and no move, 95% of changes shown within 719 ms where `WLSV`
  took 211. A START is now an ordinary verdict, and only the stills of a
  picture the client holds nothing of are a keyframe: 2 or 3 a run, where
  there were 15 to 78.
- **A tick was a verdict.** In a pause, each update of the settle's tiles
  alone arrived on time, so the walk climbed through it (30, 33, 39, 51 over
  one pause of the mixed session) and the next burst walked it back down. Such
  an update is now no verdict, and the first of a pause starts the walk's run
  of clear updates again, as `WLSV`'s settle does.

The walked tiles, before and after, and `WLSV` walked on the same link:

| Capture | Link Mbit/s | Frames/s | Mean dial | Shown p95 ms | Moves | `WLSV`: frames/s · dial · p95 |
|---|---|---:|---:|---:|---:|---:|
| Window, 1920×1080 | 5 | 11.1 → 20.9 | 87 → 55 | 503 → 237 | 2 → 9 | 23.9 · 82 · 126 |
| Window, 2880×1800@2 | 5 | 10.8 → 18.9 | 65 → 47 | 842 → 323 | 5 → 4 | 19.7 · 71 · 174 |
| Window, 3840×2160@2 | 5 | 4.1 → 17.5 | 77 → 26 | 1,374 → 378 | 4, as before | 16.9 · 65 · 225 |
| Mixed, 1920×1080 | 5 | 13.0 → 14.4 | 64 → 54 | 517 → 305 | 60 → 49 | 14.3 · 75 · 158 |
| Mixed, 2880×1800@2 | 5 | 6.9 → 13.0 | 58 → 28 | 1,315 → 501 | 47 → 31 | 12.7 · 55 · 225 |
| Window, 1920×1080 | 10 | 21.6 → 23.4 | 90 → 83 | 178 → 146 | 0 → 4 | 25.6 · 90 · 62 |
| Window, 2880×1800@2 | 10 | 14.5 → 21.3 | 90 → 66 | 280 → 170 | 0 → 4 | 22.5 · 89 · 127 |
| Window, 3840×2160@2 | 10 | 8.2 → 18.4 | 90 → 55 | 719 → 235 | 0 → 3 | 18.5 · 82 · 190 |
| Mixed, 1920×1080 | 10 | 14.2 → 15.1 | 81 → 71 | 300 → 180 | 30 → 46 | 15.2 · 84 · 122 |
| Mixed, 2880×1800@2 | 10 | 10.2 → 13.9 | 67 → 46 | 623 → 291 | 31 → 49 | 12.9 · 70 · 187 |
| Window, 1920×1080 | 50 → 5 → 50 | 18.2 → 22.5 | 90 → 72 | 337 → 281 | 0 → 5 | 23.9 · 88 · 128 |
| Window, 2880×1800@2 | 50 → 5 → 50 | 15.7 → 21.4 | 73 → 60 | 497 → 387 | 5 → 3 | 21.9 · 80 · 201 |
| Window, 3840×2160@2 | 50 → 5 → 50 | 12.4 → 14.5 | 75 → 56 | 1,074 → 833 | 5 → 4 | 20.9 · 70 · 270 |
| Mixed, 1920×1080 | 50 → 5 → 50 | 15.8, as before | 90, as before | 54, as before | 0 → 4 | 15.6 · 90 · 53 |
| Mixed, 2880×1800@2 | 50 → 5 → 50 | 14.9, as before | 90, as before | 110 → 108 | 0, as before | 14.8 · 90 · 80 |

At 25 Mbit/s and over the walk moves the dial now and then, 0 to 54 times a
run where it moved 0 to 20, and every p95 is within a few milliseconds of
what it was or better, the most the window at 4K over 25, 155 ms to 115.
Held, the tiles are as they were, within what two runs of measured encode
times differ by: `WLSV`, the same code in both, moved up to 15% at p95
walked and 11% held, and the held tiles up to a fifth on a link they fill.
The longest wait of a run moves as much between two such runs as between
the rules: over the mixed session at 2880×1800@2 over 50 Mbit/s, where the
dial moved in neither, it was 721 ms before and 1.1 s after.

The tiles now paint about as many frames a second as `WLSV` over 5 and
10 Mbit/s, within three, at a lower dial, and over a steady 5 or 10 they
send under three fifths of the link (1.90 Mbit/s of 5 over the mixed session
at 2880×1800@2). But 95% of their changes take 1.2 to 3.1 times as long to be
shown as `WLSV`'s, the most on the falling link. Over the mixed session at
5 and 10 Mbit/s, 1.5 to 2.2 times, the waits are the bursts of stills that
change the picture outside the video, which the dial does not make smaller,
so the walk walks the regions down for a delay they did not cause.

That was tried on lab1 on 2026-10-28, the same captures and links walked,
with each update's stills and streams counted (`bench/verdicts-20261028/` in
the artifacts). The first descent of each mixed session, 90 to 20 or 30, is
judged on updates of 90 to 100% stills, 50 to 200 KB each, and the later
steps on the streams' frames. But an update with no stream frame made no
verdict, or one whose stills outweighed its streams' frames, helped
nothing: over the mixed session at 5 and 10 Mbit/s the p95 was the same or
worse (295 ms to 290 and 329 at 1920×1080 over 5, 257 to 275 and 287 at
2880×1800@2 over 10) and the mean dial lower, and elsewhere within what two
runs differ by. What makes the slowest changes slow is the bursts' own
bytes: at 2880×1800@2 two thirds of the slowest updates are mostly stills,
of about 100 KB at a dial of 20 to 37, and a WebP of text at 20 is still
62 to 67% of its bytes at 90 (browser and the mixed session, the updates'
own stills; 34 to 51% on video and the walk), where the regions' H.264 is
11 to 13%. Judged or not, the stills are what does not shrink.

Nor does a burst coded as a region's picture shrink. A burst of at least 32
tiles no stream carries, boxed as a region is, coded as one H.264 picture of
the box at the dial — a stream of one frame — and owed the ceiling after,
was measured the same way (`bench/bursts-20261028/` in the artifacts). It
sent more, not less: walked to 50 15% more, to 20 16%, and at 90 13%, the
browser's walk to 20 trading 32 MB of WebP for 57 MB of H.264, with the
settle the same; and over the mixed session the waits grew, at 2880×1800@2
from 456 ms at p95 to 634 over 5 Mbit/s and from 262 to 330 over 10. A
region's H.264 shrinks with the dial because its frames code what changed
since the last; a burst is one intra picture of text, which libx264 at
`ultrafast` codes worse than WebP at any dial, and its box codes the tiles in
it that did not change again.

## The regions' encoder

Measured 2026-10-21 on an Intel UHD 630's low-power encoder, in a container
with two cores and a flooded 1280×800 terminal, 20 s at quality 90: the H.264
regions sent 598 frames for 6 s of the daemon's CPU where VP9 sent 176 for
17 s, both read back whole by FFmpeg's own decoders (`examples/tiles-sink.rs`
in the daemon's crate is the client that was run, and keeps each stream for
that).

Measured 2026-10-22 on a six-core machine with no GPU, under headless labwc,
the same flooded terminal at 1280×800 and quality 90, 20 s, every encoder on
four threads: libx264 `ultrafast` sent 590 frames for 15–17 s of the daemon's
CPU where VP9 sent 395 for 33 s, about 27 ms of the processor a frame against
83, at about 1.8 times VP9's bytes a frame, 100–135 Mbit/s against 37;
libx264's `superfast` and `veryfast`, with CABAC and more search, cost 42 s
for about 535 frames, no less than VP9, which is why the preset is
`ultrafast`.

## What the rules were chosen by

The rules beyond zDesk's Motion were chosen when the regions were VP9 by
libvpx at speed 9: told its damage rather than coded whole, a stream's encoder
costs what moves; a first picture as PNG rather than a stream took 1–15% off
the CPU over a capture, and nine tenths off a terminal's; a band no stream
touches sent as its changed tiles rather than whole sent 3–15% fewer bytes
where the damage is loose.

## The cache

What a client-side cache of tiles would serve was counted before it was
built, by `examples/tile-recurrence.rs` in the daemon's crate, on lab1 on
2026-10-27: each capture played as the encoder reads it, Motion's churn rule
deciding what moves and a 4-connected group of fewer than 5 moving tiles
left to the stills, as the encoder leaves it, every other changed 64×64 tile
a still, and each still's pixels hashed and looked for among every still the
client was sent, kept by least recent use at 1024, 4096 and 16384 slots and
unbounded, flat tiles never kept and the cache started over with each whole
picture, as the encoder's is. A hit is *off screen* where the same pixels
stand in no tile of the picture the client holds, which only a cache serves,
and *on screen* where they do, which a copy could draw. Its per-tile PNG
total for the mixed session is an eighth under what the encoder sent in the
baseline, each still coded alone.

Over the mixed sessions, 225 s of desk work, the stills and what the cache
serves off screen, as a share of the stills' tiles and of their bytes coded as
a fast PNG and as a WebP at 90, each tile alone:

| Capture | Stills | PNG | 1024 slots | 4096 | 16384 | unbounded |
|---|---:|---:|---|---|---|---|
| mixed-1920x1080 | 33,774 tiles | 67 MB | 9% of tiles, 9% of PNG, 12% of WebP | 15%, 14%, 22% | 17%, 16%, 25% | 18%, 16%, 26% |
| mixed-2880x1800@2 | 80,663 | 129 MB | 5%, 5%, 7% | 14%, 15%, 24% | 20%, 21%, 35% | 22%, 23%, 37% |

A first count that kept flat tiles too found nearly none of the off-screen
hits among them, which is why a flat tile is not stored. On-screen hits of
tiles that are not flat are 6% of the mixed session's, a copy's domain. Of
the scenarios, uncover — a window moved aside and closed — serves 11% of its
still bytes at 1920×1080 and 37% at 2880x1800@2; browser, drag, terminal,
video, walk and the video in a window serve 0 to 6%. Unbounded adds little
over 16384 slots, and 1024 keeps little more than half the gain at 1920×1080
and a third at 2880x1800@2, so the cache is 4096 slots, 64 MiB of pixels on
the page.

Built, it was run on every sample on lab1 on 2026-10-27 at `69d2d3e`
(protocol 5), the tiles alone held at ceiling 90 and walked at 90 to 90, to
50 and to 20, read against the settle's run before it
(`bench/settle-20261027/final/` in the artifacts; this run is
`bench/tiles-cache-20261027/`). Summed over the scenarios, flood out:

| Pass | Bytes | CPU | Tiles drawn from the cache | At the end |
|---|---|---|---:|---|
| held at 90 | 2,174 MB → 2,085, −4% | 161 s → 156, −3% | 60,870 | lossless, 82 of 82, as before |
| walked at 90 to 90 | 1,327 → 1,265, −5% | 237 → 218, −8% | 66,941 | 46.35 dB, as before |
| walked to 20 | 361 → 323, −11% | 189 → 174, −8% | 65,407 | 46.35 dB, as before |

By scenario, held: uncover −20% of the bytes, browser −15%, drag −9%, mixed
−4%, the rest within 2%; walked to 20: uncover −22%, browser −17%, mixed
−15%, drag −7%, the video in a window −5%. Walked, the cache serves a little
more than held and the CPU falls further: a tile drawn from a slot is coded
neither coarse nor again at the ceiling. The held CPU is within the 4% two
runs of one build differ by on lab1. `check` passes 82 of 82: the held tiles
settle lossless and send nothing after.

Kept through a picture that starts over, as it is since protocol 7, it was
counted by `tile-recurrence --keep` on 2026-10-28 (a count, run on the
desktop machine rather than lab1): the cache not started over at a whole
picture, and of each whole picture after the first, the tiles that are not
flat that it holds. Those pictures are the captures' keyframes asked — at
10 s in video and window — which is what an output switched back to, a new
size or another dial sends; a repaint the gateway asks empties the cache
first. At 4096 slots, a third of each picture's tiles or more being flat
and never stored, their PNG bytes each tile alone:

| Capture | Tiles not flat | From the cache | PNG of the picture | From the cache |
|---|---:|---:|---:|---:|
| browser-1920x1080 | 310 | 310 | 0.52 MB | 0.47 MB, 89% |
| browser-2880x1800@2 | 688 | 688 | 1.11 MB | 0.93 MB, 84% |
| drag-1920x1080 | 127 | 127 | 0.30 MB | 0.19 MB, 63% |
| drag-2880x1800@2 | 428 | 428 | 0.85 MB | 0.60 MB, 71% |
| mixed-1920x1080 | 341 | 341 | 0.53 MB | 0.48 MB, 91% |
| mixed-2880x1800@2 | 723 | 723 | 1.19 MB | 1.03 MB, 86% |
| window-1920x1080 | 503 | 490 | 0.96 MB | 0.79 MB, 82% |
| window-2880x1800@2 | 950 | 937 | 1.93 MB | 1.43 MB, 74% |
| terminal-1920x1080 | 21 | 20 | 0.43 MB | 0.02 MB, 4% |
| video-1920x1080 | 381 | 15 | 2.20 MB | 0.02 MB, 1% |

A desk's picture again is a draw of a slot for every tile it shows that is
not flat, the rest flat tiles of a few bytes each when coded as a run; a
terminal is nearly all flat, and a video playing shows new frames, which no
cache serves. Unbounded serves no more than 4096 slots. The stills between
are served as before, within 0.1% on the mixed sessions, since a whole
picture stored its tiles in the cache that started over too; the video in a
window gains 20 tiles of 2,000 to 3,000 stills, what was shown before its
keyframe shown again after it. motion-bench's samples begin with their one
whole picture, so its figures are the same.

## The motion the stills carry

How much of what the stills send is motion — what a codec that sends a
tile's difference from what the client holds, as RemoteFX Progressive's
sub-band diffing does, would code instead — was counted by
`tile-recurrence` on lab1 on 2026-10-27, each still marked by why it is
sent: *fresh*, a tile that had not changed in the 0.8 s before; *warming*,
one that had, but in fewer than the 4 of 8 tenths that make it moving;
*small*, a moving tile in a group too small for a stream; and *stopped*, a
stream's tile owed its still. For every still sent over lossless pixels it also
coded the change from them, as a PNG of the per-channel difference modulo
256: what a held list's lossless difference would cost. It sends no copies,
so a scroll is stills here that the encoder sends as a copy. The run is
`bench/motion-stills-20261027/` in the artifacts.

| Capture | Stills | Warming | Small | Stopped | Warming's difference |
|---|---|---|---|---|---|
| mixed-1920x1080 | 67 MB as PNG, 19 MB as WebP | 60% of tiles, 65% of PNG, 62% of WebP | 2%, 2%, 2% | 1%, 0%, 1% | 48% of the pixels changed, a PNG 130% of the stills' |
| mixed-2880x1800@2 | 129, 37 | 58%, 66%, 63% | 1%, 1%, 1% | 1%, 0%, 0% | 44%, 140% |

So about two thirds of the mixed session's still bytes are tiles that
changed in the 0.8 s before, nearly all of them short of Motion's 4 tenths
rather than in a group too small for a stream. Of the scenarios' PNG bytes,
warming is 87 to 94% on walk, 81 to 85% on video, 70 to 80% on the video in a window, 45 to
74% on browser, drag and uncover, and 20 to 21% on terminal. A lossless
difference does not shrink them: on the mixed session it is 130 to 140% of
the still, and 117 to 127% for a fresh one. Only terminal's warming
stills (26 and 75%), the few small groups' and the video in a window's
(fresh 42 to 94%, warming 71 to 102%) come out about even or under. Most
likely a difference of text or of antialiased edges has more colours than
either tile, and its PNG loses the palette the still's had.
What makes RDP's diffing pay is lossy: the difference of quantized wavelet
coefficients, sent coarse first.

## Progressive tiles

The progressive picture ([On a walked list, every tile
progressive](architecture.md#the-tiles-encoding)) was measured from
2026-10-27 to 2026-10-28 as a replacement for the walked list's WebP and
H.264, which it then replaced: every tile RemoteFX Progressive, still or
moving, as a Windows host draws its desktop without H.264. The run is `bench/progressive-20261028/` in
the artifacts.

**What the host does.** zDesk's link probe (`examples/rdp_link_probe.rs`)
read a Windows host's progressive tiles over a LAN and behind relays of 8, 1,
0.5 and 0.25 Mbit/s, on a desktop of flat bands and a moving square. Its
first passes were at the stages of 50 and 75 and its upgrades to full, never
lower: its tables carry no stage below 50. A slower link got the same frame,
11 to 14 KB, fewer times a second — 27, 10, 5 and 2 frames — rather than a
coarser one. About a fifth of the first passes were at 75 on the LAN and
almost none at 500 kbit/s, and most were at full where the tile was flat.
Its stages are what the protocol's are built on.

**The stages.** `examples/progressive-stills.rs` coded the samples' stills
and moving tiles at each first stage. The host's stage of 50 costs about
what a row's run of WebP at 20 does (0.97 to 0.98 of it on the mixed session
and the window) and shows 4 to 5 dB less; its first pass and the upgrades
after it cost 1.15 to 1.29 times the full stage sent once, where a WebP at
the dial and one at the ceiling cost about 1.6. One more bit held back in
every band, the protocol's stage below 50, is a third fewer bytes and 3 to
4.6 dB coarser:

| Sample | Stage 50 | One bit coarser |
|---|---|---|
| mixed, stills | 9.50 MB, 30.5 dB | 6.32 MB, 25.9 dB |
| mixed, moving tiles | 137.6 MB, 34.2 dB | 93.6 MB, 30.3 dB |
| window, moving tiles | 1.92 MB, 31.9 dB | 1.18 MB, 28.9 dB |
| video, moving tiles | 5.15 MB, 33.6 dB | 3.36 MB, 30.5 dB |

The host's full stage ends near a WebP at 60; the protocol's is a bit finer
in every band above RemoteFX's least quantizer, 6, which settled the
walks' pages at 45.7 to 47.0 dB where the host's settled them at 41.3 to
42.1.

**Which stage, when.** Over the window and mixed sessions at 1920x1080,
with a fixed encode time of 5 ms, a first pass at 50 alone left the moving
picture at 38.6 to 38.7 dB at 10 Mbit/s, against 41.3 to 42.9 for the
adaptive tiles. At 75 while the walk stands at the ceiling, as the host's
are while its link has room, it is 40.1 and 41.2 dB. Below the ceiling the
walk's dial moved nothing, since 50 was the floor: 73 to 92% of the
mixed session's and the window's bytes at 5 Mbit/s went to tiles the
adaptive tiles would have streamed, at 1.11 to 1.61 times the H.264's bytes there, where
at 10 Mbit/s they cost 0.85 to 1.05 times it. A first pass one bit coarser
than 50 while the dial is under 70 took the mixed session at 5 Mbit/s from
11.2 to 11.9 frames a second, its changes shown at 94 ms where they were 106
at the median and 260 where they were 386 at p95, for 0.8 dB on the mean;
under 60 measured the same.

**The encoder's time.** On lab1, alone, with its own encode times over the
same four walks, a first pass coded first at its stage and then whole to
compare cost 4 to 6 times the adaptive tiles' encode. Trying the whole tile
only where the stage's pass is under 96 bytes or keeps at least two thirds
of the coefficients the full stage does (every tile sent whole on both
sessions, three missed in 34,000) took 21 to 25% off; the wavelet lifting
every column of a row at once, and each row's even and odd samples apart,
19 to 24% more; and an update's first passes coded ahead on all 8 cores, 2.1
to 2.4 times. The median update codes in 3.5 to 4 ms and the worst in 23 to
32, where the adaptive tiles take 1.7 to 3.1 and 49 to 56.

**The walk.** `tiles-walk-sim` with lab1's own encode times, the regions by
libx264, every capture and link of [the walk's
verdicts](#the-walks-verdicts) walked: the adaptive tiles, then the
progressive ones.

| Capture | Link | fps | p50 ms | p95 ms | Mbit/s | dB mean | dB end |
|---|---|---|---|---|---|---|---|
| window-1920x1080 | 5 | 20.7 → 22.9 | 65 → 75 | 222 → 128 | 3.16 → 3.87 | 41.2 → 38.5 | 44.6 → 45.7 |
| window-1920x1080 | 10 | 23.4 → 25.1 | 54 → 52 | 129 → 63 | 5.79 → 5.73 | 43.8 → 41.2 | 44.6 → 45.7 |
| window-1920x1080 | 25 | 24.4 → 25.3 | 39 → 38 | 58 → 41 | 7.53 → 5.75 | 44.7 → 41.2 | 44.6 → 45.7 |
| window-1920x1080 | 50→5→50 | 22.1 → 24.2 | 35 → 35 | 277 → 120 | 4.77 → 4.95 | 42.5 → 39.5 | 44.6 → 45.7 |
| window-2880x1800@2 | 5 | 20.2 → 17.3 | 49 → 112 | 265 → 189 | 2.38 → 3.79 | 39.7 → 37.6 | 45.2 → 48.4 |
| window-2880x1800@2 | 10 | 21.4 → 22.8 | 65 → 62 | 159 → 96 | 6.27 → 6.32 | 43.4 → 40.7 | 45.2 → 48.4 |
| window-2880x1800@2 | 25 | 23.0 → 23.5 | 48 → 47 | 79 → 55 | 11.56 → 8.96 | 45.1 → 43.8 | 45.2 → 48.4 |
| window-3840x2160@2 | 5 | 17.5 → 7.2 | 63 → 192 | 369 → 327 | 2.56 → 2.48 | 38.8 → 34.9 | 47.0 → 47.7 |
| window-3840x2160@2 | 10 | 18.5 → 17.5 | 69 → 95 | 229 → 169 | 5.52 → 6.30 | 42.5 → 36.4 | 47.0 → 47.7 |
| window-3840x2160@2 | 25 | 21.1 → 22.6 | 51 → 62 | 122 → 78 | 10.89 → 14.25 | 45.0 → 43.8 | 47.0 → 47.7 |
| mixed-1920x1080 | 5 | 13.9 → 12.3 | 55 → 87 | 327 → 269 | 1.86 → 2.15 | 38.8 → 37.6 | 44.0 → 47.0 |
| mixed-1920x1080 | 10 | 15.1 → 15.4 | 54 → 54 | 181 → 147 | 3.45 → 3.52 | 40.9 → 38.8 | 44.0 → 47.0 |
| mixed-1920x1080 | 25 | 15.7 → 16.0 | 42 → 40 | 81 → 85 | 6.05 → 5.53 | 43.1 → 41.2 | 44.1 → 47.0 |
| mixed-2880x1800@2 | 5 | 12.9 → 6.9 | 59 → 250 | 469 → 577 | 1.68 → 2.23 | 38.9 → 37.1 | 46.4 → 47.6 |
| mixed-2880x1800@2 | 10 | 14.0 → 11.0 | 63 → 125 | 290 → 303 | 3.51 → 3.99 | 40.9 → 38.2 | 46.4 → 47.6 |
| mixed-2880x1800@2 | 25 | 14.4 → 14.5 | 64 → 72 | 156 → 165 | 8.52 → 8.57 | 45.5 → 41.5 | 46.4 → 47.6 |

At 50 Mbit/s the frame rates and the pictures are as at 25, and the windows send 17 to 24% fewer
bytes. From 10 Mbit/s up, but for the mixed session at 2880x1800 at 10,
and at 1920x1080 at 5, the progressive tiles keep the frame rate: never
more than 1.6 frames a second below it, most often above, by up to 2.2,
and they mostly shorten the p95, for about the same bytes: fewer on the
smaller windows at 25, more on the 4K one. The moving picture is 1.2 to
4.0 dB coarser, 6.1 on the 4K window at 10, and the settled one 0.7 to 3.2
dB finer. On the large desktops at 5 Mbit/s, and the mixed session at
2880x1800 at 10, 3.0 frames a second behind, they keep up far less:
the 4K window at 7.2 frames a second where H.264 has 17.5, the mixed
session at 6.9 where it has 12.9, their changes shown at the median three
to four times later. There a moving region is H.264's to carry: a wavelet tile
costs more bytes than a P-frame at the same quality, and a slow link takes
it as fewer frames, as the host's does. Its first passes there are already
nearly all at the stage below 50, the dial walked to 20; one that held back
the finest wavelet level too, a tile at half resolution until its upgrades,
cost the samples' moving tiles 1 to 9% fewer bytes than that stage for 0.1
to 2.6 dB, since the stage below 50 leaves those bands nearly empty
already.

**The encoder's processor time, again.** A live session of a video in a
window, and one scrolling a browser, then showed wlshare's processor at least
half again a Windows host's, the gateway's and the browser's on par. On lab1
(rebuilt on 2026-10-28, eight cores of EPYC Rome where the figures above are
Milan's), alone, with its own encode times, the regions by libx264 and a link
of 100 Mbit/s that holds nothing back, `tiles-walk-sim` read the process's
processor time around each update's read and encode, as a share of one core:
the whole session's, and its busiest second's. The runs are
`bench/progressive-cpu-20261028/` in the artifacts. Six changes took a third to a
half off it. A tile's 60 KB of state is no longer copied once or twice for
every tile coded. The first passes are coded on a pool kept for the process
and shared by every session, not threads made for each update. The bits are
written 64 at a time, and a run of zeros skipped 8 at a time. A whole tile's
colour conversion and the coefficients' loop are written as loops the
compiler makes vectors of. And the cache's key is xxh3, not SipHash. None of
the six moves a byte: `--encode 20` at 10 Mbit/s over the window and the
mixed session at 1920x1080 sent the same updates, passes and bytes before
and after. The seventh does: on a walked list, a copy from a settled source
is settled (see [The tiles encoding](architecture.md#the-tiles-encoding)),
so a page scrolled after its upgrades goes again only where the scroll
uncovered it. Before the six, then after all seven (`ddc147b`):

| Capture | Session | Busiest second | Median update |
|---|---|---|---|
| browser-1920x1080 | 28.6 → 17.6% | 48 → 30% | 6.8 → 4.8 ms |
| browser-2880x1800@2 | 63.4 → 41.4% | 128 → 80% | 16.3 → 11.7 ms |
| window-1920x1080 | 45.7 → 25.1% | 65 → 40% | 4.3 → 2.3 ms |
| window-2880x1800@2 | 65.4 → 37.9% | 96 → 60% | 6.7 → 3.8 ms |
| mixed-1920x1080 | 39.2 → 22.0% | 171 → 101% | 5.1 → 2.7 ms |

The frame rates, the bytes and the pictures before and after are within a
run's noise, as measured encode times leave them.
The copy rule's share is the busiest scrolling second at 2880x1800 from 88
to 80% and the mixed session's from 107 to 101; on the windows it moves
nothing beyond a run's noise, 1 to 2 points. On a slow link it
rarely applies, since a page is seldom settled before it is scrolled
again: at 10 Mbit/s with `--encode 20` it sent no byte differently on
the browser captures and 0.01 and 0.2% fewer on the mixed sessions, the
pictures within 0.01 dB. These are the encoder's alone, without the
screen's capture a live session adds. Of what it spends scrolling at
2880x1800, before the copy rule, coding the tiles was 63%, finding what
moved and copying it 30%, and the rest of the update 7%. Of the coding,
the coefficients' loop of a first pass was a third, the wavelet 18%, the
coding of runs and bits 15% and the colour conversion 11%.

**The scroll.** With those and the capture at 30 frames a second
(0.0.104), a live session scrolling a news page back and forth fast still
cost about as much as before. It was played again on lab1, alone: headless
sway 1.11 at 2880x1800@2 and 1920x1080, Chromium 156 fullscreen on Google
News, the daemon built from the branch, `tiles-sink` a walked client
turning the wheel 15 clicks one way and 15 back, 20 ms apart, for 20 s, and
the daemon's processor read each second; Chromium drawing in software holds
it to 18 to 20 updates a second. Such a scroll moves the page 500 to 850
pixels between updates at 2880x1800, so every update brings half a screen
the client never held, about 215 tiles that no copy or cache can draw. Two
faults made it cost more. The search for copies left out a tile that changed
in place twice in 0.8 s, and a scroll the search missed changes every tile
in place: two misses, and no copy was found for the rest of the scroll, the
page coded whole each update, half again as many tiles or more. A tile
that churns is now looked among while a copy was found within those 0.8 s.
And the scan along every fourth row of the old picture finds a probe only
where its row lands on one, so a fast scroll, whose new rows hold most of
the rest, was left with fewer than two to agree on an offset; each offset
found is now checked against every probe, and the rows stepped over are
scanned where the first scan found nothing. Of the misses that left, about 1
in 13 updates, none is found by any probe at any offset. The coder took 87
µs a tile, on one core, over 60 frames of the browser capture at 2880x1800
at the stage of 75, and takes 41.5, the same bytes: the signs in a loop of
their own that SSE2 does in eight lanes (LLVM's `signum` it does a lane at
a time), the trailing zeros found sixteen at a time, a row's even and odd
samples split without a branch, the colour conversion without the clamps
eight bits never reach, a tile's buffers kept for the next rather than
allocated, the whole coder compiled a second time for AVX2 and chosen at
run time, and the entropy coder's fields put together into one write of up
to 32 bits. A rule that held the update after one that copied and coded
three rows of tiles across the picture, to halve a fast scroll's frames,
was tried and dropped: a scroll's coding is the page it uncovers, however
many frames it is shown in, and the rule saved 6% of the processor at
2880x1800 for 28% fewer frames. The runs are `bench/scroll-cpu-20261028/`
in the artifacts. The daemon's processor while scrolling, main (0.0.104)
then the branch, two runs each:

| Desktop | Processor | Tiles sent |
|---|---|---|
| 2880x1800@2 | 79–106% → 58–59% | 101–128k → 78–81k |
| 1920x1080 | 32% → 24–25% | 32k → 32–33k |

`tiles-walk-sim` on lab1 with its own encode times and a link of 100
Mbit/s, the capture at 30 Hz in both, main then the branch: the frame rates
and the pictures as they were, and the scrolls' bytes 10 to 16% fewer for
the copies the search no longer misses.

| Capture | Session | Busiest second | Mbit/s |
|---|---|---|---|
| browser-1920x1080 | 17.3 → 12.5% | 31 → 18% | 3.40 → 3.05 |
| browser-2880x1800@2 | 42.4 → 31.0% | 83 → 53% | 7.46 → 6.29 |
| window-1920x1080 | 26.0 → 15.3% | 38 → 24% | 5.76 → 5.77 |
| window-2880x1800@2 | 37.2 → 25.8% | 53 → 40% | 9.01 → 9.01 |
| mixed-1920x1080 | 23.2 → 15.9% | 110 → 84% | 5.55 → 5.38 |

**In the browser.** zDesk's end-to-end test reads a progressive session's
tiles from wlshare in a container, through the gateway, with the page's
decoder, against the lossless picture of the tiles with motion; headless
Chromium drew a test card of bars, gradients and noise from 505
progressive tiles without asking for the picture again.

## Adapting to the link

What the progressive picture does when the link changes under it, and what
it costs the processor, measured on 2026-10-29 and 30 and changed by what
was found. The samples were recut that day: browser, drag, video and window
had begun with two whole pictures and 1.4 to 1.6 s with nothing moving,
which every figure above counts, so main (0.0.108) was walked again on the
recut samples first, and every comparison here is against that. lab1 (eight
cores of EPYC Rome), alone, with its own encode times: `tiles-walk-sim` over
every scenario at 1280x800, 1920x1080, 2880x1800@2 and 3840x2160@2 and the
mixed session at its two sizes, on links of 5, 10, 25 and 100 Mbit/s, 136
runs; changes that move no byte checked the same with `--encode 20`, which
is deterministic; and the encoder alone over the samples, the walk held at
a dial, three rounds, for its processor time. Flood is in every run, for
reference. The runs are `bench/progressive-opt-20261030/` in the artifacts.

**The settle.** Below the walk's ceiling a tick sent 40 tiles a quarter of a
second, however idle the link, so a desktop that stopped took as long to
settle as its tiles took at that pace: a 4K video on 10 Mbit/s stopped, and
21 s of 5 KB ticks followed on a link carrying 160 kbit/s. The settle is now
paced by the link ([It settles](architecture.md#the-tiles-encoding)): the
bytes a tick sends walk on how long the updates that carried one queued, and
on a still desktop the next tick goes at the fence. The upgrades of a tile no
longer wait half a second each, as an upgrade is no change, and the coarsest
go first, so the picture is refined all over a stage at a time. While the
desktop moves nothing changes: every update the same with `--encode 20`.
The settle, summed over each scenario's runs, main then the branch:

| Scenario | Settle | Mean PSNR, 4K at 5 Mbit/s |
|---|---|---|
| browser | 66.7 → 15.2 s | 32.9 → 32.0 dB |
| video | 124.7 → 20.9 s | 41.7 → 44.8 dB |
| walk | 110.6 → 11.8 s | 41.7 → 42.2 dB |
| window | 27.4 → 11.6 s | 40.5 → 41.1 dB |
| drag | 25.3 → 10.7 s | 38.1 → 39.9 dB |
| mixed, both sizes | 23.2 → 4.0 s | |

A tick between a typist's keys started the walk's verdicts over, as the
desktop stopping does, so a walk once on its floor never gathered the
second of clear updates a step up needs: the mixed session at 2880x1800 on
5 Mbit/s held the dial at 20 from 33 s to the end, through three minutes of
reading and typing on a link with room. Only a tick on a desktop still for
half a second settles the walk now; that session climbs back to 90 while it
types, 1.7 dB finer over the whole (38.3 → 40.0) for 9% more bytes.

A tick's bytes follow the queueing behind the fence, its round trip less
the link's own as the walk keeps it, and go back to the least whenever the
walk steps down: with `--encode 20` that settled the browser at 2880x1800 on
5 Mbit/s in 1.10 s where the budget it learned before took 0.98, and moved
nothing else.

**Drags.** The copy search dropped a run found at more than 32 places, so a
terminal's text, which is most of a terminal, left too few runs to agree on
where it moved, and a terminal dragged across a large desktop went as
stills. The offsets the last read's copies were at are now checked first
([What moved is a copy](architecture.md#the-tiles-encoding)), every run
found there counting. With `--encode 20`, a scroll, a terminal and the mixed
sessions sent the same bytes; on lab1:

| Capture | Link | fps | p50 ms | p95 ms | MB |
|---|---|---|---|---|---|
| drag-2880x1800@2 | 5 | 13.8 → 20.7 | 234 → 32 | 588 → 418 | 2.05 → 0.92 |
| drag-2880x1800@2 | 10 | 18.4 → 20.7 | 43 → 32 | 280 → 266 | 2.34 → 1.45 |
| drag-3840x2160@2 | 5 | 15.2 → 19.4 | 205 → 32 | 698 → 551 | 2.05 → 0.95 |
| drag-3840x2160@2 | 10 | 19.8 → 21.5 | 32 → 32 | 311 → 229 | 2.06 → 0.89 |

**The processor.** The encoder alone, the median of three, main then the
branch, the same bytes but the drag's:

| Sample | Dial 90 | Dial 40 |
|---|---|---|
| video-3840x2160@2 | 8.54 → 5.93 s | 8.29 → 5.76 s |
| video-1920x1080 | 2.01 → 1.69 s | |
| walk-1920x1080 | 3.14 → 2.62 s | |
| browser-2880x1800@2 | 2.99 → 2.60 s | 2.72 → 2.48 s |
| drag-2880x1800@2 | 1.28 → 0.77 s | |
| window-1920x1080 | 0.73 → 0.68 s | |
| terminal-1920x1080 | 0.19 → 0.15 s | |

Four changes, in order of what they took off:

- The coder's spare buffers were one pool of 256 under one lock. A 4K update
  codes 2,040 tiles into new buffers while the ones they replace are still
  held, so most went back to the allocator and came from it zeroed every
  update, and the eight coding threads queued on the lock. Each thread now
  keeps a stash, moved to and from a pool of two 4K updates' worth 32 at a
  time: a third off a 4K video, a sixth off a 1080p one and the walk.
- A first pass coded the full stage beside its own to compare wherever the
  full stage kept at most half as many coefficients again, which at 75 is
  nearly every tile: a scroll and a drag tried it for 99% of their tiles,
  and it won for 42 and 75%. It is now tried where the bits the stage holds
  back, in bytes, and the coefficients it holds back whole come to at most
  twice the margin the full stage is allowed: over nine samples at three
  dials, 15% of the tiles tried where it had been 26%, and 99.6% of those
  sent whole found.
- A walked list hashed each changed tile for the cache twice an update, and
  through xxh3's streaming hasher a row at a time; once now, its rows put
  together and hashed in one call.
- An update's first passes went to the pool a tile a task, so a few tiles
  woke every thread, each to spin idle after; four a task now, and fewer
  than four are coded as they are sent. The terminal took a fifth less.

In the walks, which code more of what moves as the encoder gets faster,
the encoder's processor time over each scenario's runs fell 15% on the
browser to 33% on the terminal, 21 to 22% on the video, the walk and the mixed
sessions, and its share of a core at 100 Mbit/s:

| Capture | Share of a core | Per update | fps |
|---|---|---|---|
| video-3840x2160@2 | 136 → 105% | 71.6 → 47.4 ms | 18.9 → 22.0 |
| mixed-2880x1800@2 | 45.2 → 32.0% | 29.2 → 20.5 ms | 15.5 → 15.6 |
| browser-2880x1800@2 | 37.3 → 34.4% | 24.1 → 20.3 ms | 15.3 → 16.7 |
| drag-2880x1800@2 | 23.5 → 14.7% | 10.1 → 6.1 ms | 23.2 → 24.1 |
| window-3840x2160@2 | 42.4 → 35.5% | 19.4 → 16.1 ms | 21.9 → 22.1 |
| mixed-1920x1080 | 17.9 → 15.2% | 11.1 → 9.4 ms | 16.0 → 16.1 |
| flood-1920x1080 | 51.7 → 46.5% | 56.1 → 47.9 ms | 9.2 → 9.7 |

The frame rates and what changed shown at 5 and 10 Mbit/s are otherwise
within a run's noise, which with measured encode times is a frame or two a
second and up to a hundred milliseconds on one run: the 4K window at 5
Mbit/s showed its changes at a median 440 ms where main's run showed them
at 308, and with `--encode 20` its every update while it plays is main's.

**Tried, and not kept.** A first pass at the coarsest stage while the last
update's fence was late, as the host's first passes go to 25 when its socket
backs up: over the samples it shortened a scroll's and a drag's latency at 5
Mbit/s and lengthened a 4K video's, coarse and fine passes alternating under
the walk, and over the mixed sessions moved nothing beyond 1 to 2%. The copy
search's rolling hash a rotating one, its chain a rotate and an xor where a
polynomial's is two multiplies: no gain, the scan's time being its loop and
its filter's lookups. Looking along each probe's own row and column before
the scan: a scroll 6% cheaper, a drag a fifth dearer and 2% more bytes.

## Lossless tiles

The lossless picture (experimental), every tile that changed a planar bitmap
whatever moves, against the held picture, every still a PNG and what moves
a region's H.264 stream, measured on 2026-10-31. lab1 (eight cores of EPYC
Rome, rebuilt that day), alone, the regions by libx264 as it has no GPU:
`motion-bench --tiles-only` and `--tiles-only --lossless` over every sample
at every size, and `check --lossless` over the still ones. Archived in the
artifacts as `bench/lossless-20261031/`.

Every frame of every capture was exact on the page, and every still capture
settled at once and sent nothing through 5 s of quiet and 10 redraws.
Summed over the sizes, flood for reference:

| Scenario | CPU, held → lossless | Bytes, held → lossless |
|---|---:|---:|
| browser | 12.1 → 12.6 s | 89 → 152 MB (1.7×) |
| drag | 4.3 → 4.3 s | 6.6 → 9.1 MB (1.4×) |
| terminal | 0.7 → 0.7 s | 9.9 → 28 MB (2.8×) |
| uncover | 2.2 → 2.2 s | 24 → 30 MB (1.3×) |
| video | 22.8 → 29.6 s | 324 → 4,648 MB (14×) |
| walk | 36.0 → 44.2 s | 578 → 6,487 MB (11×) |
| window | 6.7 → 6.0 s | 96 → 1,127 MB (12×) |
| mixed | 75.4 → 83.2 s | 660 → 14,192 MB (22×) |
| flood | 59.0 → 42.9 s | 2,598 → 5,364 MB (2.1×) |
| all but flood | 160 → 183 s (1.14×) | 1,786 → 26,673 MB (15×) |

- **The processor.** About what the held picture costs: the planar coder
  in place of the PNG's deflate and libx264's regions together, 1.14 times
  over the scenarios. Where a still is most of what is sent it is the same
  or a few percent more, and a window that plays is a tenth less than the
  regions' libx264. A desktop moving all over costs most: a 4K video
  48.3 ms a frame against 33.4 held, its 95th percentile 67.5 ms, about
  20 frames a second of one thread's coding. On a host whose GPU codes the
  held picture's regions, the held picture is the cheaper.
- **The link.** Where the desktop is still, or moves by copies, it is 1.3
  to 2.8 times the PNG's bytes: planar's runs over the rows' differences
  against deflate's matches, text the furthest behind. Where it moves, it
  is 11 to 22 times the held picture's, every moving tile sent exact at the
  frame rate rather than a stream's: at 30 frames a second a video at
  1920×1080 is 412 Mbit/s, at 4K 1.25 Gbit/s, a video in a 4K window
  415 Mbit/s, and the mixed session 149 Mbit/s at 1920×1080 and 355 at
  2880×1800 on the mean. On a link short of that the walk slows the
  updates, never what a tile shows.

At 1920×1080:

| Capture | Held: ms a frame, MB | Lossless: ms a frame, MB, Mbit/s at its frame rate |
|---|---:|---:|
| browser | 4.61, 4.0 | 4.86, 9.0, 11 |
| terminal | 0.25, 0.6 | 0.28, 1.7, 2 |
| drag | 1.24, 0.3 | 1.21, 0.4, 1 |
| window | 3.90, 7.2 | 2.90, 74, 138 |
| video | 9.94, 19.1 | 10.49, 246, 412 |
| walk | 10.56, 34.2 | 11.27, 368, 195 |
| mixed | 6.53, 221 | 6.32, 4,198, 149 |

A Windows host set to Lossless measured the same way: at 1920×1080 a video
in a 960×600 window came to 76 to 106 Mbit/s of planar, its bulk
compression taking a fifth off, against 5 Mbit/s from a stock host's
RemoteFX Progressive (zDesk's
[Lossless, by the host's policy](https://github.com/andrewtheguy/zdesk/blob/main/docs/rdp-client.md#lossless-by-the-hosts-policy)).
The picture is the host's, for a LAN, and costs what it costs there.
