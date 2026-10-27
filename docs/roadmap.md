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
