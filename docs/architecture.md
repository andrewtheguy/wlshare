# Architecture

wlshare connects to a wlroots-based Wayland compositor through its public
protocols. It is one process with two halves and one shared framebuffer between them.

```text
wlroots compositor ── Wayland socket ──▶ compositor thread ──▶ Framebuffer ──▶ session tasks ──▶ TCP
                     screencopy           (calloop)            + damage log     (tokio)          RFB clients
                     output-management                        ◀── Commands ◀──
                     virtual keyboard/pointer
                     data-control
```

- `crates/wlshare-rfb` decides every byte on the wire: handshake, message parsing
  and building, RSA-AES and its frames, the ZRLE encoder, the density and outputs
  extensions. It has no platform dependency and its tests decode every encoder's
  output with an independent decoder written from the RFC, and run the RSA-AES
  exchange against a client written from the specification.
- `crates/wlshare` is the daemon. `compositor.rs` is the Wayland thread and its
  command handler; `capture.rs`, `outputs.rs`, `input.rs` and `clipboard.rs` are
  the protocols it speaks; `framebuffer.rs` is the shared pixels and damage;
  `session.rs` is one client; `auth.rs` checks an RSA-AES login and `pam.rs` is
  the system half of that check; `shared.rs` is what crosses between them.

## One session at a time

The desktop belongs to one client. A connection that finishes the handshake
takes it, and the client that held it is disconnected with a message naming the
one that took over — the same trade Windows Remote Desktop makes, and the reason
the takeover happens *after* the handshake: an unauthenticated connection, or
one whose login is refused, never displaces the session in progress. RFB's ClientInit
shared flag is read and dropped; there is no configuration for it and no way to
watch alongside somebody else.

The compositor thread holds the single client id, so the rule is one comparison:
input, resize, density, output selection and clipboard from anyone else are
dropped, which is what
a superseded session's last in-flight messages are. Taking over releases the
keys and buttons the previous client held and its pending resize, and capture
runs from the first handshake until the client on the desktop leaves.

Who holds it is a `watch` and not one of the broadcast events: a session slow
enough to lag the broadcast drops events, and dropping this one would leave two
clients on the desktop. A watch keeps only the latest value, so the superseded
session ends on the value it finds there — and is ignored until it does.

The watch races the whole session, not the gaps in it. A session runs its
message loop against the takeover in one `select!`, so a client that has stopped
reading its socket is cut off in the middle of the write that is blocking on it,
rather than holding its capture and its task open for as long as it refuses to
read. Client ids only ever go up, which is what lets a session decide by
comparison instead of by acknowledgement: an active id above its own is a
connection that joined after it, whether or not it ever saw itself there. That
is the answer to the other end of the race — two connections whose joins are
queued together, where the second is on the desktop before the first has
subscribed at all.

## Capture

wlr-screencopy `copy_with_damage` into a `wl_shm` buffer, one frame in flight,
paced by `max_fps`. The compositor answers a damage-only copy only when
something changed, so an idle desktop costs nothing. Damaged rectangles are
copied into the framebuffer under its lock, the generation counter advances,
and every session is woken through a `watch`.

A framebuffer holding no pixels yet is the exception, both ways round. It is
asked for a plain `copy` rather than a damage-only one, because a damage-only
copy is answered only when the output changes and an output nobody is touching
may not change for minutes -- which would leave a client on a blank screen, or
on the last picture of the output it just left, until somebody moved the mouse.
And the frame that comes back is taken whole, whatever the compositor reported
changed: damage is measured against the frame before, and a framebuffer just
made, just resized, or just pointed at another output has no frame before, so
copying only the reported rectangles would leave the rest of it blank. Both
follow from `painted`, which the framebuffer clears on every resize.

The framebuffer keeps a log of `(generation, rect)`. A session asks for the
damage after the generation it last sent and gets the merged union; a session
behind the log, or one that has seen nothing yet, gets the whole framebuffer.
Sessions copy the pixels they need out under the lock and encode after releasing
it, so encoding a slow client's update never holds up a capture.

The pointer is excluded from the frame (`overlay_cursor = 0`). On a headless
output this needs wlroots 0.19 or newer, whose headless backend keeps cursors on
a distinct plane instead of painting them permanently into the output. Every
client must advertise the standard Cursor pseudo-encoding (`-239`) before it
asks for framebuffer pixels. wlshare answers with a neutral arrow in the
client's pixel format, and the client positions that shape at the coordinates
it already sends in pointer events. Cursor motion therefore never waits for a
captured frame. wlr-screencopy exposes no application-selected Wayland cursor
surface, so the arrow is deliberately stable rather than a guessed shape.

### Frame layout

The framebuffer is always XRGB8888 with its rows top down, but a captured frame
need not be either, so `FrameLayout` records how the frame in flight differs and
the copy straightens it out. Both differences come from the compositor rather
than from anything asked of it: y-invert is reported per frame, and the single
shm format offered is whatever the renderer prefers to read back. So a
compositor on an Intel iGPU hands over red and blue the other way round from one
compositing in software, and only this copy knows it: past it a frame is
XRGB8888, rows top down, and the encoders need no cases.

`FrameLayout::bytes` is a permutation, not a conversion -- where each of the
framebuffer's four bytes sits in the frame's own pixel -- so what it can absorb
is every 32-bit order at eight bits a channel and nothing else:

| offered by the compositor | in memory  | from |
| ------------------------- | ---------- | ---- |
| `XRGB8888`, `ARGB8888`    | `B G R X`  | pixman, GLES2, Vulkan |
| `XBGR8888`, `ABGR8888`    | `R G B X`  | pixman, GLES2, Vulkan |
| `RGBX8888`, `RGBA8888`    | `X B G R`  | pixman |
| `BGRX8888`, `BGRA8888`    | `X R G B`  | pixman |

That is the whole of what wlroots' screencopy can offer for a desktop: its
GLES2 renderer resolves `GL_IMPLEMENTATION_COLOR_READ_FORMAT` to one of the
first four, its Vulkan renderer reports the texture's own format, and its pixman
renderer reports the output texture's, which adds the four with the unused byte
first.

#### What is deliberately not handled

wlroots can in principle report formats outside that table, and each would need
a real conversion into the framebuffer's eight bits a channel rather than a
rearrangement of bytes:

- **10-bit** (`XRGB2101010` and its seven siblings). Reachable on a deep-colour
  output; the one entry here with a plausible future. It needs the channels
  narrowed, and the honest version of that is its own path, not a widening of
  the permutation.
- **Packed 24-bit** (`RGB888`, `BGR888`) at three bytes per pixel, which the
  four-byte stride arithmetic in `Framebuffer::apply` assumes away.
- **16-bit** (`RGBA4444` and its siblings), which only very old GLES drivers
  report.
- **`RGB565`, `BGR565`, the 5551 family, and `XBGR16161616`(`F`)**. wlroots can
  report these, but neatvnc has no case for them either, so they are not a gap
  against wayvnc so much as a gap in every wlroots VNC server.

None of these was reachable from Sway in the measured ordinary desktop, and an
unhandled format is not silent: the capture logs what was offered and what was
wanted, and retries rather than serving a frozen picture.

## Sending pixels

A client gets an update when it has asked (`FramebufferUpdateRequest`, or once
for all with continuous updates) and there is damage. Each update is one
`FramebufferUpdate` of merged rectangles, at most 32, ZRLE-encoded on the
client's own deflate stream, or Raw before the client's first `SetEncodings` and
for a client that never lists ZRLE.

With Fence negotiated, every update ends with a fence the client echoes, and the
next update waits for the echo. One update is in flight at a time, so a slow
link is never flooded and frames coalesce in the framebuffer meanwhile. remotex
negotiates both ContinuousUpdates and Fence.

A size change goes out first, as its own update — an ExtendedDesktopSize
rectangle whose reason says who asked (the server, this client, another
client), or a DesktopSize rectangle for a client without the extension — and the
whole framebuffer follows in the next update. A client that negotiated neither
cannot be told and is disconnected at its next update rather than sent pixels at
a size it does not know. The ExtendedDesktopSize announcement that answers the pseudo-encoding is an
update too, and waits for a request like any other.

## The density extension

Standard RFB has no word for pixel density. The extension is one pseudo-encoding,
`0x574c5348` (`WLSH`), and one message type, `0xE0`, in both directions; scales
are 16.16 unsigned fixed point.

- **OutputScale**, server → client, ten bytes: type, padding, width and height
  in pixels, scale. Sent as the answer to *every* `SetEncodings` that lists the
  pseudo-encoding — the only way support is announced — and whenever the shared
  output's scale or mode changes, before the frame at the new size is captured,
  so the report precedes the resize rectangle.
- **ClientDensity**, client → server, eight bytes: type, three bytes of padding,
  the scale the client wants the output drawn at. Honoured only from a client
  that listed the pseudo-encoding and only in the range 0.5–8. The server sets
  the output's scale through wlr-output-management under the same rules as a
  resize — a headless output and `resize = true` — and **answers every
  declaration** with an OutputScale: after the
  compositor's head change, or at once with the scale as it is when the
  declaration matches, is refused, or cannot be applied. A configuration the
  compositor accepts without changing the scale is answered too: `succeeded` is
  followed by one `wl_display.sync` round trip, after which the scale is reported
  as it is if no head change arrived.

The exact scale comes from the wlr-output-management head, fractional included;
`wl_output.scale`, which wlroots rounds up, is the fallback when the protocol is
absent.

## The outputs extension

One framebuffer is one output, so a desktop with two monitors has to be asked
which one to send. Standard RFB has no word for that either — `ExtendedDesktopSize`
describes screens *inside* one framebuffer — so this is a second private
extension in the shape of the first: one pseudo-encoding, `0x574c534f` (`WLSO`),
and one message type, `0xE1`, in both directions.

- **OutputList**, server → client: type, padding, a count, the shared output's
  id, then an entry per output — id, width and height in pixels, scale as 16.16
  fixed point, a flags byte whose bit 0 says the output is headless, and a
  length-prefixed UTF-8 name. Sent as the answer to *every* `SetEncodings` that
  lists the pseudo-encoding — the only way support is announced — and again
  whenever the list, an entry, or the shared output changes. Entries are ordered
  by name, and an output whose name or mode has not arrived yet is not in them.
  The id is the `wl_output` global, unique for as long as the output exists.
- **SelectOutput**, client → server, eight bytes: type, three bytes of padding,
  the id of the output to share. Honoured only from a client that listed the
  pseudo-encoding and holds the desktop, and **answered with an OutputList**
  either way: a request naming an output the compositor no longer has, or the one
  already shared, is answered with the list as it is. So a client's menu follows
  what is on the canvas rather than what was clicked.

A switch stops the capture, points the virtual pointer at the new output —
`zwlr_virtual_pointer` takes its output when it is made and never again, so what
the client holds is let go and the pointer is remade — takes the new size into
the framebuffer blank, reports the geometry, and starts capturing again. The
client is sent nothing until a frame of the output it asked for has arrived: a
different size reaches it as an ExtendedDesktopSize rectangle with the server as
the reason, and a same-sized output as a full repaint. Which output is shared to
begin with is `output` in the configuration, or the first one.

An id is selectable exactly when it is listable: the output's properties have
arrived, it has a name, and a capture of it would produce pixels. One rule serves
both, so a client can never name something the list would not have shown it, and
an output still arriving cannot become a desktop of no size.

An output the compositor takes away while it is the shared one is not left as a
name standing for nothing — that would stop the capture with nothing left to
start it again, and a client would sit watching a picture that had quietly
stopped changing. The desktop moves to whatever the list shows first, through the
same sequence a client's own switch runs, so the client is told the new geometry
and sent the new output's pixels. With no output left the capture stops and the
list goes out empty, the last geometry and the last picture standing until an
output appears; the first one to arrive is adopted the same way.

Only a headless output is ever resized or rescaled, so switching to a real
monitor leaves a client's resize and density requests answered *prohibited* —
that monitor's mode belongs to the person sitting at it.

The choice outlives the client that made it: the next connection opens on the
output the last one asked for, not on the configured default, until the daemon
restarts. Measured on a two-monitor sway session in
[remotex's `docs/wlshare-outputs.md`](https://github.com/andrewtheguy/remotex/blob/main/docs/wlshare-outputs.md),
which is where the gateway's half of this lives.

## The audio extension

The one audio extension `rfbproto` registers — pseudo-encoding `-259`, message
type `255` submessage `1` — spoken by QEMU as a server and gtk-vnc as a client.
Nothing about it is private, which is why it was taken over a second `WLSH`-style
message: a client that already speaks it hears wlshare with nothing new to learn.

- **The announcement**, server → client: an empty pseudo-rectangle of encoding
  `-259` in a `FramebufferUpdate` of its own, sent ahead of any pixels to a
  client whose `SetEncodings` listed it. The only way support is announced.
- **Set format, enable, disable**, client → server: the sample format, channel
  count and frequency are the client's to choose, and the server converts what
  the desktop plays into them. The frequency is bounded at 192 kHz — above every
  rate real audio uses, and below where a server's own arithmetic on it starts
  to overflow.
- **Begin, data, end**, server → client. Samples are interleaved and
  little-endian — the specification is silent on the byte order, QEMU writes
  host-native and gtk-vnc reads little-endian.

`audio.rs` starts one PipeWire capture per client that enables audio, on a
thread of its own. It is a `Stream/Input/Audio` node with
`stream.capture.sink = "true"`, which connects it to the **default sink's
monitor** — what the desktop is playing, whatever is playing it — and
`node.latency` asks for 20 ms buffers. The process callback runs on that
thread's loop and not on the graph's real-time one — `RT_PROCESS` is
deliberately not set, because the callback allocates, takes a mutex and wakes a
task, and doing any of that on the data thread could stall the whole audio graph
and give every application on the host an xrun. It copies whole frames into a
sixteen-deep queue, dropping the oldest when a client cannot keep up: a dropped
buffer is a hole, and a stalled capture callback is worse. A set-format on a running stream
restarts the capture in the new format, and a disable or a disconnect stops it.

The session drains that queue before every framebuffer update, so sound is never
held behind a ZRLE frame it was ready before. PipeWire honours its own quantum
before settling on the requested one, so the first buffers of a session are
often shorter than 20 ms; every one of them is a whole number of frames. A
headless session still has a sink to capture — PipeWire's Dummy Output is one.

`audio = false` in the configuration turns the announcement off, and a client
that lists the pseudo-encoding is then told nothing.

## Resize

`SetDesktopSize` sets a custom mode on the shared output, with the same rules.
A request that arrives with `resize = false` is answered *prohibited*. A request
for the current size is answered OK at once; a size the compositor rejects is
answered *invalid layout*; a request the compositor accepts is answered when the
frame at that size arrives, with an ExtendedDesktopSize rectangle naming this
client as the reason. Only a headless output — one named `HEADLESS-*` — is ever
reconfigured.

## Input and clipboard

Key events carry X11 keysyms. The server compiles the configured XKB keymap,
uploads it to the virtual keyboard, and searches the same keymap for a keycode
producing each keysym, preferring the lowest shift level. Modifier state is
tracked with `xkb_state` and sent after every key. A keysym names a character
the client has already cased — remotex never forwards Caps Lock and sends `A` or
`a` as the browser resolved it — so before each press the server checks what the
keycode would produce under the current modifiers, and presses Shift or lets a
held Shift go around the key when the keycode alone would type the other case.
Keys and buttons are let go when the client leaves or is superseded, and a
connection that never finished the handshake releases nothing. Pointer events
arrive in framebuffer pixels and are injected as absolute positions against the
framebuffer's extent, which the virtual pointer maps onto the shared output.
Wheel "buttons" become discrete axis events.

Clipboard text from the compositor is read off the loop into a pipe and sent as
latin-1 `ServerCutText`; a selection that is cleared or stops being text is sent
as empty text. A client's `ClientCutText` becomes a data source that takes the
selection; the compositor announcing that selection back is ignored while the
source is ours. Extended Clipboard is recognised and not yet spoken.

## Security

RFB 3.8 with the configuration's types on offer: RSA-AES at both widths,
`RA2_256` first, when either `[pam]` or `[password]` is set, or None alone when
neither is. Classic VncAuth is deliberately absent: it proves knowledge of a
machine's secret, names nobody, truncates the password to eight characters, and
protects the login and nothing after it. With no login configured the session is
open and in the clear, so the listen address is a loopback or VPN address by
design.

RSA-AES is RealVNC's type as `rfbproto` documents it and TigerVNC, neatvnc and
the remotex gateway speak it: the server's RSA key and a fresh client key are
exchanged in the clear, each side seals a random to the other's key, the two
randoms derive one AES-EAX key per direction, and from there every byte in both
directions travels in frames of `u16 len || ciphertext || tag` under a counter
nonce. Inside the frames each side proves the keys it saw with a hash, the
server asks for the credentials the configured login wants — subtype 1, a
username and a password, for `[pam]`; subtype 2, a password alone, for
`[password]` — and RFB's SecurityResult, ClientInit and everything after
follow. The credentials have one shape on the wire either way, a length-prefixed
username then a length-prefixed password, and a client answering subtype 2 sends
the username empty. `crates/wlshare-rfb/src/rsa_aes.rs` has the exchange byte by
byte.

The server's key is long-lived — generated once into `rsa_key_file`, logged as
RealVNC's eight-byte fingerprint at startup — because it is the one thing a
client can pin; remotex logs the fingerprint it saw on every connection.

What checks the credentials is `auth.rs`, one of two things. `[pam]` sends them
to PAM (`pam.rs`): `pam_authenticate` and `pam_acct_mgmt` under the configured
service, nothing else. Before PAM is asked, the username must be the account the
process runs as: wlshare injects input into one user's desktop, and another
account's password must not open it. `[password]` verifies the password against
an Argon2 PHC string from the configuration, in the parameters that string
carries, and names no account at all — for a host whose desktop user has no
system password to spend on a VNC client, or no PAM stack to spend it on. The
hash is parsed at startup, so an unusable one is a startup error and not a
surprise at the first client; `wlshare hash-password` prints one. An empty
password is refused before the hash is consulted. Either check blocks — a PAM
stack may sleep, Argon2 is slow on purpose — so both run on a blocking thread. A
refusal is answered after a one-second delay with SecurityResult failed and the
bare reason "authentication failed"; the actual reason is logged.

## Deliberately absent

Tight, TightPNG, Hextile, RRE, CopyRect and every lossy encoding: the gateway
re-encodes every tile anyway, and ZRLE is the standard's best lossless choice.
8- and 16-bit pixel formats and colour maps. Application-selected cursor shapes.
Multiple outputs in one framebuffer — a client picks one of them instead. A control socket. A client's microphone: the extension carries
sound one way only.
