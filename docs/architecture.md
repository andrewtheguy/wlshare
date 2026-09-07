# Architecture

swayrx is one process with two halves and one shared framebuffer between them.

```text
sway ── Wayland socket ──▶ compositor thread ──▶ Framebuffer ──▶ session tasks ──▶ TCP
        screencopy           (calloop)            + damage log     (tokio)          RFB clients
        output-management                        ◀── Commands ◀──
        virtual keyboard/pointer
        data-control
```

- `crates/swayrx-rfb` decides every byte on the wire: handshake, message parsing
  and building, VncAuth, the ZRLE encoder, the density extension. It has no
  platform dependency and its tests decode every encoder's output with an
  independent decoder written from the RFC.
- `crates/swayrx` is the daemon. `compositor.rs` is the Wayland thread and its
  command handler; `capture.rs`, `outputs.rs`, `input.rs` and `clipboard.rs` are
  the protocols it speaks; `framebuffer.rs` is the shared pixels and damage;
  `session.rs` is one client; `shared.rs` is what crosses between them.

## Capture

wlr-screencopy `copy_with_damage` into a `wl_shm` XRGB8888 buffer, one frame in
flight, paced by `max_fps`. The compositor answers a damage-only copy only when
something changed, so an idle desktop costs nothing. Damaged rectangles are
copied into the framebuffer under its lock, the generation counter advances,
and every session is woken through a `watch`.

The framebuffer keeps a log of `(generation, rect)`. A session asks for the
damage after the generation it last sent and gets the merged union; a session
behind the log, or one that has seen nothing yet, gets the whole framebuffer.
Sessions copy the pixels they need out under the lock and encode after releasing
it, so encoding a slow client's update never holds up a capture.

The pointer is composited into the frame (`overlay_cursor`). No cursor shape is
sent; a client that lists the Cursor pseudo-encoding simply never receives one.

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

The desktop is shared unless a client clears the ClientInit flag, which
disconnects every other client, as RFB has it.

## The density extension

Standard RFB has no word for pixel density. The extension is one pseudo-encoding,
`0x53575258` (`SWRX`), and one message type, `0xE0`, in both directions; scales
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
  resize — a headless output, `resize = true`, and this client owns the layout or
  nobody does — and **answers every declaration** with an OutputScale: after the
  compositor's head change, or at once with the scale as it is when the
  declaration matches, is refused, or cannot be applied. A configuration the
  compositor accepts without changing the scale is answered too: `succeeded` is
  followed by one `wl_display.sync` round trip, after which the scale is reported
  as it is if no head change arrived.

The exact scale comes from the wlr-output-management head, fractional included;
`wl_output.scale`, which wlroots rounds up, is the fallback when the protocol is
absent.

## Resize

`SetDesktopSize` sets a custom mode on the shared output, with the same rules.
The first client to resize or declare owns the layout until it disconnects;
another client's request is answered *prohibited*, as wayvnc has it. A request
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
Keys and buttons are held per client: a client leaving releases its own and
nothing another client holds, and a connection that never finished the
handshake releases nothing. Pointer events arrive in framebuffer pixels and are
injected as absolute positions against the framebuffer's extent, which the
virtual pointer maps onto the shared output; the compositor sees the union of
every client's button mask. Wheel "buttons" become discrete axis events.

Clipboard text from the compositor is read off the loop into a pipe and sent as
latin-1 `ServerCutText`; a selection that is cleared or stops being text is sent
as empty text. A client's `ClientCutText` becomes a data source that takes the
selection; the compositor announcing that selection back is ignored while the
source is ours. Extended Clipboard is recognised and not yet spoken.

## Security

RFB 3.8 with None or VncAuth, chosen by whether `password_file` is set. VncAuth
protects the login and nothing after it, so the listen address is a loopback or
VPN address by design. RSA-AES is not implemented yet.

## Deliberately absent

Tight, TightPNG, Hextile, RRE, CopyRect and every lossy encoding: the gateway
re-encodes every tile anyway, and ZRLE is the standard's best lossless choice.
8- and 16-bit pixel formats and colour maps. Cursor shapes. Multiple outputs in
one framebuffer. A control socket. Audio.
