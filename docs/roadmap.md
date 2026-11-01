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

## A drag from one display onto the other

On *All Displays* each display is a connection of its own, with a pointer bound
to its output ([A display beside](architecture.md#a-display-beside)), so a
window dragged in the first display's tab stops at its output's edge. The zDesk
page already lets a drag past the edge through while a second display's tab is
listed, as a position past the tab's own (negative, or beyond its width), and
an RDP host or a Mac, one desktop spanning both displays, takes it onto the
second. For wlshare the gateway clamps it to RFB's unsigned 16 bits, and the
daemon's bound pointer holds whatever is left at the edge.

- **A position past the edge, routed to the output it falls on.** A private
  message carries the pointer's position signed, so a drag left or up is
  carried as well as one right or down: a new message in `wlshare-proto`, and
  the protocol's version with it. The daemon holds every output's place,
  logical size and scale from wlr-output-management, so a position past a
  desk's output turns into the layout's coordinates and from there into the
  output that holds it. On the other desk's output, the motion goes out on
  that desk's pointer, in that output's pixels; on no output either desk
  shares, it stays at the edge, as now. Every pointer stays bound to its own
  output, so a reload is found as it is now. The press stays with the pointer
  that made it, and the compositor's move or drag follows the seat's one
  cursor whichever pointer moves it. The gateway sends the message, for a
  wlshare target with a second display's tab, in place of the clamped
  PointerEvent; released in order, the protocol's tag first. What it costs is
  a layout the daemon reckons otherwise than the compositor does (scaled and
  transformed outputs, a `done` not yet arrived), held to the one motion past
  an edge; tried on labwc and Sway with two outputs side by side, a drag each
  way, before it is released.

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
