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
- **A scroll and a drag are a copy.** Walked to 20, a scroll and a drag still
  send 3 to 11 times `WLSV`'s bytes, nearly all of them stills: the same
  pixels moved are coded again, a region only starts once its tiles have
  changed in 4 of 8 tenths, and what it carried is owed again at the ceiling.
  A kind that copies a rectangle the client holds to another place, as RFB's
  CopyRect does, would send a scroll for a few bytes and the strip it
  uncovered. It is a new kind in `wlshare-proto`, a new `proto-v` tag, and
  zDesk's page drawing it; the held tiles gain from it too.
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
