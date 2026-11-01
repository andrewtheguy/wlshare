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

## The pointer in the layout's coordinates

A desk's virtual pointer is made against its output, and labwc does not keep
that binding: it maps every pointer back to the output its device names,
which a virtual one has none of, whenever the outputs are laid out anew and
whenever it reloads its configuration ([Input and
clipboard](architecture.md#input-and-clipboard)). The daemon remakes the
pointer on every layout change, but a reload changes no output, so the APT
repository's wlroots carries a patch that names the pointer's output.

- **A pointer bound to no output.** Made with none, a virtual pointer's
  absolute positions span the whole layout on every wlroots compositor, and
  there is no binding for a reload to lose. The daemon already holds every output's place
  and logical size from wlr-output-management, so it can send each position
  as the shared output's place plus the client's position in logical pixels,
  against the layout's extent. The patch, the rebuilt wlroots for every
  distribution, and the remaking on a layout change then go, leaving the
  layout to be followed: what costs is a layout the daemon reckons otherwise
  than the compositor does (scaled and transformed outputs, a disabled one, a
  `done` not yet arrived), and a compositor configured to map pointers to an
  output, which binds this one as well. Tried on Sway and labwc with two
  outputs, a labwc reload among them, before the patch is dropped.

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
