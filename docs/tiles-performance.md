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
`--walk-at`, which coded it, went with it. motion-bench now codes the held
picture, or the lossless one with `--lossless`, and the progressive tiles
are measured by `tiles-walk-sim` on a link and by `progressive-bench` and
`progressive-stills` on the samples ([Progressive
tiles](#progressive-tiles)).

The held picture's stills in every section before [WebP stills and the
eased settle](#webp-stills-and-the-eased-settle) are the fast PNG they were
until 2026-11-11, and its settle the one that went at the pace from its
first tick; their figures stay as what they were measured at.

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

The held list's settle has since been paced to the link rather than sent as
fast as it takes it ([below](#the-settles-pace)); the walked list's is as
measured here.

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

## The settle's pace

Measured 2026-11-04 on the desktop machine (bytes alone, which hang on no
processor), `motion-bench --libx264 --frames` over the kept captures, the
tiles' bytes in buckets of 250 ms, a tick's at its own time and the ticks
between two frames spread over the gap between them, against main at
`0c71561`. On a held list the dial is always the ceiling, so
the settle sent 256 KiB a fence back to back once the desktop was quiet, at
whatever the link would take; and a change too large for a region yet went
as the desktop whole in PNG: a picture that starts over, and every frame of
a video until the churn said moving, 400 ms, five updates of 1.9 MB in half
a second on the 1080p video where its keyframe is 0.2 MB. The link saw the
stream at 12 Mbit/s and then 188 for a quarter of a second. What a user
asked for was a flat link over fewer bytes: an update carries at most 256
KiB of stills where nothing moved and a change past that is a region from
its first frame, and the settle keeps to the pace, the most the picture sent
over a second since something was last owed, never a tick on the heels of
the last ([architecture](architecture.md#the-tiles-encoding)). The stills
an update carries, which is also the most a tick sends, was 256 KiB at
first and is 128, below. The busiest quarter second of each capture, and
the stream's rate while it moved:

| Capture | Stream | Busiest 250 ms before | At 256 KiB | At 128 KiB |
|---|---:|---:|---:|---:|
| video-1920x1080 | 12.4 Mbit/s | 188 Mbit/s | 37 Mbit/s | 32 Mbit/s |
| video-2880x1800@2 | 27.3 | 183 | 60 | 56 |
| window-1920x1080 | 6.5 | 29 | 29 | 25 |
| window-2880x1800@2 | 9.8 | 41 | 37 | 22 |
| browser-1920x1080 | — | 11 | 11 | 15 |
| uncover-1920x1080 | — | 15 | 17 | 12 |
| still-1920x1080 | — | 8.5 | 8.5 | 15 |
| still-2880x1800@2 | — | 19 | 37 | 29 |
| still-3840x2160@2 | — | 28 | 48 | 40 |
| mixed-1920x1080 | — | 45 | 45 | 45 |

The video's bursts are gone: its first frame is a region, not five updates
of PNG, and the 1080p video sent 35 MB where it sent 57, its PNG 5.8 MB
where it was 29. What is left, 32 and 56, is a change growing under an
update's stills while the churn has yet to call it moving, up to the
budget of PNG a frame at 25 frames a second for a third of a second before
it is a region, beside the stream at 2880x1800@2; and the mixed session's
busiest quarter second is a scroll, 45 Mbit/s of copies and the strips they
uncover, which the rule leaves alone. The settle after the video is the same
0.84 MB in ticks apart where it was back to back. A page of text that
starts over is past an update's stills at every size now, and the still
captures' two keyframes a tenth of a second apart start over twice, the
second before the first's region has settled, so each pays the stills and
the keyframe again: 0.63 MB at 3840x2160@2 where the whole picture as PNG
was 0.79, twice, and 0.36 MB at 1080p where it was 0.24 and lossless at
once. A window uncovered on a page of text starts the same way, 12 where
it was 15. Those are a page of text's one-off costs, a keyframe at the dial
on top of the stills it then settles, where the user's choice is the flat
link.

The two constants were swept the same day, the same way, the rest held:
the stills an update carries, 256 KiB, against 128 and 512; and the floor
a tick keeps to, 32 KiB, against 16, 64 and 128. The floor changed nothing
these captures show: it binds where the pace is below what a tick of
32 KiB every 250 ms needs, 1 Mbit/s, and nothing here carries so little, so
it is a figure for a slow link, and a real one. The budget binds twice over,
as the PNG a frame may carry while a change grows and as the most a tick
sends. At 128 KiB against 256 the busiest quarter second fell on seven
captures of ten, the table's last two columns, rose on two and was the
mixed session's scroll on the one left, and the videos and the windows sent
less, 1.3 and 3.9 MB less on the 1080p video and the 2880x1800@2 window.
What rose is a page of text at 1080p, 236 kB of PNG, which is then past the
stills and starts as a region, 364 kB and 15 Mbit/s where it was 8.5 and lossless at
once, and the browser's restarts with it; 256 KiB was the least of the
three that kept a 1080p page of text one update of PNG. At 512 KiB the
videos' busiest quarter second rose to 81 and 85 Mbit/s, half a megabyte of
PNG a frame while a change grows and a settle's tick of 585 kB beside the
stream, and the 4K page of text started over in one update of 1 MB. The
budget is 128 KiB: the flatter link over the 1080p page of text lossless at
once, which was the user's choice.

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
for about 535 frames, no less than VP9. That kept the preset at `ultrafast`
until 2026-11-03, on a flood, which is a reference and not what a preset is
chosen by; on the scenarios it is chosen by, `superfast` with its motion
searched over 128 pixels sends about two fifths of `ultrafast`'s bytes for
about 1.7 times its processor time, and has been the preset since
2026-11-03 ([The preset and the search](video-performance.md#the-preset-and-the-search)).

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

At 16384 slots, four times the cache, it was counted on 2026-11-09 at
`9694da1` (on the desktop machine, four runs at once, so bytes and
`--encode` walks alone): the tree built with `CACHE_SLOTS` at 4096 and at
16384, then `motion-bench --tiles-only`, held and `--lossless`, over every
sample but flood and still, and `tiles-walk-sim --encode 20`, walked, over
the mixed captures at 10 and 5 Mbit/s (`bench/cache-16384-20261109/` in the
artifacts). Held, only the mixed sessions fill 4096 slots; lossless, the
browser's samples at 2x save 0.5 to 53 KB, and the video, walk, window and
mixed samples a few KB of hundreds of MB. Where nothing fills the cache, the
bytes are the same; every sample ends as many pixels off either way:

| Pass | 4096 slots | 16384 slots |
|---|---:|---:|
| held, mixed-1920x1080 | 171.9 MB | 171.1 MB, −0.5% |
| held, mixed-2880x1800@2 | 348.6 MB | 343.9 MB, −1.4% |
| held, every sample | 1,174.7 MB | 1,169.2 MB, −0.47% |
| lossless, every sample | 15,026.7 MB | 15,026.1 MB, −0.00% |

Walked, the bytes a larger cache saves go to frames, and nothing moves past
what two runs of one build differ by:

| Capture, link | Frames a second | Mbit/s | Mean PSNR |
|---|---|---|---|
| mixed-1920x1080, 10 Mbit/s | 14.90 → 14.91 | 3.48 → 3.46 | 39.57 → 39.57 dB |
| mixed-2880x1800@2, 10 Mbit/s | 10.93 → 10.90 | 4.17 → 4.12 | 39.32 → 39.38 dB |
| mixed-1920x1080, 5 Mbit/s | 11.36 → 11.37 | 2.07 → 2.05 | 38.47 → 38.46 dB |
| mixed-2880x1800@2, 5 Mbit/s | 6.32 → 6.49 | 2.16 → 2.19 | 40.24 → 39.86 dB |

The six points more of the mixed session's stills the larger cache serves at
2880x1800@2, counted above, are little of what the session sends beside its
regions, and a lossless session's bytes are nearly all where the desktop
moves, which no cache serves.

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

## A walked list's streams, not taken

Measured 2026-11-09 and not taken. Every tile of a walked list is
progressive, still or moving: a moving region is wavelet tiles, each from
nothing or as the difference of its coefficients, whose first passes on a
large desktop at 5 Mbit/s are already at the coarsest stage, so no stage
closes what that costs the frame rate (above). What was tried in its place
is the remedy the walked tiles had before progressive tiles: each region the
held list's rules lay out an H.264 stream whose frames are at the walk's
dial, a P-frame predicted from the frame before, and once nothing in it has
changed for half a second a frame more at the ceiling refining it in its own
stream, its tiles settled at that picture. It is kept on branch
`walked-regions`, which bumped the protocol to 11 for it. The run is
`bench/walked-regions-20261109/` in the artifacts: `tiles-walk-sim` over the
window and mixed captures at 5, 10 and 25 Mbit/s, main (0.0.122) then the
branch, the regions by libx264.

**Bytes and pictures.** On this machine, with `--encode 20`, which is
deterministic:

| Capture | Link | Frames/s | p50 ms | Mbit/s | dB mean | dB end |
|---|---|---|---|---|---|---|
| window-1920x1080 | 5 | 20.3 → 22.5 | 104 → 74 | 3.70 → 2.78 | 38.3 → 40.8 | 45.7 → 42.0 |
| window-1920x1080 | 10 | 24.8 → 23.8 | 73 → 72 | 5.65 → 5.36 | 41.2 → 42.7 | 45.7 → 42.0 |
| window-1920x1080 | 25 | 25.7 → 24.8 | 55 → 55 | 5.77 → 5.87 | 41.1 → 43.2 | 45.7 → 41.9 |
| window-2880x1800@2 | 5 | 17.6 → 18.7 | 122 → 107 | 4.00 → 3.56 | 37.7 → 44.2 | 48.4 → 45.4 |
| window-2880x1800@2 | 10 | 19.8 → 20.7 | 106 → 88 | 7.10 → 6.21 | 41.7 → 45.8 | 48.4 → 45.4 |
| window-2880x1800@2 | 25 | 23.9 → 23.5 | 61 → 61 | 8.99 → 9.10 | 43.8 → 46.6 | 48.4 → 45.4 |
| window-3840x2160@2 | 5 | 10.5 → 18.6 | 195 → 87 | 4.44 → 3.29 | 34.6 → 41.8 | 47.7 → 45.3 |
| window-3840x2160@2 | 10 | 17.6 → 17.5 | 123 → 118 | 7.74 → 6.84 | 36.5 → 44.4 | 47.7 → 45.3 |
| window-3840x2160@2 | 25 | 22.5 → 21.8 | 75 → 72 | 14.15 → 13.29 | 43.9 → 46.3 | 47.7 → 45.3 |
| mixed-1920x1080 | 5 | 11.4 → 13.6 | 113 → 95 | 2.07 → 2.05 | 38.5 → 40.1 | 47.0 → 47.0 |
| mixed-1920x1080 | 10 | 14.9 → 15.0 | 74 → 71 | 3.48 → 3.34 | 39.6 → 41.9 | 47.0 → 47.0 |
| mixed-1920x1080 | 25 | 15.9 → 15.9 | 56 → 56 | 5.26 → 4.85 | 41.3 → 42.7 | 47.0 → 47.1 |
| mixed-2880x1800@2 | 5 | 6.3 → 12.6 | 255 → 100 | 2.16 → 2.23 | 40.2 → 40.2 | 47.6 → 47.7 |
| mixed-2880x1800@2 | 10 | 10.9 → 13.8 | 127 → 96 | 4.17 → 4.13 | 39.3 → 41.8 | 47.6 → 47.6 |
| mixed-2880x1800@2 | 25 | 14.5 → 14.5 | 82 → 78 | 8.94 → 8.55 | 42.2 → 44.9 | 47.6 → 47.7 |

What moved went to the streams: on the 4K window at 5 Mbit/s the first
passes at the coarsest stage fell from 22,052 tiles (4.25 MB) to 1,916, and
the differences from 28,372 (3.71 MB) to 563; on the mixed session at
2880x1800 the coarsest first passes from 178,527 (33.3 MB) to 36,110.

**With lab1's encode times.** lab1 (four vCPUs of EPYC Rome, rebuilt that
day), alone, each run main's then the branch's; the encoder's processor
time as a share of one core:

| Capture | Link | Frames/s | p50 ms | p95 ms | Mbit/s | dB mean | dB end | Encoder |
|---|---|---|---|---|---|---|---|---|
| window-1920x1080 | 5 | 24.8 → 24.5 | 74 → 47 | 114 → 122 | 4.03 → 2.60 | 38.1 → 40.6 | 45.7 → 42.0 | 11 → 7% |
| window-1920x1080 | 10 | 25.6 → 24.6 | 51 → 50 | 62 → 94 | 5.75 → 5.46 | 41.1 → 42.7 | 45.7 → 42.2 | 11 → 9% |
| window-1920x1080 | 25 | 25.6 → 24.8 | 36 → 38 | 40 → 46 | 5.75 → 5.85 | 41.2 → 43.2 | 45.7 → 42.1 | 11 → 8% |
| window-2880x1800@2 | 5 | 19.2 → 21.4 | 111 → 65 | 173 → 189 | 4.21 → 3.29 | 37.7 → 43.5 | 48.4 → 45.5 | 14 → 12% |
| window-2880x1800@2 | 10 | 24.2 → 22.9 | 59 → 62 | 114 → 125 | 6.78 → 6.49 | 40.6 → 45.7 | 48.4 → 45.4 | 17 → 16% |
| window-2880x1800@2 | 25 | 24.0 → 23.6 | 44 → 46 | 51 → 60 | 8.98 → 9.10 | 43.8 → 46.6 | 48.4 → 45.4 | 18 → 17% |
| window-3840x2160@2 | 5 | 7.2 → 19.5 | 209 → 66 | 380 → 221 | 3.16 → 3.27 | 36.0 → 41.7 | 47.7 → 45.3 | 11 → 22% |
| window-3840x2160@2 | 10 | 21.1 → 20.4 | 87 → 67 | 152 → 187 | 7.90 → 6.35 | 36.2 → 43.9 | 47.7 → 45.3 | 24 → 26% |
| window-3840x2160@2 | 25 | 22.8 → 22.2 | 57 → 61 | 70 → 102 | 14.10 → 13.46 | 43.8 → 46.3 | 47.7 → 45.3 | 28 → 33% |
| mixed-1920x1080 | 5 | 12.4 → 15.0 | 74 → 63 | 320 → 155 | 2.06 → 2.08 | 38.4 → 40.0 | 47.0 → 47.0 | 6 → 10% |
| mixed-1920x1080 | 10 | 15.6 → 15.2 | 52 → 54 | 131 → 143 | 3.49 → 3.54 | 39.1 → 42.0 | 47.0 → 47.0 | 9 → 10% |
| mixed-1920x1080 | 25 | 16.1 → 15.9 | 38 → 40 | 70 → 71 | 5.37 → 4.86 | 41.3 → 42.7 | 47.0 → 47.1 | 10 → 12% |
| mixed-2880x1800@2 | 5 | 6.5 → 12.6 | 258 → 94 | 561 → 338 | 2.12 → 2.19 | 40.4 → 40.1 | 47.6 → 47.6 | 10 → 24% |
| mixed-2880x1800@2 | 10 | 11.1 → 14.0 | 121 → 84 | 295 → 192 | 4.18 → 4.04 | 39.0 → 41.7 | 47.6 → 47.7 | 16 → 27% |
| mixed-2880x1800@2 | 25 | 15.0 → 14.4 | 63 → 68 | 142 → 163 | 9.01 → 8.08 | 41.8 → 44.8 | 47.6 → 47.7 | 22 → 31% |

- **On a slow link and a large desktop** it did what it was for: the 4K
  window at 5 Mbit/s played at 19.5 frames a second where it has 7.2, its
  changes shown at a median 66 ms where they take 209, and 5.7 dB finer
  while it moved; the mixed session at 2880x1800 at 12.6 where it has 6.5.
- **With room on the link** the frame rate was within 1.3 frames a second
  of main's and the picture while it moved 1.4 to 7.7 dB finer, the
  slowest updates up to 35 ms slower at the p95.
- **Once still**, a window's region ended at the ceiling's 4:2:0 picture,
  QP 18, 2.4 to 3.7 dB under the full stage.
- **The processor**, on lab1, about twice main's where the frame rate rose
  with it: 22% of a core where 11 on the 4K window at 5 Mbit/s.

**Why it was not taken.** The walked list is the picture the page draws
with the module that composes an RDP host's pipeline alone, with no video
decoder, as a Windows host's pipeline without H.264 is drawn, which adapts
smoothly to a slow link on a phone: crisper text than the video's, light
enough for a phone, a tablet or a page short of memory, on a link that is
not always wide. A stream for each region puts up to four of the
browser's video decoders beside that module, each kept a few seconds past
its region for the next to reuse, the page then composing as it does an RDP
host's mixed pipeline, which zDesk does not recommend on a phone or tablet;
and a browser with no H.264 decoder would lose the picture. The H.264 was
left out of the walked list for that, and is left out still: on a slow link
and a large desktop the frame rate falls, as the host's does without H.264.

## Lossless tiles

Since protocol 12 the lossless picture's moving runs are the median
predictor's ([below](#the-median-predictor)), and the planar moving runs
measured here are gone: these figures are of the picture as it was on
2026-10-31, its still stream what `WLSL` still sends.

The lossless picture (experimental), every tile that changed a planar
bitmap, what moves each in a zstd frame of its own and what is still in
the still stream, against the held picture, every
still a PNG and what moves a region's H.264 stream, measured on
2026-10-31. lab1 (eight cores of EPYC Rome, rebuilt that day), alone, the
regions by libx264 as it has no GPU: `motion-bench --tiles-only` and
`--tiles-only --lossless` over every sample at every size, and
`check --lossless` over the still ones, of the bitmaps alone, framed at
zstd's level 1, framed with the parameters they have now, coded by the
planar coder as it is now (the same bytes), and with the still stream.
Archived in the artifacts as `bench/lossless-20261031/`.

Every frame of every capture was exact on the page, and every still capture
settled at once and sent nothing through 5 s of quiet and 10 redraws, each
way. Summed over the sizes, flood for reference, the processor's time over
every thread:

| Scenario | CPU, held → lossless | Bytes, held → lossless | Bytes, every bitmap framed alone | Bytes, the bitmaps alone |
|---|---:|---:|---:|---:|
| browser | 12.1 → 12.4 s | 89 → 20 MB (0.22×) | 30 MB | 152 MB |
| drag | 4.3 → 4.4 s | 6.6 → 0.6 MB (0.09×) | 1.0 MB | 9.1 MB |
| terminal | 0.7 → 0.7 s | 9.9 → 1.3 MB (0.13×) | 3.3 MB | 28 MB |
| uncover | 2.2 → 2.6 s | 24 → 3.9 MB (0.16×) | 6.4 MB | 30 MB |
| video | 22.8 → 29.6 s | 324 → 2,669 MB (8.2×) | 2,672 MB | 4,648 MB |
| walk | 36.0 → 44.9 s | 578 → 3,661 MB (6.3×) | 3,667 MB | 6,487 MB |
| window | 6.7 → 6.8 s | 96 → 706 MB (7.3×) | 706 MB | 1,127 MB |
| mixed | 75.4 → 88.4 s | 660 → 8,000 MB (12×) | 7,996 MB | 14,192 MB |
| flood | 59.0 → 42.9 s | 2,598 → 3,142 MB (1.2×) | 3,143 MB | 5,364 MB |
| all but flood | 160 → 190 s (1.19×) | 1,786 → 15,062 MB (8.4×) | 15,082 MB | 26,673 MB |

- **The link.** Where the desktop is still, or moves by copies, it is a
  quarter of the PNG's bytes or less: the planes' runs leave a page's
  repeats, which zstd takes 5 to 10 times off where deflate's matches in a
  PNG did less, and the still stream more again. Where it moves, it is 6 to 12 times the held
  picture's, every moving tile sent exact at the frame rate rather than a
  stream's: at 30 frames a second a video at 1920×1080 is 248 Mbit/s, at
  4K 723, a video in a 4K window 249, and the mixed session 89 Mbit/s at
  1920×1080 and 196 at 2880×1800 on the mean. On a link short of that the
  walk slows the updates, never what a tile shows.
- **The processor.** 1.19 times the held picture's, all of it where the
  desktop moves: a still or a scroll costs what a PNG did, give or take a
  fifth, and a video at 1920×1080 11.9 ms a frame against 9.9, its 95th
  percentile 16, at 4K 45 against 33, the frames now most of it. A run is
  packed on any core, so an update's wait is its largest run's and not
  their sum; the time is the same. On a host whose GPU codes the held
  picture's regions, the held picture is the cheaper by more.
- **The planar coder.** It first walked each row a byte at a time, three
  planes over, looking for a run at every byte, and cost 1.75 times the
  held picture in all, a video at 1920×1080 17.2 ms a frame, its 95th
  percentile 24, at 4K 72. It now finds a run eight bytes at a time, cuts
  the three planes from a row of pixels in one pass, compiled for AVX2 too
  and chosen at run time, and takes a row the same colours as the row
  above as one coding of a row of zeroes: the same bytes, the bitmaps a
  quarter of their time and less (over 120 frames on one thread, a video
  at 1920×1080 1.43 s to 0.34 s, the browser 1.02 s to 0.19 s;
  `planar-bench`), 92 s off the whole, flood's 64 s 42. Without AVX2 the
  coder takes twice as long: the video 0.73 s.
- **What the frame was chosen by.** The bitmaps alone cost 1.14 times the
  held picture's processor and were 15 times its bytes, 1.3 to 2.8 times
  where it is still. Framed at zstd's level 1 they were 14,764 MB for
  306 s; the fast strategy with a 4096-entry table and 7-byte matches took
  9% off that time for 2% more bytes, a moving picture's frame being its
  literals' entropy coding more than its matches. Levels below 1, which
  code no literal, took a video's bitmaps 1.07 to 1.13 times smaller,
  where level 1 took them 1.67.
- **The still stream.** One zstd stream across the session's still
  bitmaps, flushed at each, took a third to three fifths off where the
  desktop is still (the browser 30 → 20 MB, the terminal 3.3 → 1.3, a
  drag 1.0 → 0.6, uncovering 6.4 → 3.9), and a moving picture, which
  stays framed, nothing: 1% more of the processor in all, coded in order.
  Offline, over the 1920×1080 samples' still runs, its window chose it:
  128 KiB took the browser's 807 KB framed to 453, 1 MiB to 156 and 8 MiB
  to 144; restarting the stream every update kept 379. A tile is still by
  the churn the held picture lays its regions out by. Its table and window
  were chosen again later (below).

At 1920×1080:

| Capture | Held: ms a frame, MB | Lossless: ms a frame, MB, Mbit/s at its frame rate |
|---|---:|---:|
| browser | 4.61, 4.0 | 4.77, 0.6, 1 |
| terminal | 0.25, 0.6 | 0.32, 0.07, 0 |
| drag | 1.24, 0.3 | 1.16, 0.03, 0 |
| window | 3.90, 7.2 | 3.52, 49, 91 |
| video | 9.94, 19.1 | 11.87, 149, 249 |
| walk | 10.56, 34.2 | 12.23, 219, 116 |
| mixed | 6.53, 221 | 6.92, 2,500, 89 |

A Windows host set to Lossless measured the same way: at 1920×1080 a video
in a 960×600 window came to 76 to 106 Mbit/s of planar, its bulk
compression taking a fifth off, against 5 Mbit/s from a stock host's
RemoteFX Progressive (zDesk's
[Lossless, by the host's policy](https://github.com/andrewtheguy/zdesk/blob/main/docs/rdp-client.md#lossless-by-the-hosts-policy)).
The picture is the host's, for a LAN, and costs what it costs there.

**The moving frames' step and the still stream's table**, measured on
2026-11-05 on lab1 (eight cores of EPYC Rome, rebuilt that day), alone:
`motion-bench --tiles-only --lossless` over every sample at every size,
0.0.119 against both changes, two runs each, each change also against
0.0.119 alone. Archived in the artifacts as `bench/lossless-zstd-20261105/`,
beside `bench/lossless-codecs-20261105/`, which measured what led to them.
The bytes are this machine's count and lab1's alike. Summed over the sizes,
the processor's time over every thread, the mean of the two runs:

| Scenario | CPU, 0.0.119 → now | Bytes, 0.0.119 → now |
|---|---:|---:|
| browser | 13.7 → 13.5 s | 19.6 → 3.6 MB (0.18×) |
| drag | 4.6 → 4.6 s | 0.59 → 0.52 MB (0.88×) |
| terminal | 0.7 → 0.6 s | 1.3 → 0.9 MB (0.68×) |
| uncover | 2.6 → 2.5 s | 3.9 → 2.7 MB (0.69×) |
| still | 0.3 → 0.2 s | 1.4 → 0.6 MB (0.45×) |
| video | 30.4 → 25.1 s | 2,670 → 2,679 MB |
| walk | 46.5 → 39.9 s | 3,661 → 3,623 MB |
| window | 7.0 → 6.6 s | 706 → 706 MB |
| mixed | 90.7 → 77.0 s | 8,000 → 8,011 MB |
| flood | 44.0 → 36.1 s | 3,142 → 3,446 MB |
| all but flood | 197 → 170 s (0.87×) | 15,064 → 15,027 MB |

- **The moving frames' step.** A frame of its own now steps 8 bytes past a
  place where the fast strategy found no match, its literals forced to be
  Huffman coded still (zstd's `literalCompressionMode`, which a step
  otherwise turns off): a moving picture's frame is its literals' entropy
  coding more than its matches. Before it, of the encoder's time over a
  video at 1920×1080 by `perf`, the frames' zstd was about half, its match
  search and its entropy coding each a quarter, the planar coder 29%,
  comparing the tiles 17% and the cache's hashes 6%. Alone it took the
  processor from 197 s to 169 over all but flood for 0.19% more bytes, a
  flat run of tiles, a few hundred bytes either way, costing the most; with
  the table below, a video at 1920×1080 is 9.9 ms a frame where it was 11.6,
  its 95th percentile 12.6 where it was 15.3, and at 4K 39.2 where it was
  45.9. The still stream keeps every look: on a page of text a step of 2
  cost 3 to 4% more bytes.
- **The still stream's table.** The 4096-entry table it shared with the
  frames forgets a match from more than a few thousand places back, so the
  window, chosen with it, was never what limited the stream: with 2^16
  entries, 1 MiB to 8 MiB took the browser's stills at 2880×1800 from 689 KB
  to 364, and 0.0.119's came to 2,227. 2^16 at 8 MiB, the most the protocol lets a STILL's frame have, is
  now the stream's: a desktop of pages comes to a half to seven eighths
  less than framed alone (the browser 30 → 3.6 MB, the terminal 3.3 → 0.9),
  a ninth to a twenty-fifth of the held picture's PNGs, and the stills of a walk or
  the mixed session 7 and 15% fewer. 2^17 took under 1% more. Against
  0.0.119's table it cost the mixed session 4% more of the processor
  (76.4 → 79.4 s, the step in both), the stream being coded in order on
  the encoder's thread, and the stills alone nothing.

**Measured and not taken**, each over the samples as the steps above:

- **The planes as they are.** A bitmap sent as its planes untouched where
  the planes' segments come to more would have thrown away the rows'
  differences on a video; over the samples no bitmap did.
- **A larger table, window or level for the moving frames.** A table of
  2^16, a 1 MiB window or level 3: 1% fewer bytes at most, for 1.7 to 5
  times the zstd's time.
- **The frame before, by zstd.** A zstd stream for each tile row, each
  bitmap matched against the rows' last, with a 2 MiB window and a table of
  2^17 to 2^19: 1 to 3% off a video, a walk or a window, for 3 to 5 times
  the zstd's time. A decoded video's frames are close but seldom byte for
  byte the same, which only a prediction that codes the difference, as a
  video codec's does, takes off.

## The median predictor

What chose the median predictor as the lossless picture's own moving runs:
since protocol 12 it is what `WLSL` sends where the desktop moves, `WLMD` and
the planar moving runs gone, and `motion-bench --lossless` codes it. The
runs below name the two pictures as they were then.

The lossless tiles by the median predictor (`WLMD`, experimental), what
moves the predictor's residuals in three zstd frames where a lossless
list's is a planar bitmap in one, against the lossless tiles, measured on
2026-11-10 as built: the host's encoder through `motion-bench`, and the
page's decoder in Chrome, which what chose it
([Through motion-bench](#through-motion-bench)) left unmeasured. Archived in
the artifacts as `bench/median-tiles-20261110/`.

**The host's encoder.** lab1 (four cores of EPYC Rome, rebuilt that day, so
its planar figures are not the eight cores' of
[Lossless tiles](#lossless-tiles)), alone: `motion-bench --tiles-only
--lossless` and `--tiles-only --median` over every sample at every size,
twice each, one after the other. Every frame of every capture was exact on
the bench's page under both, and a picture's bytes were the same in both
its runs. Summed over the sizes, flood for reference, the processor's time
over every thread, the mean of the two runs:

| Scenario | CPU, lossless → median | Bytes, lossless → median |
|---|---:|---:|
| browser | 11.0 → 10.9 s | 3.58 → 3.58 MB |
| drag | 3.9 → 3.9 s | 0.52 → 0.52 MB |
| terminal | 0.5 → 0.5 s | 0.91 → 0.94 MB (1.03×) |
| uncover | 2.1 → 2.0 s | 2.72 → 2.72 MB |
| still | 0.2 → 0.2 s | 0.60 → 0.60 MB |
| video | 20.3 → 21.1 s (1.04×) | 2,679 → 1,670 MB (0.62×) |
| walk | 32.0 → 31.5 s (0.99×) | 3,623 → 2,222 MB (0.61×) |
| window | 4.7 → 4.9 s (1.05×) | 706 → 457 MB (0.65×) |
| mixed | 60.0 → 62.0 s (1.03×) | 8,011 → 5,522 MB (0.69×) |
| flood | 28.8 → 30.3 s (1.05×) | 3,446 → 2,713 MB (0.79×) |
| all but flood | 135 → 137 s (1.02×) | 15,027 → 9,879 MB (0.66×) |

The milliseconds a frame with their 95th percentile, and the link at the
capture's frame rate:

| Capture | Lossless: ms a frame, MB, Mbit/s | Median: ms a frame, MB, Mbit/s |
|---|---:|---:|
| window, 1920×1080 | 2.47 (3.2), 49, 91 | 2.62 (3.6), 31, 58 |
| video, 1920×1080 | 7.67 (9.9), 149, 249 | 8.08 (10.7), 96, 160 |
| walk, 1920×1080 | 8.62 (15.1), 219, 116 | 8.96 (16.5), 148, 78 |
| mixed, 1920×1080 | 4.77 (11.6), 2,498, 89 | 4.96 (12.4), 1,730, 62 |
| mixed, 2880×1800 | 12.99 (27.3), 5,513, 196 | 13.39 (29.2), 3,792, 135 |
| video, 2880×1800 | 22.23 (28.9), 377, 588 | 23.98 (32.3), 246, 383 |
| video, 3840×2160 | 32.63 (42.9), 432, 722 | 32.10 (43.2), 244, 408 |

- **The link.** Two thirds of the lossless tiles' bytes: the moving frames
  14,515 MB to 9,367 over all but flood, the still stream's 512 MB as they
  were. By capture, where the desktop moves, 0.48 to 0.71 of the lossless
  tiles' bytes, the least at 4K: a video at 1920×1080 160 Mbit/s at its
  frame rate where theirs is 249, at 4K 408 where theirs is 722, and the
  mixed session 62 where theirs is 89.
- **The processor.** 2% more over all but flood, inside what two runs
  differ by: the lossless tiles' two 134.4 and 134.9 s, the predictor's
  137.8 and 136.0. By capture, where the desktop moves, 0.88 to 1.10 of the
  lossless tiles', the most on a small video and the least at 4K: a video
  at 1920×1080 0.4 ms more a frame, its 95th percentile 0.8 more.
- **A page.** A browser, a drag and an uncovering go through the still
  stream whole, the same bytes under both. A terminal has moving runs, 7 to
  11 KB a capture, which the predictor codes 2 to 4 KB larger: 3% of the
  terminal's bytes.
- **Against the scratch copy.** The bytes are the scratch copy's of the day
  before, capture for capture, and the lossless tiles' as well; its
  processor was 4% more than planar's, its runs 139.2 s both.

**The page's decoder.** zDesk's WebAssembly module as its page loads it
(`zdesk-egfx` of the branch that brought the picture, its threads with it),
in a worker of a page of its own in Chrome 156: the updates
`motion-bench` sent over five samples, written out as they were sent under
each picture, each update's tiles handed to the module in one call as a
batch's are in a session, and the call timed. That is the zstd frames, the
still stream, the planar bitmaps or the planes' samples, and the pixels as
R,G,B,A; drawing them, the same under both, is not in it. Each sample three
times a run, and every update's pixels the same under both pictures. Two
machines: a Windows desktop (six cores of i5-8500T, the module's four
threads, in use meanwhile, two runs taken together), and testdesktop2 (two
cores of i5-8400T, two threads, idle, its three passes within 1%). The milliseconds an update with their 95th percentile:

| Sample | Mpixel an update | The desktop, four threads: lossless, median | testdesktop2, two threads: lossless, median |
|---|---:|---:|---:|
| window, 1920×1080 | 0.30 | 1.96 (2.9), 2.45 (3.6), 1.25× | 2.58 (3.6), 3.39 (4.9), 1.31× |
| video, 1920×1080 | 0.87 | 6.53 (11.2), 6.96 (11.3), 1.07× | 7.79 (11.0), 9.76 (13.8), 1.25× |
| walk, 1920×1080 | 0.90 | 6.83 (12.8), 7.81 (13.3), 1.14× | 8.39 (13.7), 9.92 (15.9), 1.18× |
| video, 2880×1800 | 2.31 | 13.46 (20.4), 15.55 (22.8), 1.16× | 21.36 (30.4), 26.41 (38.5), 1.24× |
| video, 3840×2160 | 3.12 | 19.00 (28.2), 21.98 (32.9), 1.16× | 29.83 (43.9), 36.71 (55.8), 1.23× |

- **What it costs the page.** A tenth to a third more than the lossless
  tiles an update: 0.4 to 2 ms at 1920×1080, and at 4K 3 ms on four
  threads and 7 on two. A video's frames are 40 ms apart, so at 1920×1080
  either is a quarter of a frame at most on the mean; at 4K on two cores the
  predictor's update is 37 ms on the mean and 56 at its 95th percentile
  where the lossless tiles' are 30 and 44, so that page falls behind under
  either and sooner under the predictor.
- **The threads.** On the desktop, a video at 1920×1080 on one thread
  is 12.4 ms an update by the lossless tiles and 16.4 by the predictor
  (1.32×), on two 7.3 and 8.7 (1.20×), on four 6.5 and 7.0 (1.07×); at 4K
  48.9 and 60.5 (1.24×), 27.9 and 33.7 (1.21×), 19.0 and 22.0 (1.16×). A
  run's three frames are read beside the other runs', and its three planes'
  samples are taken side by side, so the predictor's more is spread over
  the cores a page has.
- **In a session.** zDesk's page itself, driven by Playwright, started with
  the picker's choice on a daemon sharing a headless labwc with a test
  pattern moving at 30 frames a second: in Chromium on testdesktop2's
  display (ANGLE on Mesa, its UHD 630), 20 s brought 534 MEDIAN tiles, no
  PLANAR, 282 STILL, in 261 batches, every one acknowledged, no repaint
  asked for and nothing logged by the page; in a Chromium drawing in
  software, 719, none, 295, in 313. A check that it draws, not a figure.
- **Not measured.** Any browser but Chrome and Chromium; what the decoder's
  time goes to, which is written plainly, a sample at a time down each
  plane, and has not been profiled; and a session's rate on a link, which
  the walk paces as it does the lossless tiles'.

## Other coders for the lossless tiles' moving runs

What might code a moving run in place of planar and its zstd frame, measured
on 2026-11-09 for the roadmap's [Lossless tiles](roadmap.md#lossless-tiles);
none of it in the daemon then; the median predictor was built the day
after, a picture of its own ([The median predictor](#the-median-predictor)).
First a scratch bench of every coder, then
the two the roadmap keeps through `motion-bench`
([below](#through-motion-bench)). The scratch bench is on one thread, over
the 1920×1080 samples: every run the encoder codes, a row of changed tiles
at a time, the tiles compared with the frame before, coded by the daemon's
`planar::pack` and by each coder, each run decoded again and compared with
what went in. A coder's time includes cutting the run to R,G,B, which
planar's does not. lab1 (four cores of EPYC Rome), alone, wlshare at 0.0.122.
Archived in the artifacts as `bench/lossless-codecs-20261109/`, beside
`bench/lossless-codecs-20261105/`, which first measured JPEG-LS.

Bytes and encoding time, each against planar's, the mixed session over every
run, still or moving:

| Capture | JPEG-LS | HTJ2K | JPEG XL, effort 1 | Median, zstd | Median by context, zstd |
|---|---:|---:|---:|---:|---:|
| video | 0.55, 13.5× | 0.62, 5.1× | 0.65, 5.2× | 0.64, 2.1× | 0.61, 3.8× |
| walk | 0.56, 13.3× | 0.64, 5.0× | 0.66, 5.1× | 0.66, 2.0× | 0.62, 3.8× |
| window | 0.56, 14.5× | 0.65, 6.0× | 0.65, 6.6× | 0.64, 2.3× | 0.61, 4.6× |
| mixed | 0.62, 12.5× | 0.74, 5.1× | not exact | 0.71, 2.1× | 0.67, 4.2× |
| browser | 1.21, 6.4× | 2.78, 7.3× | 1.23, 11.6× | 0.90, 4.3× | 0.96, 9.1× |
| drag | 1.55, 5.8× | 3.19, 7.0× | 1.67, 13.7× | 0.76, 3.6× | 1.05, 7.6× |
| uncover | 1.47, 5.6× | 3.06, 6.3× | 1.64, 14.3× | 0.80, 3.3× | 1.05, 7.0× |
| terminal | 1.96, 5.9× | 4.01, 6.9× | 2.14, 19.3× | 1.11, 4.0× | 1.23, 7.9× |

Every run of every coder came back exact but five of JPEG XL's in the mixed
session, below. A video is 4.7 ms a frame by planar, 9.7 by the median and
63 by JPEG-LS, and decoded in native code in 9.9 ms by the median, 12 by
HTJ2K, 22 by the median's contexts, 35 by JPEG XL and 49 by JPEG-LS.

- **A median predictor ahead of zstd.** G, R−G and B−G as planes, each
  sample's residual from the median of the sample left, the one above and
  left + above − above-left, which is JPEG-LS's predictor, a zstd frame a
  component with the parameters a planar bitmap's frame has. Two thirds of
  planar's bytes where the desktop moves for twice its time, nine points
  short of JPEG-LS for a sixth of its time; and where the desktop is a
  page, each run framed alone, smaller than planar but on the terminal,
  for 3.3 to 4.3 times planar's time, which codes a row the same as the row
  above at once. In the daemon a page's runs go to the still stream, whose
  matches are across bitmaps, and never to it (below). Only the encoder's
  inner loop was written for speed, a row at a time from the pixels
  themselves.
- **Its residuals by context.** The same residuals in eight streams a
  component by how busy a sample's neighbours are (|left − above-left| +
  |above − above-left| + |above-right − above|, cut at 0, 2, 5, 10, 20, 40
  and 80), each stream a zstd frame: three or four points fewer where the
  desktop moves for nearly twice the median's time, and more than the
  median alone on a page. Under an adaptive table for each stream in place
  of zstd's Huffman, whose shortest code is a bit, the residuals' bits come
  to 0.58 of planar's bytes on the video, 0.59 on the walk and the window
  and 0.65 on the mixed session, counted and never coded; under one table
  a component, 0.64, 0.66, 0.69 and 0.73.
- **JPEG-LS.** CharLS 2.4.2 (Debian's `libcharls-dev`), a line at a time,
  with its HP1 colour transform; HP2 and HP3 came to the same, a sample at
  a time to 0.54 of the video for 14 times planar's time, and no transform
  to 0.69 (measured 2026-11-05). The fewest bytes where the desktop moves
  and the most time, and 1.2 to 2 times planar's bytes on a page, which
  zstd's matches of what repeats take off. A run starts over what it has
  learned of its contexts: the video's frames whole are 81.9 MB and in
  strips a tile high, each coded alone, 83.4, 1.8% more, as the walk's are;
  in strips 768 wide 2.5%, as single tiles 6.7%, and the browser's 12%, 16%
  and 57% (this machine, bytes alone).
- **HTJ2K.** OpenJPH 0.21.2 (`libopenjph-dev`), reversible, 64×64 blocks,
  five decompositions; three were 1% more. The median's bytes where the
  desktop moves for two and a half times its time, and 2.8 to 4 times
  planar's bytes on a page. It decodes in a quarter of JPEG-LS's time.
- **JPEG XL.** libjxl 0.11.2 (`libjxl-dev`), lossless at effort 1, its fast
  path. The median's bytes for two and a half times its time, slower to
  decode than to code, and not lossless: five of the mixed session's 47,062
  runs, each one to three tiles, came back with one pixel wrong, and
  libjxl's own `cjxl` and `djxl` do the same with them, where effort 2 is
  exact. Every run of the other seven samples was exact; the mixed
  session's bytes with those five are 0.72 of planar's. At effort 2 the
  video is 0.59 for 11 times planar's time (this machine).
- **Whole frames, by ffmpeg.** The video's 120 frames, still picture and
  all, by ffmpeg 7.1 on one thread (this machine), against planar's 148 MB
  over its runs: FFV1 78 to 81 MB in the time of ffmpeg's JPEG-LS, which
  with no transform is 102; x264 lossless at 4:4:4 101 to 118, its
  prediction from the frame before 8% of that, and at ultrafast a quarter
  of that time; lossless WebP 106, PNG 125, QOI 136. Ut Video and MagicYUV,
  Huffman coders of a median's residuals, are 153 and 158 whether the table
  is the frame's or a slice's, where the same residuals over the runs are
  0.64 of planar's: a table's shortest code is a bit, paid for every
  sample of the still picture, 145 of the frames' 249 Mpixel, which the
  runs leave out. ffmpeg's JPEG-LS `-pred` is the error a sample may be
  left with, not a predictor: at 1 it is 65 MB and off by one in a third of
  its samples.

### Through motion-bench

The median predictor and JPEG-LS in the daemon's own encoder, measured on
2026-11-09 with neither built into it: a scratch copy of the tree at 0.0.122
whose `planar::pack` hands a moving run to the coder the environment names,
the bench decoding each with its decoder. The median coder is written as
planar's is: cut from a row of pixels, compiled for AVX2 too and chosen at
run time, a row the same colours as the row above taken at once as a row of
zero residuals. CharLS is as above. A still run is a planar bitmap in the
still stream under each. lab1 (four cores of EPYC Rome, so its planar
figures are not the eight cores' of [Lossless tiles](#lossless-tiles)),
alone: `motion-bench --tiles-only --lossless` over every sample at every
size, planar and the median twice each, JPEG-LS once. Archived in the
artifacts as `bench/lossless-median-20261109/`.

Every frame of every capture came back exact under each coder. Summed over
the sizes, flood for reference, the processor's time over every thread, the
mean of the runs:

| Scenario | CPU: planar, median, JPEG-LS | Bytes: planar, median, JPEG-LS |
|---|---:|---:|
| browser | 10.7, 10.8, 10.6 s | 3.6, 3.6, 3.6 MB |
| drag | 3.9, 3.9, 4.0 s | 0.52, 0.52, 0.52 MB |
| terminal | 0.5, 0.5, 0.5 s | 0.91, 0.94, 0.97 MB |
| uncover | 2.1, 2.0, 2.0 s | 2.7, 2.7, 2.7 MB |
| still | 0.2, 0.2, 0.2 s | 0.6, 0.6, 0.6 MB |
| video | 20.4, 21.5 (1.05×), 134.4 (6.6×) s | 2,679, 1,670 (0.62×), 1,370 (0.51×) MB |
| walk | 32.1, 31.8 (0.99×), 172.5 (5.4×) s | 3,623, 2,222 (0.61×), 1,919 (0.53×) MB |
| window | 4.8, 4.9 (1.04×), 36.9 (7.8×) s | 706, 457 (0.65×), 398 (0.56×) MB |
| mixed | 59.9, 63.6 (1.06×), 458.0 (7.6×) s | 8,011, 5,522 (0.69×), 4,671 (0.58×) MB |
| flood | 28.7, 30.7 (1.07×), 173.7 (6.1×) s | 3,446, 2,713 (0.79×), 3,554 (1.03×) MB |
| all but flood | 135, 139 (1.04×), 819 (6.1×) s | 15,027, 9,879 (0.66×), 8,367 (0.56×) MB |

At 1920×1080, the milliseconds a frame with their 95th percentile:

| Capture | Planar: ms a frame, MB | Median: ms a frame, MB | JPEG-LS: ms a frame, MB |
|---|---:|---:|---:|
| window | 2.57 (3.4), 49 | 2.77 (3.8), 31 | 20.7 (31), 28 |
| video | 7.56 (9.9), 149 | 8.18 (10.9), 96 | 65.5 (99), 82 |
| walk | 8.79 (14.7), 219 | 9.27 (17.2), 148 | 64.0 (111), 129 |
| mixed | 4.88 (12.2), 2,498 | 5.03 (12.4), 1,730 | 39.6 (118), 1,495 |

- **The median predictor.** Two thirds of the lossless tiles' bytes for 4%
  more of the processor: the moving frames 14,515 MB to 9,367 over all but
  flood, the still stream's 512 MB as they were. By capture, where the
  desktop moves, 0.48 to 0.71 of planar's bytes for 0.88 to 1.11 of its
  processor, the two planar runs 133.9 and 135.1 s over all but flood and
  the median's both 139.2. A video at 1920×1080 is 159 Mbit/s at its frame
  rate where planar's is 248, and at 4K 244 MB for 32.5 ms a frame where
  planar's is 432 for 32.7. Written as planar's coder is, it adds 0.6 ms
  to a video's frame at 1920×1080, where the scratch bench's added 5.
- **JPEG-LS.** 0.56 of the bytes, ten points fewer than the median, for 6.1
  times the processor, 5.9 times the median's: a video at 1920×1080 65 ms of
  the processor a frame, at 2880×1800 172. On flood it is more than
  planar's bytes.
- **A page.** A browser, a drag and an uncovering go through the still
  stream whole, the same bytes under each coder, so what the scratch bench
  found of a page's runs framed alone does not arise. The terminal has
  moving runs, 7 to 11 KB a capture, which the median codes 2 to 4 KB
  larger and JPEG-LS 3 to 9: 3% and 6% of the terminal's bytes.
- **Not measured.** The page's decoding: the bench's times are the
  encoder's alone, and its decoder is native code. The median predictor's
  is measured as built, [above](#the-median-predictor).

## WebP stills and the eased settle

Measured 2026-11-11 on a four-vCPU AMD EPYC (Rome) VM, idle and with no
swap, `motion-bench --libx264 --tiles-only --frames` over the 1080p samples
and the larger ones named, against main at `8a83c5f`, whose held stills were
a fast PNG and whose settle went at the pace from its first tick. Two
changes, one for each thing a user asked for: fewer bytes for a still, and
no step up in what the link carries when a video stops.

**The stills are WebP.** A held list's still is a lossless WebP by libwebp
in place of the `png` crate's fast PNG. The coders like for like, the
update's budget of stills lifted for both so that each codes the same tiles
and nothing starts as a region that the churn did not make one, bytes of
stills and the encoder's CPU over the capture, libwebp at its lossless
presets:

| Capture | Fast PNG | Preset 0 | Preset 1 | Preset 3 |
|---|---:|---:|---:|---:|
| still-1920x1080 | 264 kB, 21 ms | 170 kB, 44 ms | 96 kB, 58 ms | 82 kB, 76 ms |
| terminal-1920x1080 | 640 kB, 30 ms | 167 kB, 71 ms | 132 kB, 77 ms | 104 kB, 127 ms |
| browser-1920x1080 | 4.01 MB, 0.63 s | 2.34 MB, 0.90 s | 1.30 MB, 0.89 s | 1.10 MB, 1.20 s |
| uncover-1920x1080 | 1.34 MB, 0.12 s | 0.39 MB, 0.17 s | 0.29 MB, 0.17 s | 0.27 MB, 0.27 s |
| drag-1920x1080 | 267 kB, 0.15 s | 46 kB, 0.16 s | 31 kB, 0.17 s | 28 kB, 0.18 s |
| mixed-1920x1080 | 71.2 MB, 26.1 s | 39.7 MB, 29.0 s | 30.5 MB, 35.0 s | 28.3 MB, 44.8 s |

The mixed session's time is the whole encoder's, its regions' 117 MB of
H.264 with it, the same in every column. Preset 1 is the knee and the one
taken: 43% of the PNG's bytes over the session for a third more of the
encoder's time, three quarters of preset 0's for a fifth more, and presets
2 to 6 are a twentieth to a tenth fewer for a tenth to a third more time. Text
and an interface are where WebP gains, a fifth to a half of the PNG; a
photograph is where it gains least and costs most, since nothing lossless
is far under its entropy: the settle of a 1080p video's last frame is
1.58 MB as PNG in 15 ms, 1.19 MB at preset 0 in 56 and 1.12 MB at preset 1
in 247, a quarter of a second of one core, once, when a video stops.

**The settle eases in.** On main the settle after the 1080p video sent its
first tick of 191 kB half a second after the stream's END and 200 kB every
50 ms from there: 33 Mbit/s over its busiest quarter second where the
stream had been 20. Two things made it more than the pace it was meant to
keep to: a tick checked its bytes between runs, and a run is a row, 30
tiles of a photograph, more than a tick's bytes by itself; and the ticks'
own bytes counted toward the pace, so a settle raised the rate it kept to.
Now the pace is the reads' bytes alone; a run is cut to what is left of
the tick's bytes, each tile taken to cost what its pixels that differ from
the one to their left cost in the piece before, a piece half of what is
left at most; and the settle's rate rises to the pace over the second
after it began paying
— the pace measured at every update while anything is owed, the update's
own bytes in it, whether or not a tick had a tile to pay then, so a
settle that begins more than a second after a video's last frame still
keeps to the video's rate —
([architecture](architecture.md#the-tiles-encoding)). As the encoder is
now, WebP and the eased settle together, against main:

| Capture | Bytes | CPU | Settle's busiest 250 ms | Its first tick | Settled after |
|---|---:|---:|---:|---:|---:|
| video-1920x1080 | 13.0 → 12.1 MB | 1.58 → 2.64 s | 32.9 → 16.7 Mbit/s | 191 → 8 kB | 0.95 → 1.48 s |
| video-2880x1800@2 | 26.3 → 24.7 MB | 5.29 → 6.96 s | 29.0 → 21.2 Mbit/s | 138 → 14 kB | 1.77 → 1.93 s |
| window-1920x1080 | 4.32 → 5.16 MB | 0.59 → 0.93 s | 10.4 → 6.1 Mbit/s | 150 → 16 kB | 1.39 → 1.20 s |
| window-2880x1800@2 | 7.14 → 8.37 MB | 1.78 → 1.62 s | 21.2 → 8.7 Mbit/s | 7 → 11 kB | 1.62 → 1.56 s |
| browser-1920x1080 | 2.27 → 1.30 MB | 0.95 → 0.84 s | 4.7 Mbit/s → none | 146 kB → none | 0.81 s → at once |
| uncover-1920x1080 | 1.30 → 0.29 MB | 0.23 → 0.18 s | none | none | at once |
| still-1920x1080 | 455 → 96 kB | 45 → 44 ms | none | none | at once |
| still-2880x1800@2 | 1.19 → 0.46 MB | 0.15 → 0.17 s | 5.3 Mbit/s → none | 6 kB → none | 1.64 s → at once |
| still-3840x2160@2 | 1.80 → 0.71 MB | 0.22 → 0.25 s | 11.9 Mbit/s → none | 6 kB → none | 1.63 s → at once |
| terminal-1920x1080 | 1.11 → 0.13 MB | 78 → 81 ms | 2.3 Mbit/s → none | 71 kB → none | 0.10 s → at once |
| drag-1920x1080 | 550 → 31 kB | 0.27 → 0.16 s | 2.5 Mbit/s → none | 77 kB → none | 1.94 s → at once |

The stream's rate over each video's last second was 19.5 Mbit/s at 1080p
and 42 at 2880x1800@2, 8 and 12 in the windows: the settle is now under
the stream it follows at every size, from a first tick of 8 to 16 kB, and
no tick is past 135 kB where the largest was 253. What it pays is the time
to lossless, half a second more after the 1080p video, a sixth after the
larger, and none after a window.

Where no settle is left at all, the smaller still is why: a page of text
at 1080p is 96 kB, within the 128 KiB of stills an update carries, so it
is lossless at once and never a region; the terminal, the browser, the
drag and the window uncovered send no H.264 at all, every frame of theirs
lossless on the page where 0 to 108 of 120 were. The two larger pages of
text are still past the budget, and start over once where they did twice.

What rose is a video in a window. Its page was past the budget as PNG, so
the first picture began a region the video then played in, one stream for
the whole capture; as WebP the page is lossless at once, and the video
that fades in grows under the update's stills for most of a second
until a region carries all of it, up to 110 kB a frame, 0.8 MB more at
1080p and 1.2 at 2880x1800@2. That is the budget's known cost
([above](#the-settles-pace)), which a still a quarter the size now meets
in another place.

The budget was swept again for WebP the same day, the same way: the stills
an update carries, which is also the most a tick sends, at 32, 64, 128 and
256 KiB, the rest as above. Bytes, the busiest quarter second, and the
frames the page held lossless:

| Capture | 32 KiB | 64 KiB | 128 KiB | 256 KiB |
|---|---:|---:|---:|---:|
| video-1920x1080 | 10.7 MB, 21 Mbit/s | 11.3 MB, 23 | 12.1 MB, 30 | 13.2 MB, 43 |
| video-2880x1800@2 | 24.4 MB, 68 | 23.1 MB, 43 | 24.7 MB, 47 | 26.9 MB, 65 |
| walk-1920x1080 | 16.1 MB, 20 | 17.0 MB, 22 | 18.0 MB, 32 | 19.4 MB, 55 |
| window-1920x1080 | 3.87 MB, 8 | 4.85 MB, 13 | 5.16 MB, 15 | 5.16 MB, 15 |
| window-2880x1800@2 | 6.39 MB, 13 | 6.38 MB, 13 | 8.37 MB, 21 | 8.37 MB, 21 |
| browser-1920x1080 | 2.02 MB, 10, 0 of 120 | 2.05 MB, 11, 0 | 1.30 MB, 4, 120 | 1.30 MB, 4, 120 |
| uncover-1920x1080 | 0.53 MB, 9, 2 of 48 | 0.54 MB, 10, 2 | 0.29 MB, 3, 48 | 0.29 MB, 3, 48 |
| terminal-1920x1080 | 0.52 MB, 14, 108 of 120 | 0.52 MB, 15, 111 | 0.13 MB, 3, 120 | 0.13 MB, 3, 120 |
| still-1920x1080 | 0.52 MB, 16 | 0.30 MB, 10 | 0.10 MB, 3 | 0.10 MB, 3 |
| still-2880x1800@2 | 0.78 MB, 22 | 0.77 MB, 24 | 0.46 MB, 15 | 0.16 MB, 5 |
| still-3840x2160@2 | 1.22 MB, 33 | 1.19 MB, 35 | 0.71 MB, 23 | 0.24 MB, 8 |
| mixed-1920x1080 | 134.9 MB, 22 | 134.4 MB, 22 | 134.7 MB, 22 | 134.5 MB, 22 |
| mixed-2880x1800@2 | 273.4 MB, 46 | 273.2 MB, 46 | 273.5 MB, 46 | 273.9 MB, 46 |

The budget stays 128 KiB. Below it a page at 1080p is past the stills
again — the browser, the terminal, the window uncovered and the page of
text each start as a region, lossy, for 1.6 to 5 times the bytes and
about three to five times the busiest quarter second — for 7 Mbit/s off the
1080p video's busiest and 11 off the walk's; at 32 KiB the larger video
lays its regions out 27 times where it did 11, and a settle of 36 kB a
tick takes 4.7 s. Above it the two larger pages of text are lossless at
once, a third of their bytes, and a video's growth under the stills is
two fifths more of the link's busiest quarter second, 43 and 65 Mbit/s. The
windows' video is one stream again only where the page is past the
budget, at 32 KiB, or at 64 at 2880x1800@2. The sessions at the desk are
within a half of one per cent at every budget: the constant decides how
a picture starts and how a video begins, and little of what a session
sends.

That sweep moved one constant that did two things. They are two now, the
stills an update carries (`HELD_BYTES`) and the most a tick sends
(`SETTLE_MOST`), and were swept apart the same day, each with the other at
128 KiB. The stills alone give the table above again, to within a
hundredth, whatever the tick: how a picture starts and how a video begins
are theirs, and the tick has no part in them. The tick alone, the stills
at 128 KiB, changes the settle and nothing else — the bytes, the streams
and the busiest quarter second of every capture are the same at each — so
what it decides is the settle's own busiest quarter second, its largest
tick, and the time to lossless:

| Capture | 32 KiB | 64 KiB | 128 KiB | 256 KiB |
|---|---:|---:|---:|---:|
| video-1920x1080 | 5.6 Mbit/s, 36 kB, 2.37 s | 10.8, 70 kB, 1.67 s | 16.7, 118 kB, 1.48 s | 18.9, 209 kB, 1.43 s |
| video-2880x1800@2 | 5.6, 36 kB, 4.66 s | 10.9, 69 kB, 2.76 s | 21.2, 134 kB, 1.93 s | 35.8, 249 kB, 1.63 s |
| walk-1920x1080 | 5.7, 37 kB, 2.00 s | 10.8, 69 kB, 1.43 s | 15.2, 116 kB, 1.26 s | 16.1, 192 kB, 1.21 s |
| window-1920x1080 | 5.5, 36 kB, 1.29 s | 6.1, 50 kB, 1.27 s | 6.1, 85 kB, 1.20 s | 7.5, 142 kB, 1.21 s |
| window-2880x1800@2 | 5.7, 36 kB, 1.92 s | 10.3, 67 kB, 1.63 s | 8.7, 94 kB, 1.57 s | 10.4, 170 kB, 1.57 s |

Both stay 128 KiB. A tick of 256 KiB settles a twentieth of a second
sooner at 1080p and 0.3 s after the larger video, for an update twice the
size and a settle of 36 Mbit/s there beside the stream's 42; one of 64 KiB
is an update half the size for 0.2 s more to lossless at 1080p and 0.8 s
after the larger video, whose settle the 50 ms between ticks then holds
to 11 Mbit/s; one of 32 KiB takes 4.7 s there. Apart, neither finds a better place than
together: the pairs off the axes, 64 and 64 and 256 and 64, are their
stills' row and their tick's column, and the windows' video is one stream
only where the stills are too few for a page.

## What is worth a stream

Motion's rule made a video of whatever kept changing over 5 tiles' area of
the layout, and a desk has much that does and is no video: a striped
progress bar, a line of text sliding sideways, a notice fading in. `small`
is a capture of those — four minutes of the small movements of a desk, one
kind at a time with the desktop still between them
(`docker/capture/scenarios/small.sh`), at 1920x1080 and 2880x1800@2,
captured on 2026-11-11 — played through a held list by `motion-bench
--libx264 --tiles-only --frames`, each part by its seconds. What each part
was sent as under that rule, the regions started and the bytes of their
frames, with the stills beside them:

| Part | 1920x1080 | 2880x1800@2 |
|---|---|---|
| striped bar, 600×16 points, 6 s | 1 region, 110 kB + 2 kB | 1 region, 220 kB + 4 kB |
| text sliding, 520×28 points, 6 s | 6 regions, 179 kB + 27 kB | 4 regions, 350 kB + 153 kB |
| chart of 300×100 redrawn 5 a second | 1 region, 24 kB + 22 kB | stills, 277 kB |
| log of 400×180, 5 lines a second | 1 region, 8 kB + 98 kB | stills, 154 kB |
| notice of 280×70 sliding in and out, twice | 4 regions, 31 kB + 54 kB | 4 regions, 58 kB + 121 kB |
| clock, spinner and typing at once | 1 region, 46 kB + 100 kB | stills, 351 kB |
| video of 320×180 points, 6 s | 1 region, 522 kB + 301 kB | 1 region, 878 kB + 892 kB |

The spinners (32 and 16 points), the filling bar, the stopwatch, the clock,
the hover, the selection, the menu, the slider, the wheel's nudges and the
terminal were stills already, under 5 tiles' area or changing too seldom.
The text that slides is the worst of it: a region started again each time
it grew, 6 keyframes in 6 seconds, the text lossy throughout and settled
after.

What separates them from a video is not their size but what they cost
lossless. With no region allowed at all, each part's stills over its
seconds:

| Part | 1920x1080 | 2880x1800@2 |
|---|---:|---:|
| striped bar | 33 kB, 6 kB/s | 73 kB, 12 kB/s |
| text sliding | 274 kB, 46 kB/s | 632 kB, 105 kB/s |
| chart | 67 kB | 277 kB |
| notice | 90 kB | 173 kB |
| video of 96×54 points | 511 kB, 85 kB/s | 1493 kB, 249 kB/s |
| video of 320×180 points | 5597 kB, 933 kB/s | not run: a region at every setting below |

A bar's stills are a third of the bytes its video was, and exact; the
text's are a quarter to a third more than its regions and their stills
came to; and the video is ten times any of them or more. So a component is worth a stream
only where the stills of its tiles came to `MOVING_BYTES` over the churn's
8 tenths, the bytes each still came to shared among its tiles. Swept:

| `MOVING_BYTES` | what streams |
|---|---|
| 64 KiB | the video, and at 2880x1800@2 the sliding text, 4 regions |
| 128 KiB | the video alone, from the frame it did before |
| 256 KiB | the same |
| 512 KiB | the video, at 1920x1080 five frames later, 229 kB more of stills |
| 1 MiB | at 1920x1080 nothing, the video 5.6 MB of stills; at 2880x1800@2 the video, two frames later |

It is 256 KiB, twice the last that fails either way: about 2.6 Mbit/s of
stills, on a list for a link with room. The small video of 96×54 points
stays stills as it did, under the area.

A region that one large change began has no stills under it to count, and
was kept for as long as anything of 5 tiles' area moved in it: a window
that opened with a bar moving on it was a video of the whole window until
the bar stopped. What moves in it is now coded to see — at most 16 of the
component's tiles, a WebP each, thrown away — from the layout after its
first, half a second in, and the region is kept only where that would be
dear and is half of it or more; where it is dear and less, it ends for a
region of what moves; otherwise it ends at its second, as an idle one
does.

Over the two captures, and the samples at the same sizes (`mixed` and
`walk` whole), the rule before and now:

| Capture | bytes, MB | regions started | frames held lossless | held MSE |
|---|---:|---:|---:|---:|
| small-1920x1080 | 4.27 → 4.34 | 16 → 2 | 1447 → 1834 of 1955 | 0.09 → 0.08 |
| small-2880x1800@2 | 8.19 → 8.15 | 12 → 3 | 1581 → 1800 of 1921 | 0.05 → 0.04 |
| mixed-1920x1080 | 134.68 → 135.05 | 15 → 13 | 991 → 1058 of 3515 | 2.47 → 2.47 |
| mixed-2880x1800@2 | 273.44 → 273.16 | 20 → 17 | 1045 → 1104 of 3328 | 1.32 → 1.32 |
| window-1920x1080 | 5.16 → 4.99 | 7 → 8 | 9 → 12 | 1.26 → 1.24 |
| window-2880x1800@2 | 8.37 → 8.37 | 8 → 8 | 8 → 8 | 0.62 → 0.62 |
| video-1920x1080 | 12.06 → 12.06 | 9 → 8 | 6 → 6 | 2.18 → 2.19 |
| video-2880x1800@2 | 24.71 → 24.67 | 11 → 10 | 4 → 4 | 1.71 → 1.71 |
| walk-1920x1080 | 17.96 → 18.02 | 15 → 14 | 9 → 9 | 2.50 → 2.50 |
| walk-2880x1800@2 | 29.54 → 29.91 | 18 → 17 | 6 → 6 | 1.73 → 1.73 |
| uncover-2880x1800@2 | 0.97 → 1.54 | 1 → 2 | 2 → 3 of 49 | 1.83 → 0.62 |

The two regions left of `small` at 1920x1080 are the first picture's and
the video's. browser, drag, terminal and flood at both sizes and uncover
at 1920x1080 are the same to the byte, and every capture ends lossless.
The encoder's time is within its noise on all of them. uncover at
2880x1800@2 is the one that sends more, 0.6 MB, in two regions where it
had one: a third of the error held, for half as many bytes again.

`motion_debug` outlines each region in its frames. On lab1 (4 vCPUs of AMD
EPYC Rome), libx264, three runs each: video-1920x1080 took 2.52 s of the
processor without it and 2.54 s with, window-2880x1800@2 1.64 s and 1.63 s,
inside what the runs differ by, for 0.4% and 0.2% more bytes; and `check`
passes with it, the picture lossless once settled, the outline gone.
