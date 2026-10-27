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
  scroll and a drag, where the stills are most of what is sent. The picture
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
