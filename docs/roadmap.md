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
  ([measured](tiles-performance.md#the-walks-verdicts)). What is left to try
  is a first pass that does shrink, a burst past a size sent downscaled and
  owed the ceiling, the settle sharpening it in the pause after, as RDP's
  progressive first pass is refined; or a cap on a burst's bytes in one
  update, the rest owed to the next, which leaves the stills as late but
  lets the streams' frames go ahead of them.
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
- **A tile's difference from what the client holds.** RemoteFX
  Progressive codes a changed tile as the difference of its wavelet
  coefficients from those the client holds, which is how it carries motion
  without AVC. The stills of tiles that changed in the 0.8 s before but are
  no stream's are about two thirds of the mixed session's still bytes, yet a
  lossless difference of their pixels costs more than the still, 130 to 140%
  ([measured](tiles-performance.md#the-motion-the-stills-carry)). The
  difference would have to be lossy, of a transform's coefficients: the
  wavelet codec of the progressive refinement below, measured with it on a
  walked list, whose stills are lossy already. What a held list can try
  without one is the rule: how many of those stills a stream would carry if
  a tile were moving sooner, counted by `tile-recurrence` first.
- **Progressive refinement of the stills.** RemoteFX's progressive codec
  sends a tile coarse in one round trip and refines it in idle time by
  sending only the coefficient bits it dropped, so coarse plus refinements
  cost about the sharp tile once, where a walked list sends a WebP at the
  dial and a second at the ceiling. The browser decodes PNG, WebP and
  H.264 natively and nothing else, so it would be a wavelet tile codec of
  this repository's own, compiled once for the daemon and once to WASM for
  the gateway's page, tested by the same tests. The saving is the first
  installment alone, since the wavelet compresses no better than WebP at
  the ceiling; [what the settle sends
  again](tiles-performance.md#what-the-settle-sends-again) puts the stills
  owed for being sent below the ceiling at about 3% of what is sent walked
  to 20, so it is measured there first, and done only where that grows.
