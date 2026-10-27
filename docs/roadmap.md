# Roadmap

Work that is planned but not started. Once something here is done, its design
moves to [`architecture.md`](architecture.md) and it comes off this list.

## The walked tiles

Improvements to [the tiles encoding](architecture.md#the-tiles-encoding) on a
list whose dial walks, in the order they are to be tried. Each is measured
with `motion-bench --tiles-only` against the walked logs in [Tiles
performance](tiles-performance.md#the-three-pictures), flood left out.

- **A cheaper WebP.** A whole still costs a WebP about four times the fast
  PNG, which is why the walked picture at the ceiling costs 2.4 to 4.5 times
  the held one's processor where most of the desktop is still. `encode_webp`
  sets libwebp's method alone, so its analysis runs at libwebp's defaults
  (spatial noise shaping 50, filter strength 60, four segments): try its text
  preset, or noise shaping and the filter off, against what text loses. And
  code the bands of a whole picture on several threads: that costs the
  processor no less, but the update goes out sooner, which is what a walked
  picture is for.
- **Fewer stills sent twice.** A still sent below the ceiling is owed again at
  the ceiling, so walked to 20 most of a scroll's still bytes are sent twice.
  Owing it only below some distance from the ceiling, or settling at a level
  the link decides, sends fewer, for a softer picture at rest on a slow link:
  a choice of what the picture at rest is, to be made before it is measured.
- **A faster settle.** The settle sends at most 40 tiles every quarter of a
  second, so a 2880×1800 desktop's 1,305 tiles take about eight seconds, and
  a capture took up to 13. A walked list's tiles are small WebPs, so the
  count can rise while the walk stands at the ceiling.
- **The walk's verdicts on the tiles.** The walk is `WLSV`'s thresholds and
  steps, and only `walk-sim` has played it over the tiles. Their updates are
  less even than `WLSV`'s frames, a burst of stills and then small region
  frames, so whether its verdicts are as steady is to be seen in a zDesk
  session on a shaped link before any threshold of its own is considered.
