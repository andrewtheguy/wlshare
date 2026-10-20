# Roadmap

Work that is planned but not started. Once something here is done, its design
moves to [`architecture.md`](architecture.md) and it comes off this list.

## A picture for a fast LAN: exact tiles, and video where it moves

A second private picture encoding beside `WLSV`, for a client on a link with
room to spare: what is still as exact PNG tiles, and each region that moves as
a VP9 stream of its own. It is a choice the gateway makes per target, not a
replacement: `WLSV`, one stream over the whole desktop walked on the link,
stays the picture for a slow link, where a lossy stream in fewer bytes is what
the link bears. This one costs bytes the link has, for less of the encoder's
time and a picture that ends exact.

- **Why here.** It is remotex's Motion (alpha, removed in its `a312b711`; its
  rules in `src/motion.rs` and `src/regions.rs` there before that commit).
  The gateway dropped it because it sat between a native protocol and the page:
  on RDP it decoded what the host had encoded only to encode it again, and on
  a Mac it was layered over ZRLE, each bound by a protocol the gateway does not
  own. wlshare owns both ends: the compositor's pixels and damage are coded
  once, and the gateway passes what it is sent.
- **Still is exact.** The desktop in 64×64 tiles from (0,0); a changed tile
  that is not moving goes as a PNG of its exact pixels, a row's run of them one
  image, drawn on its own and depending on no other.
- **What moves is video.** Motion's rules: a tile is moving when it changed in
  4 of the last 8 tenths of a second; moving tiles join into 4-connected
  components, one under 5 tiles is left to PNG, boxes merge unless the merged
  box would be more than twice the moving tiles, and at most 4 regions stream
  at once. Geometry is retuned at most every 500 ms: a region that shrinks
  keeps its stream, one that grows is rebuilt at a keyframe, and one with no
  motion for a second ends. Each region is a VP9 encoder over its rectangle
  alone, told the damage inside it as `WLSV`'s is, so the encoder's time
  follows the area that moves rather than the desktop. A region's stream is
  4:2:0, VP9 profile 0, whatever the browser decodes: what it loses of a
  colour lasts only while the region moves, since the settle makes it exact,
  and 4:2:0 is half the samples to code and every browser's hardware decoder
  takes it. A picture that starts over is seen first as one stream over the
  whole desktop, as Motion found: every tile as PNG ahead of the first pixel
  was the largest thing ever sent.
- **It settles to lossless.** Every tile a stream carried is owed an exact PNG
  once it has been still for half a second, sent after the stream's end on the
  ordered path, so a desktop that stops is exact on the page. This is the
  point of the mode, and it lands with it: the regions' 4:2:0 depends on it.
- **The gateway passes it.** A `wlshare` target lists it in place of `WLSV`
  from a key of its own; the tiles go to the page as the `TILE` records it
  draws already, and the regions as records the page draws with a VP9 decoder
  each, Motion's `REGION` records brought back. The gateway codes nothing.
- **To be measured first.** The kept captures, replayed through these rules
  against `WLSV` by `examples/motion-bench.rs` in `wlshare-rfb`: the
  encoder's CPU time and the bytes, scenario by scenario, and each picture
  decoded to check where it ends.
