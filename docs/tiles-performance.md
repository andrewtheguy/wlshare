# Tiles performance

What the tiles encoding ([The tiles encoding](architecture.md#the-tiles-encoding))
costs the processor and the link, held and walked, against the `WLSV` stream,
and what its rules were chosen by. `examples/motion-bench.rs` in the daemon's
crate measured it, over the kept captures (`frame-samples/`; see [Capturing
what the encoder is handed](architecture.md#capturing-what-the-encoder-is-handed)).
Flood is in the figures for reference and is not a target.

## The three pictures

Measured 2026-10-26 on an eight-core AMD EPYC (Milan) VM, idle and with no
swap, the regions by libx264 and `WLSV` on four threads, over the samples from
1280×800 to 4K at a ceiling of 90, summed over the scenarios, flood left out:

- **Held**, every still a PNG: 22% of `WLSV` 4:4:4's CPU for 3.2 times its
  bytes, from 1.9 times on a terminal to 21 on a drag, every capture ending
  exact where `WLSV` ends at 42–50 dB.
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
| Tiles, held | 21.7 s | 262 MB | exact |
| Tiles, walked, at the ceiling | 26.2 s | 217 MB | 44 dB |
| Tiles, walked to 50 | 22.4 s | 93 MB | 44 dB |
| Tiles, walked to 20 | 20.9 s | 51 MB | 44 dB |

A lower ceiling takes most off `WLSV`: over both mixed sessions its bytes are
356 MB at 90, 93 at 50 and 31 at 20, where the held tiles' are 794, 431 and
313, their stills exact whatever the dial, and the walked tiles' at the
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
| Tiles, held | −8% | −15% | exact |
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
| Tiles, held | 20.0 s | 237 MB | exact |
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
