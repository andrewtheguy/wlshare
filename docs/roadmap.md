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

On *All Displays* each display is a connection of its own, with a pointer of
its own aimed at its output ([A display beside](architecture.md#a-display-beside)),
so a window dragged in the first display's tab stops at its output's edge. The zDesk
page already lets a drag past the edge through while a second display's tab is
listed, as a position past the tab's own (negative, or beyond its width), and
an RDP host or a Mac, one desktop spanning both displays, takes it onto the
second. For wlshare the gateway clamps it to RFB's unsigned 16 bits, and the
daemon holds whatever is left at the edge.

- **A position past the edge, carried signed.** A private message carries the
  pointer's position signed, so a drag left or up is carried as well as one
  right or down: a new message in `wlshare-proto`, and the protocol's version
  with it. The daemon aims every motion over the whole layout from the outputs'
  boxes ([Input and clipboard](architecture.md#input-and-clipboard)), so a
  position past a desk's output is a place in the layout like any other: on
  the output that holds it, it is aimed there on the same pointer, with the
  press held, and the compositor's move or drag follows the seat's one cursor;
  on no output, it is held at the edge, as now. The gateway sends the message,
  for a wlshare target with a second display's tab, in place of the clamped
  PointerEvent; released in order, the protocol's tag first. What it costs is
  a layout the daemon has otherwise than the compositor does (a `done` not yet
  arrived), held to the one motion past an edge; tried on labwc and Sway with
  two outputs side by side, a drag each way, before it is released.

## The pointer exact at any fractional scale

The pointer is aimed over the layout from the outputs' boxes, each output's
pixels spread over its box as a pointer mapped to the output had them
([Input and clipboard](architecture.md#input-and-clipboard)). zDesk declares a
density of 1x or 2x, which are whole, and lands every motion on its pixel. At
a fractional scale the box is the pixels over the scale truncated, so a motion
lands short of its pixel by up to two: 2593 at 2.75 on 3456 pixels is 2591.
The scale on the wire is no cure by itself: Sway applies what it is set to
rounded to the 1/120 fractional-scale-v1 carries, so a declared 1.7 is
1.69921875 on the wire and 1.7 applied, and a scale from the compositor's own
configuration (a 1.1, a 1.7) reaches the daemon rounded to 1/256.

- **The scale the pixels are laid out at.** A scale the daemon set is the
  compositor's own where it is whole in both 1/256 and 1/120, every eighth,
  remembered on the head the configuration set; any other is taken as the
  middle of what the wire's step and the box's bounds leave, within half the
  scale in pixels at the far edge. The place is then taken up to the next
  1/256 of a unit, since a client is sent it truncated to one and floors it
  (2593 at 2.75 is 942.909… units, whose step below is 2592). With it, the desk that last moved the
  cursor is aimed again when the layout changes under it, so a press without
  a motion lands where its client last pointed. Tried on branch
  `unbound-pointer`, e2e-tested on labwc and Sway with a declared 2.75 and a
  configured 1.1 and 1.7, and held back as more than 1x and 2x need; it is
  taken up when zDesk offers a fractional density.

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
