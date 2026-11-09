# Roadmap

Work that is planned but not started. Once something here is done, its design
moves to [`architecture.md`](architecture.md) and it comes off this list; once
it is measured and set aside, it comes off too, its figures kept in the
measurements' documents. What
a section puts *under consideration* is not planned: it is held back by what
it costs, which it says, until that is settled.

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

## Lossless tiles

What is left of the lossless picture, every tile that changed a planar bitmap
([On a lossless list, every tile planar](architecture.md#the-tiles-encoding)).
It is for a desktop client on a LAN, never a phone, a tablet or a page zDesk
flags as tight on memory, none of which it accommodates, so a client decoder
that takes more memory or more of the processor than RDP's planar codec is no
cost against it. Where the desktop is still it is already about a ninth of the
held picture's bytes or less; where it moves it is 6 to 12 times them, nearly
all of its 15 GB over the samples
([measured](tiles-performance.md#lossless-tiles), with what the encoder's time
went to). Each item is measured as the lossless tiles were: `motion-bench
--tiles-only --lossless` over the samples on lab1, against [Lossless
tiles](tiles-performance.md#lossless-tiles).

### Under consideration

Held back by the host's processor. Both were measured over the runs the
encoder codes, with the other codecs that came no nearer ([Other coders for
the lossless tiles' moving
runs](tiles-performance.md#other-coders-for-the-lossless-tiles-moving-runs)).

#### A median predictor ahead of zstd for the moving runs

Most of what a better codec takes off a moving picture is planar's predictor,
and a better one costs little. Each sample is predicted from the median of the
pixel left, the pixel above and their gradient, which is JPEG-LS's own
predictor, over green, red less green and blue less green, and the residuals
go to zstd as a planar bitmap does. As a scratch coder it is two thirds of
planar's bytes where the desktop moves for twice planar's time, and smaller
than planar on a page too, a terminal apart, so it might code every moving
run with no choice between two codecs. It leaves RDP's planar codec, with a
body kind in the protocol and `PROTOCOL_VERSION` for it, but it needs no
library: the coder would be the daemon's own, and the page's decoder a loop
beside the zstd it already has. What is left before it is planned: the coder
as the daemon would have it, compiled for AVX2 as planar's is and measured by
`motion-bench` over the samples, the still stream with it, and the page's
decoding in WebAssembly.

#### JPEG-LS by CharLS for the moving runs

Its cost is what holds it back: 12 to 15 times planar's processor on the host
for a little over half of planar's bytes where the desktop moves, nine points
fewer than the median predictor above for six times its time. Planar predicts
each byte from the pixel above alone, codes red, green and blue apart with no
transform between them, and leaves the entropy coding to zstd: a weak
predictor and no context. JPEG-LS is a codec built for lossless pictures, as
FLAC is for sound: a predictor from the pixels left, above and above-left, a
reversible colour transform, and Golomb-Rice coding by context, which is the
most there is to take off where the desktop moves. CharLS is its library, in
Debian and Ubuntu as `libcharls-dev`, linked from the system as libx264 and
libva are. Where the desktop is a page or a terminal it is more than planar's
bytes, which zstd's matches of what repeats take off, so it would be chosen
run by run, a moving picture JPEG-LS and the rest planar, by how little
planar's segments took off: a video's bitmaps 0.8 of their planes, a page's
0.15. It leaves RDP's planar codec, the one a Windows host draws its lossless
pipeline with, so the page needs a decoder of its own (CharLS in WebAssembly)
beside its RDP compositor's, and the protocol a body kind for it,
`PROTOCOL_VERSION` with it. A run is coded on any thread, as a planar one is,
so an update's wait is its largest run's. It is planned only once the host's
processor at the frame rate is settled, a faster coder or fewer of the runs;
then the page's decoding in WebAssembly, measured on testdesktop2's Chrome,
and a cut between the two codecs that loses nothing where the desktop is a
page.
