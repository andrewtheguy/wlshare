# Architecture

English | [简体中文](architecture.zh-CN.md) | [繁體中文](architecture.zh-TW.md)

wlshare connects to a wlroots-based Wayland compositor through its public
protocols. It is one process with two halves and one shared framebuffer between them.

```text
wlroots compositor ── Wayland socket ──▶ compositor thread ──▶ Framebuffer ──▶ session tasks ──▶ TCP
                     screencopy           (calloop)            + damage log     (tokio)          RFB clients
                     image-copy-capture                       + cursor image
                     output-management                        ◀── Commands ◀──
                     virtual keyboard/pointer
                     data-control
```

- `crates/wlshare-vp9` is the VP9 encoder: libvpx pinned to a quality dial,
  8-bit 4:4:4 or 4:2:0, with the planes in front of it, its decoder, and the
  quality walk. It decides nothing about the wire, and like the protocol crate
  has no OS APIs. Its current prebuilt archives cover macOS arm64, Linux x86_64
  and aarch64, and Windows x86_64 MSVC. See
  [The VP9 encoding](#the-vp9-encoding).
- `crates/wlshare-rfb` decides every byte on the wire: handshake, message parsing
  and building, RSA-AES and its frames, the ZRLE encoder, the VP9 encoding's
  framing over its encoder, the tiles encoding, the cursor and clipboard encodings, and the
  density, outputs, audio, camera and microphone extensions and the scroll
  message. It has no platform
  dependency — the audio extension's FLAC encoder is `sound-flac`'s own, in
  plain Rust, and its Opus encoder, libopus under `sound-opus`, a prebuilt
  static archive — and its tests decode every encoder's output with an
  independent decoder, read every server message back with a client's parser,
  and run the RSA-AES exchange against a client written from the specification.
- `crates/wlshare` is the daemon. `compositor.rs` is the Wayland thread and its
  command handler; `capture.rs`, `cursor.rs`, `outputs.rs`, `input.rs` and
  `clipboard.rs` are the protocols it speaks, and `sway.rs` is sway's IPC, which
  makes and unplugs headless outputs for a client's virtual displays; `audio.rs`, `camera.rs` and
  `microphone.rs` are PipeWire's side, `decode.rs` is the camera's libavcodec decoder
  and `h264.rs` the H.264 encoder through the same libavcodec, the host's
  hardware or libx264, for the tiles picture's moving regions;
  `framebuffer.rs` is the shared pixels and damage;
  `session.rs` is one client; `auth.rs` checks an RSA-AES login and `pam.rs` is
  the system half of that check; `state.rs` tells whoever connects to the state socket
  whether the desktop is held, and `watch.rs` is `wlshare watch`, the process
  that follows it and runs a command for each; `shared.rs` is what crosses
  between the compositor thread and the sessions. Its allocator is jemalloc,
  linked statically as `malloc` itself, so the C libraries it links allocate
  through it too: glibc's per-thread arenas keep what each thread frees, and
  the zDesk gateway's footprint grew with every session on them. The arm64
  build sets jemalloc's page to 64K, which runs on the 4K, 16K and 64K
  kernels arm64 boards ship.

Its client is the zDesk gateway, which every viewer reaches the desktop
through.

## One session at a time

The desktop belongs to one client. A connection that finishes the handshake
takes it, and the connection of the client that held it is closed, the daemon
logging which one took over — the same trade Windows Remote Desktop makes, and the reason
the takeover happens *after* the handshake: an unauthenticated connection, or
one whose login is refused, never displaces the session in progress. RFB's ClientInit
shared flag is read as nothing of the kind; there is no configuration for it and no way to
watch alongside somebody else. The one connection that does not take the desktop
is the client's own second, which shows it another output
([A display beside](#a-display-beside)).

The compositor thread holds the client id of each of the two, so the rule is one
comparison: input, resize and density from anyone else are dropped, which is
what a superseded session's last in-flight messages are; output selection and
the clipboard are the desktop's client's alone, and a display beside asking to
switch is answered with the list as it is. Taking over releases the keys and
buttons the previous client held, its pending resize and queued declaration,
and ends the display beside it, and capture runs from the first handshake until
the client on the desktop leaves.

Who is where is a `watch` and not one of the broadcast events: a session slow
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

### The state socket

`state_socket` names a Unix socket on which the daemon says whether a client is
on the desktop, so the session can be rearranged around the client: sway can be
told to disable the monitors, which moves their workspaces onto the headless
output the client is shown, and to enable them again when the client is gone.
The daemon itself does nothing to the session and runs nothing: what to do is
the compositor's and the operator's, and so is noticing that the daemon has
gone, which a daemon cannot be relied on to say.

The headless output need not be there while nobody is watching. The configured
`output` is a preference: with no output of that name the daemon shares another
and moves the desktop to the configured one each time it appears — on its
appearing only, so a client that then names another output stays where it asked
to be. A client takes the desktop before its ServerInit rather than after, so
the socket says `held` while the client has still been told no size, and with
`output_wait_secs` set a session that finds no output of that name waits that
long for it: whatever follows the socket enables it, and the size the client is
told first is that output's, not a monitor's it would have been shown for a
moment and whose mode it cannot change. An output that appears is shared before
it is listed, so the wait ends the moment the output is listed, and otherwise
runs out, and the client starts on the output shared meanwhile. One that is
already there and not shared is a client's choice of another, which nothing
waiting would undo, so nobody waits for it. The desktop is held throughout, so the wait is also raced
against the client closing its connection and against a later connection taking
the desktop, either of which ends the session there. Enabling it is not the daemon's: a compositor reports a
disabled head with no position, so a daemon enabling one through
wlr-output-management would be choosing where it goes.

A connection is told where the desktop stands — `held` or `free`, one word to a
line — and again each time that changes; it says nothing itself, and only the
account the daemon runs as can connect. The words follow the desktop, not the
connections: a takeover is the desktop passing from one client to another, held
before and after, and is not a change, and a display beside holds nothing. Each
connection reads the same `watch` of seats the sessions do, so what it is told
is where the desktop stands now, however many joins and leaves happened in
between.

The end of the stream is the third thing a follower is told, and the reason
this is a socket and not a command the daemon runs. The kernel closes it when
the daemon is gone, whether it stopped, panicked or was killed where it stood,
so a daemon that never got to say the desktop was free has said so all the same.
The file is removed by a daemon that stops in order and replaced by the next one
otherwise. A socket that still answers is another daemon's: a second one
refuses to start on it rather than take its followers, and a daemon removes
the file only while it is still the one it made.

### The watcher

`wlshare watch --held COMMAND --free COMMAND` is that follower: a process of its
own, started by the session — a user unit beside the daemon's, or the
compositor's `exec` — with the session's environment, which reads the same
configuration file for where the socket is. It runs *held* when a client is on
the desktop and *free* when nobody has been for `--free-after-secs`, through
`sh -c`. A daemon it cannot reach holds nothing, and the socket is tried again
every second, so the order the two start in does not matter and neither does
which of them is restarted.

It keeps the session at a state rather than reporting transitions. The first
command it runs is the one for where the desktop stands when it starts, since
it cannot know what an earlier watcher left behind, and it runs nothing until
that is known — the daemon has said, or cannot be reached — so a watcher
started under a client never frees the desktop first; after that a command runs
only when the desktop stands otherwise than the last command said. Both
commands therefore have to be safe to run on a session already as they would
leave it. The wait before *free* is what keeps a flapping link from flapping
the monitors, and a daemon restarted by its unit from doing the same: the
client that comes back inside it, through the same daemon or the next, runs
neither command.

One command runs at a time. When it exits, the command for where the desktop
stands *now* runs if that differs from what the last one said, so a slow *held*
followed by a leave is followed by *free*, and a slow *held* followed by a
leave and a return by nothing. The desktop is watched while a command runs, so
the wait counts from when the client left and not from when the command ended.
A command still running at `--timeout-secs` is killed rather than waited on,
because the one that puts the monitors back is the one an operator is counting
on; each runs as a process group of its own and the group is what is killed, so
what the shell started goes with it; the same happens to a command whose task
is dropped under it. A command's exit status is logged and changes nothing
else.

A watcher that stops while its last command said the desktop was held — on
SIGINT or SIGTERM — runs *free* before it exits, without the wait and after any
command still running. One that is killed where it stands leaves the session as
it was, and the watcher its unit starts in its place runs the command for where
the desktop stands, which is what puts it right.

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
follow from `painted`, which the framebuffer clears on every resize, and from
the frame's size: a frame announced at a size the framebuffer does not have yet
is asked for outright too, since an idle output would otherwise never send the
frame that resizes it. A frame that reports no damage at all is also taken
whole.

The framebuffer keeps a log of `(generation, rect)`. A session asks for the
damage after the generation it last sent and gets the merged union; a session
behind the log, or one that has seen nothing yet, gets the whole framebuffer.
Sessions take the pixels they need under the lock — copied out for Raw and
ZRLE, into the encoder's own picture for tiles, converted straight into the
stream's planes for VP9 — and encode after
releasing it, so encoding a slow client's update never holds up a capture.

### The cursor

The pointer is excluded from the frame (`overlay_cursor = 0`). On a headless
output this needs wlroots 0.19 or newer, whose headless backend keeps cursors on
a distinct plane instead of painting them permanently into the output. The same
wlroots exports that plane as the pointer cursor of the output's
ext-image-capture-source, and `cursor.rs` holds an ext-image-copy-capture cursor
session on it while a client is on the desktop: its frames are the cursor image
the application under the pointer chose, in the output's pixels, which are the
framebuffer's. One frame is always in flight, and wlroots answers it only when
the cursor buffer changes — a new shape, a new scale — so a pointer that only
moves costs nothing. The session's `enter` and `leave` say whether the cursor is
on the shared output and showing; outside them there is no image. A session
needs a `wl_pointer`, which the seat refuses before it has ever had one and
turns inert when it loses one, so the session waits for the seat to name a
pointer (wlshare's own virtual pointer is one) and takes a fresh `wl_pointer`
each time it opens: on every output switch, where retargeting the virtual
pointer briefly takes the seat's pointer away.

The image is cropped to the pixels it paints and its hotspot, and goes to every
client in an update of its own whenever it changes. Every client must list
the standard Cursor pseudo-encoding (`-239`) in every `SetEncodings`, and so
before it asks for framebuffer pixels; one that also lists Cursor With Alpha (`-314`) is sent the premultiplied
RGBA as it is, Raw-encoded, and the rest get pixels in their own format beside a
mask cut at half alpha, which loses a shadow and antialiased edges. No image — a
hidden pointer, or one on another output — is an empty rectangle. RFB cursor
dimensions are framebuffer pixels, as the captured image is, so it is sent at its
own size. The client positions it at the coordinates it already sends in pointer
events. Cursor motion therefore never waits for a captured frame.

Anything that locks the output to software cursors — another client
screencopying with the cursor painted in, such as `grim -c` — takes the cursor
off its plane. wlroots then paints it into every capture of that output, this
one's included, and the cursor session reports it gone.

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

A client gets a pixel update when it has asked (`FramebufferUpdateRequest`, or
once for all with continuous updates) and there is damage. Each pixel update is
one `FramebufferUpdate` of merged rectangles, at most 32, ZRLE-encoded on the
client's own deflate stream, or Raw for a client whose list names none of
ZRLE, VP9 and tiles — or, for a client that
lists the VP9 encoding, one rectangle of the whole framebuffer
([below](#the-vp9-encoding)), and for one that lists the tiles encoding
its PNG tiles and region streams ([below](#the-tiles-encoding)).

With Fence negotiated, every pixel update ends with a fence the client echoes,
and the next pixel round waits for the echo. Cursor, audio and geometry
announcements sent with a request precede any pixels for it and have no fences
of their own. One pixel round is in flight at a time, so a slow link is never
flooded and frames coalesce in the framebuffer meanwhile. zDesk negotiates
both ContinuousUpdates and Fence.

What the echo means is the client's to decide, and zDesk holds it while the
picture is the VP9 stream until the browser has taken the frame, rather than
answering the moment it arrives ([below](#the-clients-paint)).

A size change goes out first, as its own update — an ExtendedDesktopSize
rectangle whose reason says who asked (the server, this client, another
client), or a DesktopSize rectangle for a client without the extension — and the
whole framebuffer follows in the next update. A client that negotiated neither
cannot be told and is disconnected at its next update rather than sent pixels at
a size it does not know. The ExtendedDesktopSize announcement that answers the pseudo-encoding is an
update too, and waits for a request like any other.

## The VP9 encoding

A private encoding, `WLSV` (`0x574c5356`), for the zDesk gateway: the whole
desktop as one VP9 stream, for a client that
would rather have a picture that moves than one that is exact. While the
framebuffer is within its video ceiling, zDesk lists it for every browser on a
target with `subtype = "wlshare"` and passes each frame to the browser as it came, since it is the stream zDesk would
otherwise encode from ZRLE's pixels — at the target's own dial and the
browser's chroma, which it names beside the encoding, below. A client
that does not list it is unchanged.

A client that lists it, with a quality beside it (below), gets it instead of a
standard pixel encoding, wherever in the list it is; listed without one it is
fatal. Each pixel update is then one rectangle covering the whole
framebuffer, whose body is the dial the frame was coded at, a length word and
one VP9 frame:

```text
u8 quality   the 1–100 dial the frame was coded at
u32 length   the frame's bytes
u8[length]   one VP9 frame
```

The quality is the walk's dial as the frame was coded, or the ceiling for a
settle's frame (both below): the gateway passes the frames on without decoding
them, so it is the one thing about a frame it cannot read for itself, and
what it logs at a session's end for judging a slow link by. Successive
rectangles are one stream, each frame coded against the ones before
it, so a client decodes them all with one decoder, in order. The coding is
libvpx's, as `wlshare-vp9` sets it up: the quantizer
pinned to the dial, no lag, no dropped frames, no keyframe that was not
asked for, and the colour declared in the bitstream.
`crates/wlshare-rfb/src/vp9.rs` is the framing over it — the quality byte, the length word and
its ceiling, and the framebuffer's pixels in.
What a frame holds is fixed:

- **8-bit 4:4:4, VP9 profile 1, unless the list asks for 4:2:0.** A colour
  sample per pixel: the loss 4:2:0 costs a desktop is its text's colour — a
  one-pixel coloured stem shares its sample with three pixels of background —
  and no quantizer puts it back. But no browser's hardware VP9 decoder takes
  profile 1, and a browser with no software one, Safari on iOS and iPadOS,
  refuses it, so for such a browser the gateway lists `WLSS` (below) and the
  stream is 4:2:0, VP9 profile 0, which every decoder takes.
- **BT.601 at studio swing**, converted from the framebuffer's `B, G, R, X` and
  declared in the keyframe header, so a decoder converts back with the same
  matrix. The client's pixel format does not apply.
- **A quantizer and a frame rate that follow the link.** The 1–100 dial,
  finest last, maps onto VP9's quantizer 63 down to 8, as zDesk's dial does; rate control is pinned
  to wherever the dial is, with no bitrate, no adaptive quantization and no
  dropped frames. A session starts at the ceiling the client's list names
  (`WLQ`, below), which it never goes above, and
  walks down to a floor of 20 while the client is behind — `wlshare-vp9`'s
  `walk`. The floor is a constant, not a key, as
  every adaptive stream's is: where WebRTC's quality scaler hands off from the
  quantizer to resolution and frame rate at its own threshold, this walk hands
  off to the frame rate, and the settle below sharpens a quiet desktop back at
  the ceiling.
  A frame's queueing is its fence's round trip — answered once the client has
  the frame, which for zDesk means once the browser has taken it
  ([below](#the-clients-paint)) — less the shortest of the last
  minute's, so distance does not read as queueing; a keyframe counts towards
  that floor but is no verdict. Without Fence — and for a held dial, fence or
  no fence — it is how long writing the frame blocked, 20 ms of it being behind. Two
  behind frames (60 ms of queueing or more) among the last four give quality
  up: ten points, twenty at 150 ms, thirty at 400 ms; once the dial is on the
  floor the capture's frame interval doubles instead, 33 ms up to 133 at the
  default `max_fps`, and at 400 ms both go at once. The interval is read when
  the next frame is due, so a step the last frame's fence brings paces that
  frame. The cursor, the audio announcement and a desktop that changed size
  never wait for a slowed frame's turn. A step is taken at most once a second — half a second after a step down while the lag is 150 ms or more, such a step giving quality up and leaving the frames — and while the lag
  is still falling a fifth per second from the step before — the queue it
  left draining — no further step is taken, since the step was enough; after
  a keyframe the verdicts wait two seconds, since the frames behind it queue
  behind its crossing. A second of clear frames (30 ms or less), four at the
  least, takes frames back first, then quality in steps that double from
  three to twenty-four while the link keeps taking them; a step the link
  refuses within four seconds is walked back after 300 ms to the quality it
  came from, and for fifteen seconds the walk climbs no further than halfway
  back towards the one refused. The dial
  moves on the running encoder, so a move costs no keyframe, and an encoder
  made at a new size starts where it stands. A slowed session paces its own
  frames; an unslowed one is paced by the capture and its one fence in flight.
- **What the client lists beside it says what the stream is to be**, the way
  Tight's quality levels ride `SetEncodings` — pseudo-encodings rather than a
  message, because a server that is not wlshare ignores an encoding it does
  not know where a message it does not know ends the connection, and because
  they ride the list that names the encoding, so the first frame is already
  what was asked for. `0x574c5100` plus a quality 1–100 (`WLQ` and the value) names the ceiling the
  walk never goes above; `WLSD` (`0x574c5344`)
  holds the dial there, with a walk that hears nothing in a fence — only a
  frame whose write blocked moves it, fence or no fence — and a settle only
  for what such a frame coarsened; `WLSS` (`0x574c5353`, subsampled) asks for the stream
  at 4:2:0. The
  gateway lists the first two from the target's keys, so
  `video_quality` and `render_adaptive` mean on a passed stream what they mean
  on one the gateway codes, and `WLSS` from what the browser decodes. Read at
  every `SetEncodings`: a new ceiling moves the running encoder's dial and
  sends the picture once at it, as a settle does, whether or not the desktop
  changed, and a new walk moves the dial alone. Neither costs a keyframe; a
  new chroma is a new stream, from a keyframe. A list that names VP9 without a quality is
  fatal, as a malformed message is: wlshare has no quality of its own, the
  gateway names one beside the encoding on every list, and a client that is
  not the gateway — a plain `vnc` target reads wlshare through the RFB
  baseline — lists no VP9 at all. Without `WLSD` the stream walks, and without `WLSS` it is 4:4:4.
  No lag, and threads with row and tile parallelism: the
  machine's cores less two, at most eight, for the encoder, since an encode
  is a burst the person at the other end waits on. The stream's shape follows
  the picture's width and those threads, for the decoder's sake: one tile
  column under 1440 wide, two from 1440, four from 2048, eight from 5760, never more than the
  threads, and a stream in four columns or more is coded without the loop
  filter. An inter frame that codes half the picture or more — a page
  scrolled, a video playing — is searched at libvpx's speed 9, the fastest
  its real-time VP9 has, and every other frame, a keyframe and a settle
  among them, at speed 7, the speed moving on the running encoder with no
  keyframe: on the kept captures at 4:4:4 and quality 90, a scroll or a
  video at 9 took 37–44% less of the processor a frame, for 1–15% more bytes
  at 4K and 7–37% at 1080p and at most 0.4 dB below while it moved, where a
  window dragged or uncovered, a quarter to a third of the picture, sent
  1.4 to 2.3 times the bytes at 9 and is left at 7. The conversion
  from the framebuffer's pixels to the planes is the `yuv` crate's, which
  chooses its AVX2 or NEON path at run time: the daemon is built for its
  target's baseline and asks nothing more of the processor.
- **A quiet desktop is sharpened at the ceiling, and the walk keeps its
  place.** The walk only runs when a frame goes out. Ordinarily a frame only
  goes out when something changed, so a desktop that stops right after the link
  coarsened it would keep that picture until it changed again. Once a frame
  encoded below the ceiling has been delivered — its fence answered, or
  without Fence its write finished — and nothing has been sent for 500 ms
  since, the unchanged picture goes out again, at the next update the client
  asks for, as one inter frame at the ceiling, coded whole and not by its
  damage: the residual of the blocks the client holds coarse is coded at the
  finer quantizer, and a block it holds exactly already is left as it is, so
  it sharpens the whole desktop without a keyframe
  (`a_settle_sharpens_the_picture_and_leaves_the_dial` guards that). The encoder is retuned for that one frame and returned to the
  walk's quality after it, and the frame is no verdict: the screen stopping
  says nothing about the link, and a walk that started every burst of motion
  from the ceiling was measured to put the picture half a second behind at
  every one. The clear frames before the quiet do not span it either: the
  walk's run of clear frames starts over at the settle, so a burst earns its
  step back up from its own frames rather than taking one on its first. A
  whole frame that went out at the ceiling owes nothing, and a desktop that
  goes quiet after one sends nothing; a frame of damage at the ceiling
  sharpens only the blocks it codes, so a picture coarsened before it is
  still owed its settle.
- **Keyframes only when a decoder needs one**: the first frame after the
  encoding is listed, the first at a new size or a new chroma (the encoder
  is made again for either), and the frame that answers a non-incremental request. There is
  no periodic keyframe; nothing is lost on TCP.

A normal VP9 update is sent when anything is damaged; a non-incremental request,
a quality-ceiling change and the settle described above can also owe
one. The frame is the whole picture, and what did not change costs it
nearly nothing, in bytes or in time: the session hands the encoder the damage
since the client's last frame, the encoder converts the rectangles of the damage
and codes the 16×16 blocks they touch, and every other block is left as the client
holds it, in a segment a decoder reads nothing more of. The conversion is
straight out of the framebuffer, under its lock, into the encoder's planes, and
the frame is coded from those once the lock is let go: the pixels are copied
nowhere between, unless the frames are captured. What the encoder was
measured at is beside the constants it was measured for, in
`crates/wlshare-vp9`. A keyframe,
an encoder's first frame, a settle and a client the damage log no longer
reaches back for are coded whole. The encode runs on the session's worker,
which is told it is blocking, and the fence keeps one frame in flight as it does
a standard pixel update. A `SetEncodings` that drops the encoding is answered
with the whole framebuffer in the standard encoding it selected — ZRLE when
listed, Raw otherwise — since the client is holding a lossy picture.

libvpx is in the daemon as `wlshare-vp9` links it, from `libvpx-prebuilt`'s
static archive, and it is the decoder the VP9 encoding's tests read every
frame back with (`wlshare_vp9::Decoder`). If libvpx makes no frame, nothing is
sent: the damage stays unseen and the keyframe
owed, for the frame the next change brings.

### Capturing what the encoder is handed

A daemon started with `--capture-frames DIR` writes, for every session, what
its VP9 encoder is handed, exact: each frame's time, its size, the dial at
the time, whether a keyframe was asked, the damage rectangles or that there
were none, and the rows the rectangles span as the framebuffer holds them,
`B, G, R, X`. The file is the capture file of `wlshare-rfb`'s `capture`
module, whose layout is written out there: this repository's own, read and
written here, and owing nothing to the codec, since a capture is the pixels
in front of an encoder whichever encoder that is. There is one
`<unix millis>-client<id>.wlcap` per session, written at the one call that
hands the encoder its pixels and so spanning every encoder the session makes
across resizes. Nothing on the way is lossy, which is the point: an encoder
is run again on the pictures a desktop is actually shown as and the
rectangles they came with, not on a decode of a stream. A capture is a 4:4:4
stream's alone, the chroma every use of one is about and the one its tools
play it back at (`examples/vp9cap.rs`): a session whose client asks for
4:2:0 (`WLSS`) writes no frame of it while it does. The files are
uncompressed and large — a 4K whole frame is 33 MB — and are `zstd`
compressed afterwards.

The flag is on the command line alone, with no key in the configuration, and
on purpose: a capture writes every frame of every session to disk and removes
nothing, so it is on only where somebody typed it, for a run they started by
hand and will stop. The packaged unit does not name it, and a unit that has
been given it is refused at start: systemd says which process it started in
`SYSTEMD_EXEC_PID`, and a daemon that finds its own id there with the flag
set does not run. A daemon started from a terminal inside a session systemd
started has another id and is not taken for one.

`docker/capture/` runs that without a person: a headless sway on wlroots
0.19, the daemon started by the container's script with the flag and no
authentication on loopback, and `vp9-sink` (`crates/wlshare-rfb/examples/`),
the one client the container has, which lists VP9 at a quality with Fence,
ContinuousUpdates, ExtendedDesktopSize and Cursor, sets the desktop's size, echoes
the fences, throws the frames away, on cue reads late for a while, and
types what the scenarios have it type, so the keys reach the desktop through
the daemon as a person's do. The
content is `docker/capture/scenarios/`, a script per scenario that plays on
any sway desktop and can be run by hand on one a daemon is capturing: the
quiet ones, a terminal typed into and scrolled, a page left alone, and a
window over a page moved aside and closed; the busy ones, a terminal flooded with random coloured lines
(`flood.sh`, and `busy-lines.sh`, which floods whatever terminal runs it), a
page scrolled,
a window dragged, and a video full screen, cropped to fill it, so that every
pixel is the video's: the file `--video` hands the container, which the
video and the walk need. `run.sh` plays them at each size
with a session around each — the first resizes into the size, the video
asks a keyframe midway, and the walk is the video while the sink reads late
so the fences come back late and the dial goes down, then back up, then the
video paused where it stands for the settle of a frame of it — each scenario its own session and so its own
file. A script opens its windows and says it is ready before the session
that captures it connects, and plays once that session has its first frame,
so that every capture begins with the scenario's windows drawn and not with
the desktop bare while they open: they open under a session of their own at
the size, which ends and its capture is thrown away, because the daemon's
framebuffer takes its size from a client alone and keeps its picture only
while one is connected, and that picture is what a client connecting is
first sent. The flood's session runs on past its length until it has a hundred and
fifty frames: every frame of it is the whole picture new, the slowest there
is to code, and a large desktop gets a few of them a second. A scenario that
fails, or whose sink does, leaves no capture and fails the run, so a file
named for a scenario is never something else. `scripts/capture-frames.sh` builds and runs it, against the workspace's own
encoder and nothing else. The zDesk gateway is not instrumented for
this: its Mac in Standard mode and its Windows over RDP hand it exact pixels
of the same kind of desktop, so a wlshare capture at their sizes stands for
them.

The captures are the source, and the streams a decoder is tested and timed
on are made of them: `vp9cap` (`crates/wlshare-rfb/examples/`) plays a
capture into the encoder again, each frame with its rectangles, at the dial
it was coded at, a keyframe where one was asked. So a walk's steps and its
settle are in the stream it makes without a slow link being played again,
and a change to how a desktop is coded — in `wlshare-vp9`, or a bump of its
libvpx — is a run
of `scripts/vp9-samples.sh` over the captures already kept, not a desktop
played again. The script writes what vp9-wasm's benchmark reads: for each
size a quiet and a busy sample of 120 frames, the start of the terminal's
capture and the frames in a row of the video's that said the most pixels
changed, each starting with a keyframe of the picture as it stood and named
`<scenario>-<WxH>-<tile columns>col-<lf|nolf>` from what the stream's own
keyframe header says, not from the encoder's rule. With `--streams` it also
codes every capture whole, one stream per run of a size the capture held
for thirty frames or more, as IVF beside a CSV of each frame's time, bytes,
keyframe and dial. Every stream gets the MD5 of each frame libvpx decodes of
it, from ffmpeg, and the run stops at a stream that ffmpeg's own VP9 decoder,
which shares no code with libvpx, makes other pictures of. The encoder there is given four threads
whatever the machine, since the tile columns a width is coded in are capped
by the threads and a sample must be the same stream wherever it is made.
An encoder is measured on the captures themselves, and for a quick run on
parts of them: `scripts/capture-samples.sh` cuts each capture to 120 frames
under its own name with `vp9cap trim --frames`, chosen as the samples are,
a quiet scenario's first and a busy one's busiest, the cut's first frame
the picture as it stood, whole, with a keyframe asked; `walk` it copies
whole, since its dial walks down, back up and settles over all of it.

The walk is measured on the captures the same way, without a link being
shaped: `walk-sim` (`crates/wlshare-rfb/examples/`) plays one or several of
them through the encoder and `QualityWalk` as a session does, one frame in
flight, to a model of zDesk passing the stream to a browser — the frame
handed to the browser's link as it arrives, its fence echoed at once while
nothing handed before it is unpainted and otherwise once the oldest such has
been owed past the link's ping round trip, 500 ms at most — over a link of a
given rate, changing at given times, and round trip. It says how long each
change of the desktop took to be painted, the frames a second, the dial the
motion was coded at and the link it used, and with `--timeline` every frame.
The time is the model's, so a run takes seconds; `--encode` fixes what an
encode takes, so that two walks are compared on the same frames.

### The client's paint

The walk above is only as honest as the fence it times, and a fence echoed the
moment a frame arrives times the link to the gateway and nothing else. A
browser that cannot take frames as fast as the desktop draws them would read as
a healthy client: the server would keep coding at the dial, and the person
would watch a picture that lags while every measurement said the link was
clear.

So zDesk holds each echo for what the browser's link makes of the frames
before it, measured by the `paintAck` the browser sends after its painter
draws. The path a frame takes to the screen is then inside the round trip the
walk reads, and, because the server sends nothing until the echo arrives, a
frame the browser would only have overwritten unseen is never encoded at all.
A browser that is not drawing must not be able to stop the desktop, so no echo
is held past 500 ms — zDesk's `FENCE_HOLD_LIMIT`, its paint window's grace.

## The tiles encoding

A second private picture encoding beside `WLSV`, `WLST` (`0x574c5354`), for
the zDesk gateway on a link with room to spare: the desktop as exact PNG
tiles where it is still, and a video stream of its own for each region that
moves, H.264 — the host's hardware, or libx264. It is a choice the gateway makes per target, not a replacement:
`WLSV`, one stream over the whole desktop walked on the link, stays the
picture for a slow link, where a lossy stream in fewer bytes is what the link
bears. This one spends bytes the link has, for less of the encoder's time and
a picture that ends exact. It is zDesk's Motion, which that gateway dropped
because it sat between a native protocol and the page — on RDP it decoded what
the host had encoded only to encode it again, on a Mac it was layered over
ZRLE — and which belongs here, where the compositor's pixels and damage are
coded once. `crates/wlshare-rfb/src/tiles.rs` is the encoder and its framing.

A client that lists it, with a quality beside it (`WLQ` and the value, as
VP9's), gets it instead of a standard pixel encoding, wherever in the list it
is. Listed without a quality it is fatal, and so is a list that names both it
and `WLSV`: they are two pictures, of which a client wants one. A pixel update
is then any number of rectangles of the encoding, drawn in order, each a
rectangle of the desktop whose body begins with a kind:

```text
u8 kind
0 PNG     u32 length | u8[length]                          a PNG of exactly the rectangle's pixels
1 START   u8 stream | u32 length | u8[length]              a region stream's first frame
2 FRAME   u8 stream | u32 length | u8[length]              the stream's next frame
3 END     u8 stream                                        the stream is over
```

A PNG is the desktop's pixels there exactly, 8-bit RGB or a palette of it,
depending on nothing before it. A START begins a stream on its id over its
rectangle, an H.264 keyframe of that size rounded up to even sides, a picture
a client cuts to the rectangle from its top-left; each FRAME on the id is the
next frame of that stream over the same rectangle; an END says it is over, its last picture
staying until something draws over it. A PNG over a stream's rectangle only
ever follows its END, so drawing every rectangle where it says, in order, is
the picture.

- **Still is exact.** The desktop is in 64×64 tiles from (0,0). The damage
  since the client's last update is copied out of the framebuffer under the
  lock into a picture the encoder keeps, and compared there tile by tile with
  what the client holds, so a compositor redrawing what it shows sends
  nothing. A tile that changed and that no stream carries goes as PNG, a
  row's run of them, cut into bands of 64 rows, one image. A picture that
  starts over — the encoding newly listed, a new size, a non-incremental
  request — goes the same way at once, whole, where Motion streamed it first
  and sent every tile as PNG after it anyway: on a link with room that stream
  is work for nothing. The PNG is the gateway's tile coder: a palette
  unfiltered where the pixels have at most 256 colours, RGB with each row's
  filter chosen otherwise, at `png`'s fast compression.
- **What moves is video.** Motion's rules: a tile is moving when it changed in
  4 of the last 8 tenths of a second; moving tiles join into 4-connected
  components, one under 5 tiles is left to PNG, boxes merge unless the merged
  box would be more than twice the moving tiles, and at most 4 regions stream
  at once. The regions are laid out again at most every 500 ms: one that
  shrinks keeps its stream, one that grows is started again at a keyframe, one
  with no motion for a second ends. A region is at most the desktop, which
  has the ceiling `WLSV`'s whole-desktop stream has: the gateway lists this
  encoding only for a desktop its browser's decoder takes, as it does `WLSV`,
  so wlshare holds a region to no ceiling of its own. Each region
  is an encoder over its rectangle alone, told the damage inside it as
  `WLSV`'s is, so the encoder's time follows the area that moves rather than
  the desktop. It is H.264 at 4:2:0, below, which every browser decodes, its
  hardware decoders included; what it loses lasts only while the region
  moves, since the settle makes it exact. Its dial is the one the list names, held there: there is no
  walk and no settle at the ceiling, since the picture is for a link with
  room and settles to lossless anyway.
- **H.264, which wlshare chooses: the host's hardware, or libx264.** Every
  region is H.264, by an encoder found once at startup; the client has no
  say, and a START names no codec. On a host whose GPU has an H.264 encoder a VA-API
  driver reaches — looked for with `hardware_h264 = true` in the
  configuration, the default — it is that one, which is what this picture is
  for: the processor pays for the conversion to NV12 and nothing of the
  search. On a host with none, or with the key off, it is libx264 at its
  `ultrafast` preset, which costs the processor about a third of VP9's time a
  frame for about twice its bytes, on a link that has them (measured below).
  `crates/wlshare/src/h264.rs` is that encoder, libavcodec's `h264_vaapi` or
  `libx264` through the libavcodec already linked for the camera — Debian's,
  which links libx264 itself — and the `RegionCoder` of `wlshare-rfb`'s tiles
  module, which opens a stream of it for each region. The coding
  is pinned as VP9's is: constant quantizer, the dial mapped onto QP 40 down
  to 16, no bitrate, nothing dropped, no B-frames, one frame in and one out,
  BT.601 at studio swing in the stream's VUI; the picture the region's size
  rounded up to even sides, which libx264 requires, so a region of an odd
  side, as one at the edge of an odd desktop is, is coded a pixel larger, its
  last row again below it and black to its right, which the gateway clips; each frame one Annex B access
  unit, the parameter sets ahead of every IDR, and an IDR at the stream's
  START and again after the dial moves, since a VA-API quantizer is its
  encoder's for life, and libx264's encoder is made again the same way. The
  hardware's is High profile with CABAC; libx264's is Constrained Baseline,
  which `ultrafast` comes to, tuned `zerolatency` — no lookahead, the
  picture sliced among the VP9 encoder's count of threads — with no scene
  cut, and with its version SEI ahead of a stream's first IDR, which a
  decoder passes over. The gateway announces the stream from that parameter
  set, `avc1.` and its profile and level, and the browser's decoder takes it
  in hardware where it has one. At startup the daemon tries each render node,
  the low-power encoder first — Intel's fixed-function block, which leaves
  the execution units to the desktop and codes CQP without the HuC firmware —
  and the shader one after it, then libx264, each on a frame of a small
  picture, since a driver lists more profiles than it opens; the log says
  what it found, and with nothing found the daemon does not start: a region
  has no other codec. A coder that cannot be opened for a session's tiles
  picture, refuses a stream or fails one midway ends that session, as any
  encoder's failure does. Each session opens the device itself, so a VA
  display is one thread's. The
  whole-desktop `WLSV` stream is VP9 whatever the list says: the walk's
  quantizer moves every frame, which a hardware encoder's does not.
  Measured 2026-10-21 on an Intel UHD 630's low-power encoder, in a
  container with two cores and a flooded 1280×800 terminal, 20 s at quality
  90: the H.264 regions sent 598 frames for 6 s of the daemon's CPU where VP9
  sent 176 for 17 s, both read back whole by FFmpeg's own decoders
  (`examples/tiles-sink.rs` in `wlshare-rfb` is the client that was run, and
  keeps each stream for that). Measured 2026-10-22 on a six-core machine
  with no GPU, under headless labwc, the same flooded terminal at 1280×800
  and quality 90, 20 s, every encoder on four threads: libx264 `ultrafast`
  sent 590 frames for 15–17 s of the daemon's CPU where VP9 sent 395 for
  33 s, about 27 ms of the processor a frame against 83, at about 1.8 times
  VP9's bytes a frame, 100–135 Mbit/s against 37; libx264's `superfast` and
  `veryfast`, with CABAC and more search, cost 42 s for about 535 frames, no
  less than VP9, which is why the preset is `ultrafast`. The daemon's unit
  tests code frames on libx264, on every machine, and on the hardware, and
  decode them with libavcodec's software decoder; the hardware's passes with
  a word on a machine with no encoder.
- **It settles to lossless.** Every tile a stream carried is owed an exact PNG
  once it has been still for half a second. What is owed is looked for every
  quarter of a second, 40 tiles at most at a time, each look an update that
  waits for a request and the fence as any other, so a desktop that stops is
  exact on the page, and once it is nothing more is sent.
- **Rounds as any other.** Each update ends with its fence, and one is in
  flight at a time. A new size ends every stream with no END — the size goes
  out first, and the picture whole after it — and a non-incremental request
  ends them with theirs. A list that drops the encoding is answered with the
  whole framebuffer in the standard encoding it selected.

`examples/motion-bench.rs` in the daemon's crate plays the kept captures
through this encoder and through `WLSV`, both decoded as a page would, the
tiles' updates through the client's parser and each region by libavcodec's
software H.264 decoder. Its regions are the daemon's own H.264 — `h264.rs`,
found as the daemon finds it, or libx264 with `--libx264` — so it runs where
the daemon builds. The figures below were measured when the regions were VP9
by libvpx at speed 9, before they were H.264. The rules beyond Motion's were chosen by
it: told its damage rather than coded whole, a stream's encoder costs what
moves; at speed 9 motion took 44–72% of `WLSV`'s CPU at 4:2:0 and speed 7,
against 73–98% at speed 7, the picture while it moved at most 1.3 dB below
speed 7's and mostly within half a dB; a first picture as PNG rather than a
stream took 1–15% off the CPU over a capture, and nine tenths off a
terminal's; a band no stream touches sent as its changed tiles rather than
whole sent 3–15% fewer bytes where the damage is loose. Against `WLSV` at
4:2:0 and speed 7, at 1080p and 4K: typing takes 3% of its CPU; motion
44–72%, nearly all of that saving speed 9's, since `WLSV` at speed 9 takes
about the tiles' CPU; the picture while it moves is up to 3 dB better and at
worst 0.6 dB below, a terminal's exact throughout where `WLSV`'s is 53 dB;
every capture ends exact where `WLSV` ends at 23.9–52.7 dB, and the bytes are
1.2–13 times `WLSV`'s: up to 53 Mbit/s, and 95 and 129 for a desktop flooded
with change at 1080p and 4K. The ratios hold from 1280×800 to 4K. Its `check`
holds the encoder to sending nothing once settled, through 5 s of quiet and
10 redraws of the pixels the page already has, and to ending exact; the
`still` captures are for it and no measurement.

## The density extension

Standard RFB has no word for pixel density. The extension is one pseudo-encoding,
`0x574c5348` (`WLSH`), and one message type, `0xE0`, in both directions; scales
are 16.16 unsigned fixed point, so `0x0002_0000` is 2.0 and `0x0001_8000` is 1.5.
Both messages have one layout:

| Offset | Type | Field |
|---|---|---|
| 0 | U8 | message type, `0xE0` |
| 1 | U8 | padding |
| 2 | U16 | width, pixels |
| 4 | U16 | height, pixels |
| 6 | U32 | scale, 16.16 fixed |

- **OutputScale**, server → client, ten bytes: type, padding, width and height
  in pixels, scale. Sent as the answer to *every* `SetEncodings` that lists the
  pseudo-encoding — the only way support is announced — and whenever the shared
  output's size or scale changes, another output becoming the shared one
  included, before
  the frame at the new size is captured, so the report precedes the resize
  rectangle.
- **ClientDensity**, client → server, ten bytes in OutputScale's layout: type,
  padding, width and height in pixels, scale. The client states the scale it
  wants the output drawn at *and* the size it wants at that scale, every time,
  so a change of density is one output configuration: a scale alone would change
  the logical size until a resize followed, and every application would redraw
  twice. A resize at an unchanged density is `SetDesktopSize`. Honoured only from
  a client that listed the pseudo-encoding and ExtendedDesktopSize, only in the
  range 0.5–8 and at a size that is not empty. The server sets the output's mode
  and scale through wlr-output-management in one configuration, asking only for
  what differs, under the same rules as a resize — a headless output and
  `resize = true` — and **answers every declaration** with an OutputScale: after
  the compositor's head scale change, or at once with the output as it is when the
  declaration matches, is refused, or cannot be applied. A configuration the
  compositor accepts without changing the scale is answered too: `succeeded` is
  followed by one `wl_display.sync` round trip, after which the output is
  reported as it is if no scale change arrived. One declaration's configuration
  is out at a time, and its events are told from any other's, so each is
  answered once: one arriving meanwhile waits for it to settle, and a newer one
  replaces it, the replaced one answered with the output as it is. A
  configuration the compositor *cancels* because the layout changed under it —
  an output added or taken away, which a client's virtual displays do — is made
  once more against the new layout, once; a second cancellation is a refusal.
  A new size
  reaches the client as
  an ExtendedDesktopSize rectangle whose reason is this client, as a
  SetDesktopSize's does.

The exact scale comes from the wlr-output-management head, fractional included;
`wl_output.scale`, which wlroots rounds up, is the fallback when the protocol is
absent.

## The outputs extension

One framebuffer is one output, so a desktop with two monitors has to be asked
which one to send. Standard RFB has no word for that either — `ExtendedDesktopSize`
describes screens *inside* one framebuffer — so this is a second private
extension in the shape of the first: one pseudo-encoding, `0x574c534f` (`WLSO`),
and one message type, `0xE1`, in both directions.

- **OutputList**, server → client: type, the most virtual displays the server
  makes on request ([Virtual displays](#virtual-displays)), a count, the shared output's
  id, then an entry per output — id, width and height in pixels, scale as 16.16
  fixed point, a flags byte whose bit 0 says the output is headless, and a
  length-prefixed UTF-8 name. Sent as the answer to *every* `SetEncodings` that
  lists the pseudo-encoding — the only way support is announced — and again
  whenever the list, an entry, or the shared output changes. Entries are ordered
  by name, and an output whose name or mode has not arrived yet is not in them.
  The id is the `wl_output` global, unique for as long as the output exists and
  opaque to the client. The name is the compositor's own, `DP-2` or
  `HEADLESS-1`, and the headless flag marks an output the compositor made rather
  than a monitor somebody is sitting at.

| Offset | Type | Field |
|---|---|---|
| 0 | U8 | message type, `0xE1` |
| 1 | U8 | virtual displays: the most the server makes, 0 for none |
| 2 | U16 | count |
| 4 | U32 | the shared output's id |

  then `count` entries:

| Offset | Type | Field |
|---|---|---|
| 0 | U32 | id |
| 4 | U16 | width, pixels |
| 6 | U16 | height, pixels |
| 8 | U32 | scale, 16.16 fixed |
| 12 | U8 | flags — bit 0: headless |
| 13 | U8 | name length |
| 14 | U8[] | name, UTF-8 |

- **SelectOutput**, client → server, eight bytes: type, an operation byte of
  0, two bytes of padding, the id of the output to share. Ignored from a client
  that did not list the pseudo-encoding, honoured only from the client on the
  desktop, and **answered with an OutputList** either way, a display beside
  included: a request naming an output the compositor no longer has, or the one
  already shared, is answered with the list as it is. So a client's menu follows
  what is on the canvas rather than what was clicked.

| Offset | Type | Field |
|---|---|---|
| 0 | U8 | message type, `0xE1` |
| 1 | U8 | operation, 0 |
| 2 | U8[2] | padding |
| 4 | U32 | id |

- **VirtualDisplays**, client → server, eight bytes in the same layout with an
  operation byte of 1 and a count in place of the id: have this many virtual
  displays ([Virtual displays](#virtual-displays)). Heard under SelectOutput's
  rules and answered with the list as it is; what it brings about arrives as
  the outputs come and go, a list each.

| Offset | Type | Field |
|---|---|---|
| 0 | U8 | message type, `0xE1` |
| 1 | U8 | operation, 1 |
| 2 | U8[2] | padding |
| 4 | U32 | count |

A switch stops the capture, takes the new size into the framebuffer blank,
points the virtual pointer at the new output — `zwlr_virtual_pointer` takes its
output when it is made and never again, so what the client holds is let go and
the pointer is remade — reports the geometry and the list, and starts capturing
again. The
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
stopped changing. The desktop moves to the configured `output` when the
compositor has it and otherwise to whatever the list shows first, through the
same sequence a client's own switch runs, so the client is told the new geometry
and sent the new output's pixels. With no output left the capture stops and the
list goes out empty, the last geometry and the last picture standing until an
output appears; the first one to arrive is adopted the same way.

### Virtual displays

An RDP client's monitor layout and a Mac's display configuration tell the host
how many displays the session has, and the host makes them. `VirtualDisplays` is
that request: the client on the desktop says how many virtual displays it wants
— one or two, one per desk, since an output is shown on one desk and there are
two — and the daemon has the compositor make headless outputs, or unplug them,
until there are that many. The client on the desktop alone is heard; a display
beside's request, like its `SelectOutput`, is answered with the list as it is.

Only sway can be asked. No Wayland protocol a client binds makes an output, and
sway's IPC does: `create_output` makes a headless output and `output NAME
unplug` destroys one, a startup one included. The daemon finds the socket in
`SWAYSOCK`, which sway sets for everything it starts and the session's user
manager carries to its units, and checks at startup that it answers as sway;
there is nothing to configure. Every OutputList says in its second byte the most
virtual displays the daemon makes — two with sway, none without — so a client
knows before it asks, and a request the daemon cannot act on, or one for a count
it does not make, is answered with the list as it is and changes nothing. On
labwc or another compositor the outputs are the ones the compositor has.

One step at a time. The request is answered with the list as it stands, and what
it brings about arrives as the outputs do: a `create_output` is sent, its output
appears and is listed, and only then is the next sent; an unplug likewise waits
for its output to go. Each arrival and departure is a list of its own, so a
client asking for two on a session with one sees the second output in the list
that follows and, in the zDesk gateway, opens its tab. The count is of
headless outputs, whoever made them: a session started with two
(`WLR_HEADLESS_OUTPUTS=2`) and asked for one loses one, as a Windows host with
two monitors asked for one does. The output the desktop is on is never
unplugged; of the rest the latest made goes first — the highest number, since
wlroots counts up for the life of the compositor, so an output made after one
was unplugged is a new one under a new name, never the old one back — and the
configured `output` last. A refusal from sway is logged and ends the request.

The request is the client's. It is let go when the client leaves or is taken
over, and the outputs stand as they are until the next client says, so windows
stay where they were across a reconnect, as on a Windows host. A headed
session's watcher can unplug what is left in its *free* command.

### A display beside

One framebuffer is one output and one connection is one framebuffer, so a client
that wants two outputs at once connects twice. Its second connection finishes
the same handshake, login included, and sends `0xB5` as its ClientInit byte
where RFB has a shared flag. It then does not take the desktop: the client on
it stays, and this connection is shown the first output the list has that the
client is not on. Every other ClientInit value takes the desktop, RFB's `1`
included.

It is a whole RFB session over that output: its own framebuffer and capture,
its own cursor session, a virtual pointer made against its output, its own
encoding — VP9 with a walk of its own link, where it lists it — and its own
resize and density, under the rules of the output it is on. Its ServerInit
names that output's size, so it is sent only once the output is its own. The
keyboard is the seat's, and keys from either connection reach whatever the
compositor has focused; what each holds is let go when it leaves, and nothing
the other holds. A key both hold goes up when the last of them lets go of it.
The clipboard is the desktop's and is set by the client on
it alone.

What it may not do is choose. Which output is where is the client's on the
desktop: the connection beside is sent an `OutputList` naming its own output
as the shared one if it lists the extension, and its `SelectOutput` is answered
with that list as it is. It ends, its socket closed, when

- the client on the desktop leaves or is taken over — a display beside is that
  client's, and a new client starts with none;
- the client on the desktop selects the output it shows, since an output is on
  one of them;
- another connection asks to be beside, which takes its place;
- the compositor takes its output away.

One that asks with nobody on the desktop, or with no output the client is not
on, is closed before ServerInit. So is one whose handshake finished after a
later connection's display beside had already ended: it takes nobody's place.
And so is one that connected before the client now on the desktop did: it was
opened beside whoever was there before.
There is one beside, so two outputs at once is
the most a client is shown. The zDesk gateway opens it for the second
display's browser tab on *All Displays*, and lists on it the pixel encodings,
the cursor, the size and the density and nothing else.

Only a headless output is ever resized or rescaled, so switching to a real
monitor leaves a client's resize requests answered *prohibited* and its density
declarations answered with the output as it is —
that monitor's mode belongs to the person sitting at it.

The choice outlives the client that made it: the next connection opens on the
output the last one asked for, not on the configured default, until the
configured output next appears or the daemon restarts. Measured on a two-monitor sway session in
[zDesk's `docs/wlshare-outputs.md`](https://github.com/andrewtheguy/zdesk/blob/main/docs/wlshare-outputs.md),
which is where the gateway's half of this lives.

## The audio extension

The desktop's sound, as FLAC or as Opus, on the connection the pixels use. It
is private — pseudo-encoding `0x574c5346` (`WLSF`) and server message type
`0xE4` — and its client is the zDesk gateway. Which codec is the client's to
ask for, as the format is, and nothing here configures it: FLAC, unless the
client lists `0x574c4f50` (`WLOP`) beside the encoding. The client's messages and the stream's
begin and end are borrowed from the QEMU Audio extension `rfbproto` registers,
message type `255` submessage `1`; QEMU's pseudo-encoding, `-259`, is not
spoken, because what it promises is raw samples and none are sent. A client that
lists only `-259`, gtk-vnc for one, hears nothing.

- **The announcement**, server → client: an empty pseudo-rectangle of encoding
  `WLSF` in a `FramebufferUpdate` of its own, sent ahead of any pixels to a
  client whose `SetEncodings` listed it. The only way support is announced.

| Offset | Type | Field |
|---|---|---|
| 0 | U16 | x, 0 |
| 2 | U16 | y, 0 |
| 4 | U16 | width, 0 |
| 6 | U16 | height, 0 |
| 8 | S32 | encoding, `0x574c5346` |

- **Set format, enable, disable**, client → server, QEMU's messages: the sample
  format, channel count and frequency are the client's to choose, and the
  server converts what the desktop plays into them. The formats are QEMU's
  codes 0–3, U8, S8, U16 and S16; its 32-bit codes are refused, because FLAC
  stores at most 24 bits. The frequency is bounded at 8 kHz, the lowest rate
  real audio uses, and at 96 kHz, twice what the desktop's own graph runs at.
  A code, a channel count other than 1 or 2, or a rate outside those is fatal. Four bytes for an enable or a disable,
  ten for a set-format:

| Offset | Type | Field |
|---|---|---|
| 0 | U8 | `255` |
| 1 | U8 | `1` |
| 2 | U16 | operation: 0 enable, 1 disable, 2 set format |
| 4 | U8 | sample format (set format only): 0 U8, 1 S8, 2 U16, 3 S16 |
| 5 | U8 | channels, 1 or 2 |
| 6 | U32 | frequency |

- **Opus**, client → server: the pseudo-encoding `WLOP`, listed beside `WLSF`,
  asks for the sound as Opus in place of FLAC. A pseudo-encoding rather than a
  message, as the VP9 stream's choices are, so it rides the list that asks for
  the sound at all. A list that changes the codec while a stream runs restarts
  the stream in the new one, between an end and a begin.
- **Set bitrate**, client → server, operation `3` beside QEMU's three and
  wlshare's own: the rate Opus is coded at, in bits per second, 6 000 to
  510 000, libopus's bounds; a rate outside them is fatal. An Opus stream has
  no rate of its own: one must have been set before its enable, and an enable
  without one is refused with a log line and no begin, as one at a frequency
  Opus does not code is. It may come again while the stream runs, and the
  running stream moves to it at its next packet with no restart. A FLAC stream
  has no rate to move.

| Offset | Type | Field |
|---|---|---|
| 0 | U8 | `255` |
| 1 | U8 | `1` |
| 2 | U16 | operation, `3` |
| 4 | U32 | bits per second |

- **Begin and end**, server → client, QEMU's messages:

| Offset | Type | Field |
|---|---|---|
| 0 | U8 | `255` |
| 1 | U8 | `1` |
| 2 | U16 | operation: 0 end, 1 begin |

- **A frame**, server → client, between a begin and an end — one FLAC frame, or
  one Opus packet:

| Offset | Type | Field |
|---|---|---|
| 0 | U8 | message type, `0xE4` |
| 1 | U8[3] | padding |
| 4 | U32 | length of the frame |
| 8 | U8[] | one FLAC frame or one Opus packet |

Every FLAC frame holds exactly `frequency / 50` frames of samples, rounded down —
twenty milliseconds, 960 at 48 kHz — in fixed-blocking mode, and is numbered
zero. The FLAC stream header, `STREAMINFO`, is never sent: everything
in it is already agreed, so a client builds it from the format it set, with
that block size as both minimum and maximum. A frame states its own rate unless
no frame header has a code for it, as for 70001 Hz, and then leaves it to that
header. An unsigned format has the top
bit of every sample flipped before it is encoded, which maps its range onto the
signed one of the same width with silence on zero; the client flips it back.
Decoded samples are interleaved and little-endian, and bit for bit what the
capture produced.

FLAC is lossless, while music costs about half of its 1.5 Mbit/s PCM rate,
speech under a third and a silent desktop a few bytes a frame. A frame length past 64 KiB is fatal to a
client: the largest block there is, 20 ms of 16-bit stereo at 96 kHz, is 7680
bytes before compression.

The encoder is spoken to in one place:
[sound-flac](https://github.com/andrewtheguy/sound-flac), a repository of
its own that this workspace and the zDesk gateway, for the FLAC it codes of an
RDP host's sound, each pin by release tag. The encoder is that crate's own, in
plain Rust, tuned for sound heard as it is made: no C is compiled, nothing is
downloaded or loaded at run time and the package depends on no FLAC library.
It makes one frame of every block the moment the block is whole, with nothing
of the blocks before it, so a frame lost on the way costs its own samples and
nothing after, which is why every frame is numbered zero. No marker or stream
header is sent: a client builds the header from the format it set.
`wlshare-rfb`'s tests read every frame back with symphonia's decoder, which
shares nothing with it.

As Opus, every packet is twenty milliseconds too, `frequency / 50` frames, and
the frequency must be one Opus codes at — 8, 12, 16, 24 or 48 kHz — or the
enable is refused with a log line and no begin. The sample format says only
what the capture hands the encoder, an unsigned sample flipped as for FLAC and
an 8-bit one widened to sixteen: what a decoder gives back is its own business.
No `OpusHead` is sent. A client builds it from the format it set, with a
pre-skip of 312, the encoder's lookahead in 48 kHz samples at every frequency.
A packet decodes from the ones before it and the wire numbers none, so a client
could not tell one was missing: wlshare drops no packet it coded, and a client
that fell behind loses sound that was never coded instead.

It is the choice of a client whose own listener takes Opus: the zDesk gateway
hands each packet to the browser as it came, where it would otherwise decode
the FLAC and code Opus itself, and sends the rate the walk of the browser's
link arrives at as a set-bitrate: sound-opus's `walk`, the one rate walk
there is, which the gateway runs for the sound it codes itself and for this. The encoder is libopus, spoken to in one
place as the FLAC encoder is: [sound-opus](https://github.com/andrewtheguy/sound-opus),
which this workspace and the gateway, for the sound it codes itself, each pin
by release tag, so a packet made here and one made there are the same stream —
music-tuned, constrained variable rate around the bitrate, at full effort, with
silence a few bytes a packet. libopus is linked from a prebuilt static archive
its sys crate downloads: no C is compiled,
nothing is loaded at run time and the package depends on nothing for it.
`wlshare-rfb`'s tests wrap the packets in an Ogg stream behind the header a
client builds and decode them with FFmpeg's own Opus decoder, which shares
nothing with libopus, so they need `ffmpeg` on the path.

While any client listens the host is silent, the way a remote desktop's sound
is: the desktop plays into the **speaker**, a sink of wlshare's own, rather than
into the host's. `audio.rs` makes it with the first client's enable and removes
it with the last one's disable or disconnect — a `support.null-audio-sink` named
`wlshare-speaker`, described as "wlshare remote audio", on a thread of its own.
Nothing on the host is changed to get there, no sink muted and no default
written. The speaker's `priority.session` is 100000, and WirePlumber makes the
available sink with the highest priority the default, after adding 30000 to the
one the user configured and up to 20000 to those configured before it; a
hardware sink's own is in the low thousands, so the speaker is the default
while it exists, chosen sink or not, and every stream that follows the default
moves to it. The node belongs to the speaker thread's PipeWire connection, so
when the thread quits or the daemon dies PipeWire removes it, WirePlumber makes
the host's sink the default again, and the streams follow it back. A stream an
application pinned to a sink of its own stays there and is heard on the host.

`audio.rs` starts one PipeWire capture per client that enables audio, on a
thread of its own. It is a `Stream/Input/Audio` node with
`stream.capture.sink = "true"` and `target.object` the speaker, which connects
it to the **speaker's monitor** — what the desktop is playing, whatever is
playing it — and `node.latency` asks for 20 ms buffers. The process callback runs on that
thread's loop and not on the graph's real-time one — `RT_PROCESS` is
deliberately not set, because the callback encodes, allocates, takes a mutex
and wakes a task, and doing any of that on the data thread could stall the whole
audio graph and give every application on the host an xrun. Encoding there
keeps it off the session's task, which has pixels to compress; a 20 ms buffer
takes a fraction of a millisecond. The encoder keeps what does not fill a frame
for the next buffer — PipeWire honours its own quantum before settling on the
requested one, so the first buffers of a session are often shorter than 20 ms,
512 frames where 960 were asked for —
and queues each frame it completes in a sixteen-deep queue. When a client cannot
keep up, FLAC drops the oldest: each frame decodes on its own, so a dropped one
is a 20 ms hole, and a stalled capture callback is worse. Opus leaves a buffer
uncoded while the queue is full instead, so the decoder is handed every packet
the encoder made and the two stay in step across the hole. A set-format on a
running stream, or a list that changes its codec, restarts the capture as now
asked for, holding the speaker
across so the host is not heard between the two captures, and a disable or a
disconnect stops it; what is left of a frame goes with it.

The session drains that queue before every framebuffer update, so sound is never
held behind a pixel update it was ready before. A headless session needs no sink
of its own: the speaker is one.

The announcement is off unless the configuration sets `audio = true`; while it
is off, a client that lists the pseudo-encoding is told nothing.

## The camera extension

A client's camera, lent to the desktop. RFB carries nothing from a client but
input and a clipboard, and no registered extension carries video that way, so
this is a third private pair in the shape of the density and outputs extensions:
pseudo-encoding `0x574c5343` (`WLSC`) and message type `0xE2`, in both
directions. Every message is the type, an operation, two more bytes, and what
the operation carries; integers are big-endian.

| Direction | Operation | Bytes 2–3 | Then |
| --------- | --------- | --------- | ---- |
| client → server | 0, plug | padding | `u16` width, `u16` height, `u32` frame-rate numerator, `u32` denominator |
| client → server | 1, unplug | padding | nothing |
| client → server | 2, sample | flags (bit 0: keyframe), padding | `u32` length, one Annex B access unit |
| server → client | 0, available | padding | nothing |
| server → client | 1, start | padding | the plugged format, as a plug lays it out |
| server → client | 2, stop | padding | nothing |
| server → client | 3, keyframe | padding | nothing |

- **Available** answers *every* `SetEncodings` that lists the pseudo-encoding —
  the only way support is announced.
- **Plug** makes a camera of the H.264 the client will send; another plug
  replaces it, and an unplug or the client leaving removes it. A plug with no
  pixels or no rate, and a sample over 4 MiB, are fatal: they are a client that
  means something else by the fields. A plug of more pixels than 4096x2304 —
  H.264 level 5.2's largest frame, 36 864 macroblocks — is refused, logged, and leaves the client without a
  camera: the fields reach 65535x65535, whose pictures no buffer should hold.
- **Start** and **stop** are the desktop's decisions, not the client's: an
  application opened the camera, or the last one closed it. The client sends
  samples between the two and nothing outside them, and a stream opens on a
  keyframe.
- **Keyframe** is owed after a gap: H.264 cannot be decoded across a lost unit.

`camera.rs` makes each plugged camera a PipeWire node, `wlshare-camera-<client>`,
of class `Video/Source` and role `Camera`, described as "wlshare remote camera"
so it is not mistaken for a camera of the host's own. Like an audio capture it is
a thread of its own running PipeWire's loop, and its callbacks run on that loop,
not on the graph's real-time thread, because they decode and copy. It offers one
format — I420 at the plugged geometry and rate, what one decoder behind it makes
— and an application that wants another converts or does not open it. The node is
its own driver: nothing else in the graph knows when a camera frame is due, so
each decoded picture triggers the cycle that delivers it. The stream going to
*streaming* when an application links to it, and back when the last one leaves,
is what the session sends as start and stop.

`decode.rs` is the system's libavcodec, reached through `ffmpeg-sys-next` with
avcodec alone and linked dynamically, so the package depends on Debian's
`libavcodec` rather than carrying a codec. The decoder runs on one thread with
`LOW_DELAY`, so a unit is a picture the moment it is decoded rather than a frame
later. It takes 4:2:0 at eight bits, which is everything Constrained Baseline
makes. A picture at another size than the plug named is one the node cannot
offer, and ends the camera: it is said once, nothing more is decoded, the client
is sent stop, and the camera is unplugged.

Samples reach the camera thread through a queue eight deep. One that finds it full
is dropped, and so is every sample after it until a keyframe, which is asked of the
client once per gap — again if the keyframe itself found no room. A unit the
decoder refuses owes a keyframe the same way. Nothing here waits on the client's
socket: a desktop that stops watching costs the client nothing but a stop.

Measured 2026-09-14 on a sway session with PipeWire 1.4.2: a client plugging
640x480 at 15/1 and sending libx264 Constrained Baseline, and a PipeWire consumer
linking to `wlshare-camera-1` and offering YUY2, I420, NV12 and BGRx. The consumer
negotiated I420 640x480 at 15/1 and went to *streaming*, wlshare sent start, and
the consumer took whole pictures — 460800 bytes, stride 640 — until it left and
wlshare sent stop.

An application finds the node through PipeWire, and a browser through
xdg-desktop-portal's Camera interface, which hands it a PipeWire remote that
shows only nodes of role `Camera`. The portal exports that interface only when a
backend implements Access, to ask the user; `xdg-desktop-portal-wlr` does not,
and `xdg-desktop-portal-gtk` does. The portal finds its backends when it starts,
so a running one must be restarted after a backend is installed. Chrome also
reaches cameras through PipeWire only with
`chrome://flags/#enable-webrtc-pipewire-camera` enabled; without the flag, or
without the portal's Camera interface, it looks at `/dev/video*` alone and lists
no camera. Checked 2026-09-13 with Chrome 153 on a labwc session with
xdg-desktop-portal 1.20.3: with only `xdg-desktop-portal-wlr` installed, the
portal exported no `org.freedesktop.portal.Camera`, and with
`xdg-desktop-portal-gtk` added and the portal restarted, it did.

The announcement is off unless the configuration sets `camera = true`; while it
is off, a client that lists the pseudo-encoding is told nothing.

## The microphone extension

A client's microphone, lent to the desktop — the camera's twin, and what an RDP
host gets from MS-RDPEAI. The QEMU Audio extension carries sound one way only, so
this is a fourth private pair: pseudo-encoding `0x574c534d` (`WLSM`) and message
type `0xE3`, in both directions, every message the type, an operation, two more
bytes, and what the operation carries; integers are big-endian.

| Direction | Operation | Bytes 2–3 | Then |
| --------- | --------- | --------- | ---- |
| client → server | 0, plug | padding | nothing |
| client → server | 1, unplug | padding | nothing |
| client → server | 2, sample | padding | `u32` length, interleaved PCM |
| server → client | 0, available | padding | nothing |
| server → client | 1, start | padding | `u16` channels, `u16` padding, `u32` frequency |
| server → client | 2, stop | padding | nothing |

- **Available** answers *every* `SetEncodings` that lists the pseudo-encoding —
  the only way support is announced.
- **Plug** makes a microphone; another plug replaces it, and an unplug or the
  client leaving removes it. A sample over 256 KiB is fatal, and so is one that
  is not whole frames of the format the start named.
- **Start** and **stop** are the desktop's decisions: an application started
  recording, or the last one stopped. The client sends samples between the two
  and nothing outside them. The format is the server's, as a host's is over RDP:
  samples are always signed 16-bit little-endian, and the start names the channel
  count and rate. wlshare names mono at 48 kHz, which is what the zDesk
  gateway's Opus decodes to, so nothing between the browser and the node
  resamples. There is no keyframe: a lost sample is a moment of silence.

`microphone.rs` makes each plugged microphone a PipeWire node,
`wlshare-microphone-<client>`, of class `Audio/Source` and role `Communication`,
described as "wlshare remote microphone". Like the camera it is a thread of its
own running PipeWire's loop, its callbacks on that loop and not on the graph's
real-time thread, and the stream going to *streaming* and back is what the
session sends as start and stop. It is not connected with `AUTOCONNECT`: a
source is linked to by what records from it, and linked on its own it would play
the client's voice into the default sink.

Unlike the camera it is not its own driver. Audio already has a clock, the
graph's, and a source keeping its own would drift against the sinks the
recording application also plays into; so the graph asks for a quantum when it
wants one and the node answers from a jitter buffer between the client's pace
and the graph's. The buffer holds back 60 ms before it plays — after a start, and
again after it runs dry — so a late sample is a gap in the stream rather than a
click in every quantum, answers silence while it has nothing, and keeps at most
200 ms, dropping the oldest, so a client that bursts after a stall is heard live
rather than late. A stop empties it.

Measured 2026-09-14 on a labwc session with PipeWire 1.4.2: a client plugging a
microphone and, on start, sending a 440 Hz tone 20 ms at a time, and `pw-record
--target wlshare-microphone-1` recording mono 48 kHz for three seconds and then
two. Each recording sent start as it linked and stop as it left; past the first
half second both held the tone at 440 Hz with no silent 10 ms block, and the node
was gone from the graph after the unplug.

The announcement is off unless the configuration sets `microphone = true`;
while it is off, a client that lists the pseudo-encoding is told nothing.

## Resize

`SetDesktopSize` sets a custom mode on the shared output, with the same rules.
A request from a client that did not list ExtendedDesktopSize is not answered
at all. One that arrives with `resize = false` is answered *prohibited*; one
naming other than one screen, or no pixels, *invalid layout*. A request for the
current size is answered OK at once; one for an output that cannot be
reconfigured — only a headless output, one named `HEADLESS-*`, ever is, and
only through wlr-output-management — is answered *prohibited*; a size the
compositor rejects is answered *invalid layout*, one it cancels because the
layout changed under it is made once more against the new layout, as a
declaration's is; a request the compositor
accepts is answered when the frame at that size arrives, with an
ExtendedDesktopSize rectangle naming this client as the reason.

## Input and clipboard

Key events carry X11 keysyms. The server compiles the configured XKB keymap,
uploads it to the virtual keyboard, and searches the same keymap for a keycode
producing each keysym, preferring the lowest shift level. Modifier state is
tracked with `xkb_state` and sent after every key. The state is fed a keycode's
transitions only: xkb counts a modifier key's presses and holds the modifier
until as many releases arrive. The compositor is sent the same and no more:
a client repeats a held key as presses — a browser repeats a held modifier
like any key — and none of them is forwarded, since a Wayland client repeats
by itself and Sway takes each press for a key of its own, filling its table of
32 pressed keys until the next key matches no binding and its release never
reaches the application. A character keysym names a
character the client has already cased — zDesk never forwards Caps Lock and
sends `A` or `a` as the browser resolved it — so before each press the server
checks what the keycode would produce under the current modifiers, and presses
Shift or lets a held Shift go around the key when the keycode alone would type
the other case. A Shift pressed for a key goes up with the key unless the client
has since pressed that Shift key itself, which makes it the client's to let go
of. A keysym that names a key rather than a printable character is
exempt and goes out on its keycode under whatever the client holds: Shift+Tab
arrives as Shift then `Tab`, the keycode's shifted level is `ISO_Left_Tab`, and
letting Shift go to make it produce `Tab` would type a plain Tab.
Keys and buttons are let go when the client leaves or is superseded, and a
connection that never finished the handshake releases nothing. Pointer events
arrive in framebuffer pixels and are injected as absolute positions against the
framebuffer's extent, which the virtual pointer maps onto the shared output — a
pointer per connection, each on its own output, moving the seat's one cursor.
That binding is made with the pointer and labwc does not keep it: whenever the
outputs are laid out anew it maps every pointer back to the output its device
names, none for a virtual one, and absolute positions then span the whole
layout — off by the output's place in it once there are two. The wlroots the
APT repository builds carries a patch that gives a virtual pointer its output's
name, which labwc maps it back to; any other wlroots does not, so on every
wlroots each wlr-output-management `done`, which follows every change to the layout, remakes
every desk's pointer against its output, the new one before the old one goes,
letting go of the buttons held and none of the keys.
Wheel "buttons" become discrete axis events, a notch apiece.

A notch is all RFB can say, so a touchpad glide or two fingers on a phone would
scroll in lurches. A client that knows the server is wlshare sends a distance
instead: the private message `0xE5`, six bytes — the type, a byte of padding,
and the horizontal and vertical distance as S16 logical pixels of the output,
positive rightward and downward — after the `PointerEvent` that says where. It
has no pseudo-encoding, the server having no state to hold for it and nothing
to answer. Logical pixels are the units an axis is in, so the distance is
injected as it arrives, as a continuous axis: what a touchpad plugged into the
machine sends, which an application spends as the distance it is.

The clipboard is shared as UTF-8 text through Extended Clipboard, and only
that way: latin-1 cut text is dropped in both directions, and a client that
does not list the extension has no clipboard. Every `SetEncodings` listing it is
answered with the server's caps — text, every action, and no unsolicited text,
as the extension recommends, so a client notifies a change and the server asks
for it. A selection the compositor announces is read off the loop into a pipe
and kept as the shared clipboard, and the client is sent a notify — or, to a
client whose caps take no notify but take text unasked, the text itself, within
the size its caps name; otherwise the text goes when the client requests it, as
the shared clipboard is then. A selection
that is cleared or stops being text is kept as empty text, and notified as
holding nothing. A client's notify is answered with a request, and the text it
provides becomes the shared clipboard and a data source that takes the
selection; the compositor announcing that selection back is ignored while the
source is ours. A clipboard message that cannot be read is dropped, not the
connection.

## Security

RFB 3.8 with the configuration's types on offer: RSA-AES at both widths,
`RA2_256` first, when either `[pam]` or `[password]` is set, or None alone when
neither is. Classic VncAuth is deliberately absent: it proves knowledge of a
machine's secret, names nobody, truncates the password to eight characters, and
protects the login and nothing after it. With no login configured the session is
open and in the clear, so the listen address is a loopback or VPN address by
design.

RSA-AES is RealVNC's type as `rfbproto` documents it and TigerVNC, neatvnc and
the zDesk gateway speak it: the server's RSA key and a fresh client key are
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
client can pin; zDesk logs the fingerprint it saw on every connection.

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

Tight, TightPNG, Hextile, RRE, CopyRect and every lossy encoding but the
private ones, VP9 and the tiles' region streams: the gateway re-encodes every tile anyway, and ZRLE is the standard's best
lossless choice. A VP9 quality above the ceiling the stream was asked for
however much room the link has, the VP9 encoding for any client that does not
list it.
8- and 16-bit pixel formats and colour maps. Moving the client's pointer: the
PointerPos pseudo-encoding would carry a warp the compositor made, and the
cursor session does report positions, but only when the output repaints.
Multiple outputs in one framebuffer — a client picks one of them, or connects
again for another beside it. More than two outputs at once. A
control socket. A microphone format beside the one the server names. A V4L2 camera device for the client's camera: a PipeWire node
needs no kernel module and no privilege, at the cost of applications that open
`/dev/video*` alone not seeing it. Camera formats beside I420, and scaling a
camera picture to a size an application asks for.
