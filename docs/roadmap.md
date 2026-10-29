# Roadmap

Work that is planned but not started. Once something here is done, its design
moves to [`architecture.md`](architecture.md) and it comes off this list.

## The whole-desktop stream in hardware

`WLSV` is coded by libvpx on the processor, and VP9 has almost no hardware
encoder to move it to: none on NVIDIA's or AMD's GPUs, and on Intel's only
from Ice Lake to Meteor Lake and again from Panther Lake, not Lunar Lake or
Battlemage, nor the UHD 630 of testdesktop2, which decodes it alone. So VP9
goes, and `WLSV` becomes H.264, which every GPU encodes, and HEVC 4:4:4
after it, for text's colour. A host without a GPU codes it with libx264 for
now, as the tiles' regions are.

- **Its own encoder over libva.** The walk moves the quantizer every frame,
  with no keyframe, and libavcodec's `h264_vaapi` and `hevc_vaapi` fix it
  when they open. Its region-of-interest offset is no substitute: on
  testdesktop2's normal encoder it leaves a few macroblocks of every frame
  at the base, and on the low-power one it is ignored beyond about eight
  either side. Coded through libva itself — the sequence, picture and slice
  headers written by the daemon, the walk's quantizer in each slice's
  `slice_qp_delta` — every frame of a walk jumping between 18 and 51 came
  out at the quantizer asked, on both encoders, with no keyframe but the
  first, and libavcodec's software decoder read back the GPU's own
  reconstruction byte for byte. The headers and parameter buffers are all
  that is H.264's, so HEVC is a second set of them, not a second design.
- **`WLSV` on the wire is H.264.** Annex B access units in the rectangle's
  framing, the quality byte as it is; `WLSS` goes, one chroma until HEVC
  brings 4:4:4 back. A `wlshare-proto` major version, and the gateway decodes
  it with WebCodecs, as it does the regions.
- **libx264 where there is no GPU, through its own API.** The encoder found
  once at startup, as the regions' is: the hardware's where `hardware_h264`
  finds one, libx264 otherwise. Not through libavcodec, whose wrapper
  ignores a `qp` changed between frames and a region's offset alike: x264
  itself, the quantizer forced on each picture by `i_qpplus1`. That holds
  only under its CRF rate control, every frame of the same walk at the
  quantizer asked with no keyframe but the first; under CQP x264 clamps a
  forced quantizer to about three either side of its constant. libx264 is
  linked directly beside the libavcodec that already links it.
- **Without `wlshare-vp9`.** libvpx, its decoder and the planes go with it;
  the walk, the capture file and `walk-sim` move, to a crate of their own or
  the daemon, which is still to be decided. `walk-sim` on lab1, which has no
  GPU, times libx264, and the hardware's walk replays with `--encode MS`. `vp9-performance.md` is retired, and
  the stream's tests read every walk step back with libavcodec's software
  decoder, the hardware's passing with a word where there is no VA-API, as
  `h264.rs`'s does.
- **HEVC 4:4:4 after it.** `VAProfileHEVCMain444` on Intel's low-power
  encoder, from Ice Lake; NVIDIA has no VA-API encoder and would be a
  backend of its own over NVENC, and AMD has no 4:4:4 encoder at all. A
  browser decodes HEVC only where its machine's decoder does, and 4:4:4 on
  fewer, so the gateway keeps a 4:2:0 list for the rest, as `WLSS` was.

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
