# Roadmap

Work that is planned but not started. Once something here is done, its design
moves to [`architecture.md`](architecture.md) and it comes off this list.

## The walked tiles

Improvements to [the tiles encoding](architecture.md#the-tiles-encoding) on a
list whose dial walks, in the order they are to be tried. Each is measured
with `motion-bench --tiles-only` against the walked logs in [Tiles
performance](tiles-performance.md#the-three-pictures), flood left out.

- **The walk's verdicts on the tiles.** The walk is `WLSV`'s thresholds and
  steps, and only `walk-sim` has played it over the tiles. Their updates are
  less even than `WLSV`'s frames, a burst of stills and then small region
  frames, so whether its verdicts are as steady is to be seen in a zDesk
  session on a shaped link before any threshold of its own is considered.
