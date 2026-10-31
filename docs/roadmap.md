# Roadmap

Work that is planned but not started. Once something here is done, its design
moves to [`architecture.md`](architecture.md) and it comes off this list.

## Text's colour on the whole-desktop stream

`WLSV` is H.264 at 4:2:0, by the GPU through libva where there is one and
libx264 where there is none, each frame at the quantizer the walk asks
([The video encoding](architecture.md#the-video-encoding)). What is left is
text's colour, which 4:2:0 halves.

- **HEVC 4:4:4.** `VAProfileHEVCMain444` on Intel's low-power encoder, from
  Ice Lake: a second set of the headers and parameter buffers the daemon
  hands libva for H.264, not a second design. NVIDIA has no VA-API encoder
  and would be a backend of its own over NVENC, and AMD has no 4:4:4
  encoder at all. A browser decodes HEVC only where its machine's decoder
  does, and 4:4:4 on fewer, so the gateway keeps a 4:2:0 stream for the
  rest.

## The tiles' cache

What is left of what RDP's graphics pipeline does without AVC, once the
client keeps a cache of tiles ([What it was shown is a cached
tile](architecture.md#the-tiles-encoding)). Each is measured as the cache
was: `tile-recurrence` for what a rule would serve, then the tiles-only
passes of motion-bench against [the cache's
rows](tiles-performance.md#the-cache).

- **A larger cache kept as the coded stills.** 16384 slots serve 18% of the
  mixed session's stills at 2880x1800@2 where 4096 serve 13%, but as pixels
  they are 256 MiB in a tab. Kept as the stills that were sent, about
  2.5 KB a tile, they are 40 MB, decoded again on a draw, which is a hit's
  cost in the page rather than a still's on the link.

## Progressive tiles

What is left of the progressive picture, every tile of a walked list
([On a walked list, every tile
progressive](architecture.md#the-tiles-encoding)), each measured as it was:
`tiles-walk-sim` against the runs in [Tiles
performance](tiles-performance.md#progressive-tiles).

- **Motion on a slow link and a large desktop.** What costs the progressive
  picture its frame rate there is a moving region coded as wavelet tiles,
  each from nothing or as a difference of its coefficients, which a P-frame
  predicted from the frame before carries in fewer bytes. The first passes
  there are already at the stage below 50, and one at half resolution
  shrinks them by 1 to 9% more ([measured](tiles-performance.md#progressive-tiles)),
  so no stage closes it: what is left is a frame rate that falls, as the
  host's does, or a moving region carried by prediction, as the walked
  tiles' H.264 regions did before progressive tiles took their place (a 4K
  window at 5 Mbit/s at 17 frames a second where these have 7).

## Lossless tiles

What is left of the lossless picture, every tile of a lossless list a
planar bitmap in a zstd frame ([On a lossless list, every tile
planar](architecture.md#the-tiles-encoding)), each measured as it was:
the tiles-only passes of motion-bench, `--lossless`, against [the runs in
Tiles performance](tiles-performance.md#lossless-tiles).

- **One zstd stream across a picture.** Every body is a zstd frame of its
  own, so the page reads each alone and nothing is kept between them.
  Offline, one stream across a picture's bodies, flushed at each, took a
  desktop of pages nine times more off than frames of their own do (the
  browser capture at 1920×1080: 1.7 MB framed, 0.2 MB as one stream), and
  a moving picture nothing. It needs a page decoder that hands out every
  byte as its block ends, which ruzstd, the one the page reads the frames
  with, does not: libzstd itself built into the page's module, which
  builds for its shared memory as it is (zstd-safe 7 by clang 19, `cc`
  passing it the module's `atomics` and `bulk-memory`), so CI's runner
  needs clang for wasm32. A stream is coded in order, where the frames
  are packed on every core: a moving picture, which it takes nothing off,
  would be coded on one, unless the stream is the picture's stills alone
  and what moves stays framed. And the
  stream's history is state both ends keep, so a body would say where the
  stream starts over — where the picture does — and the page would ask for
  the picture whole wherever it lost one.
