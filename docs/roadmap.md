# Roadmap

Work that is planned but not started. Once something here is done, its design
moves to [`architecture.md`](architecture.md) and it comes off this list.

## The walked tiles

Improvements to [the tiles encoding](architecture.md#the-tiles-encoding) on a
list whose dial walks. Each is measured with `tiles-walk-sim` against the runs
in [Tiles performance](tiles-performance.md#the-walks-verdicts), `WLSV`
beside it on the same links.

- **Bursts of stills that shrink with the dial.** Over the mixed session at
  5 and 10 Mbit/s, 95% of the walked tiles' changes are shown 1.5 to 2.2
  times later than `WLSV`'s, though their dial is lower and they send under
  three fifths of the link. The waits are bursts of stills, mostly text,
  which a lower dial barely shrinks: a WebP of text at 20 is 62 to 67% of
  its bytes at 90. Judging them no verdict changed nothing, and coding a
  burst as one H.264 picture at the dial sent more, since a picture of
  text coded from nothing is no region's frame
  ([measured](tiles-performance.md#the-walks-verdicts)). A first pass that
  does shrink is the progressive picture's, a third of the bytes below 50
  ([measured](tiles-performance.md#progressive-tiles)); what is left to try
  on this list is a cap on a burst's bytes in one update, the rest owed to
  the next, which leaves the stills as late but lets the streams' frames go
  ahead of them.
- **A stream that settles itself.** A walked list's stream leaves its
  tiles owed a WebP at the ceiling once they are still, so a region that
  moved is sent twice, in two codecs. RDP never replaces its H.264 with
  another picture: it refines a still region inside the stream, at QP 33
  and then 25 after the 41 it moves at (38, 30 and 22 for the whole desktop,
  [zDesk's
  measurements](https://github.com/andrewtheguy/zdesk/blob/main/docs/rdp-client.md#the-connection-sequence)),
  and leaves it at what the refinement reaches, coarser than this ceiling's
  QP 18 and not the WebP's picture. Here a stream whose motion stops would
  end with P-frames over the same picture at finer quantizers, each carrying
  only what the one before left out, and its tiles owed nothing after them.
  An IDR at the finer quantizer is a picture coded from nothing again, the
  replacement in another form. What is missing is a quantizer per frame: a
  region's is its encoder's for life, and libx264 in constant-quantizer
  mode turns adaptive quantization off, so libavcodec's regions of interest
  skip its offsets (`ffmpeg`'s `addroi` at an offset of −1 left every
  P-frame at 41, with or without `aq-mode`, libavcodec 7.1 and x264 164 on
  2026-10-27), and its reconfiguration takes no new constant quantizer.
  What can carry one is a rate factor, which x264's reconfiguration does
  take, with no lookahead or macroblock tree to hold frames back, or
  `h264_vaapi`'s regions of interest where the driver takes them, not yet
  tried on the UHD 630. Leaving a stream at the ceiling owed nothing saved
  1% of the bytes ([measured](tiles-performance.md#what-the-settle-sends-again)),
  since at the ceiling its frames were already coded fine; the saving is
  below the ceiling, where the stream moves coarse and the WebP's second
  send is the most of a still region's bytes. It is measured against the
  WebP settle with motion-bench `--walk-at` and `tiles-walk-sim`: the bytes,
  the time to settle, the end dB and the CPU.

## The tiles' cache

What is left of what RDP's graphics pipeline does without AVC, once the
client keeps a cache of tiles ([What it was shown is a cached
tile](architecture.md#the-tiles-encoding)). Each is measured as the cache
was: `tile-recurrence` for what a rule would serve, then the tiles-only
passes of motion-bench against [the cache's
rows](tiles-performance.md#the-cache).

- **The cache kept across a picture that starts over.** A whole-picture
  request forgets the cache, so a reconnect or a non-incremental request
  sends every tile again as a still. With the record kept, and the page's
  slots with it, the resend is a draw of a slot for nearly every tile. The
  daemon's record is per encoder, which a size change makes anew, so the
  record would have to outlive it, and the gateway would have to promise
  that the page's slots do.
- **A larger cache kept as the coded stills.** 16384 slots serve 18% of the
  mixed session's stills at 2880x1800@2 where 4096 serve 13%, but as pixels
  they are 256 MiB in a tab. Kept as the PNG or WebP that was sent, about
  2.5 KB a tile, they are 40 MB, decoded again on a draw, which is a hit's
  cost in the page rather than a still's on the link.

## Progressive tiles

What is left of the progressive picture ([Or every tile
progressive](architecture.md#the-tiles-encoding)), each measured as it was:
`tiles-walk-sim --progressive` against the walked tiles in [Tiles
performance](tiles-performance.md#progressive-tiles).

- **In place of the adaptive tiles.** From 10 Mbit/s up, and at 1920x1080 at
  5, the progressive tiles keep the adaptive tiles' frame rate for about
  their bytes, with no video decoder in the browser, coarser while the
  desktop moves and finer once it stops. Where they do not is a large
  desktop that moves on a slow link: a 4K window at 5 Mbit/s at 7 frames a
  second where H.264 has 17. Used on real links, and that gap closed or
  judged worth it, the walked list's WebP and its H.264 streams go, and
  zDesk's Adaptive tiles with them; until then both are offered.
- **Motion on a slow link and a large desktop.** What costs the progressive
  picture its frame rate there is a moving region coded as wavelet tiles at
  a stage, which a P-frame carries in fewer bytes. A stage coarser still
  for a dial walked far down, or a first pass of a moving tile at a lower
  resolution, owed its stages once it stops, would shrink it; each is
  measured on the large desktops at 5 and 10 Mbit/s.
- **The release.** Protocol 6 is tagged `proto-v6` at the release that
  brings it, and zDesk's gateway and page module pin that tag in place of
  the local tree.
