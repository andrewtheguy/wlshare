# Roadmap

Work that is planned but not started. Once something here is done, its design
moves to [`architecture.md`](architecture.md) and it comes off this list.

## The walked tiles

Improvements to [the tiles encoding](architecture.md#the-tiles-encoding) on a
list whose dial walks. Each is measured with `tiles-walk-sim` against the runs
in [Tiles performance](tiles-performance.md#the-walks-verdicts), `WLSV`
beside it on the same links.

- **The walk's verdicts on the bursts of stills.** Over the mixed session at
  5 and 10 Mbit/s, 95% of the walked tiles' changes are shown 1.5 to 2.2
  times later than `WLSV`'s, though their dial is lower and they send under
  three fifths of the link. The delay comes from the bursts of stills, which a
  lower dial does not shrink, so the walk lowers the regions' quality for
  delays they did not cause. A threshold of the tiles' own, or a burst of
  stills that is no verdict, is what to try.

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
