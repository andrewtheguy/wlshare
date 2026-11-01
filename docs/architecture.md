# Architecture

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

- `crates/wlshare-proto` is the bytes of wlshare's private extensions, both
  directions: the video and tiles encodings' framing and what a client lists
  for them, the density, outputs, audio, camera and microphone extensions, the
  scroll message and the beside ClientInit byte, and the protocol version
  that names them all. It is the one crate the zDesk gateway depends on,
  pinned by its `proto-v<N>` tag behind its `wlshare` feature, so the daemon
  and the gateway build and parse every private byte with the same code
  ([The protocol version](#the-protocol-version)). It depends on `thiserror` alone, has no OS
  APIs, and each builder is tested against a decoder written from the
  documented layout and read back by its parser.
- The package at the root, `wlshare`, is the daemon. `rfb/` decides every byte on the wire that
  is not a private extension's: handshake, message parsing and building,
  RSA-AES and its frames, the ZRLE encoder, the video encoder, the tiles
  encoder, the audio extension's encoders, and the
  cursor and clipboard encodings. The audio extension's FLAC encoder is
  `sound-flac`'s own, in plain Rust, and its Opus encoder libopus under
  `sound-opus`, a prebuilt static archive. Its tests decode every encoder's
  output with an independent decoder, read every server message back with a
  client's parser (`rfb/client.rs`), and run the RSA-AES exchange against a
  client written from the specification. `compositor.rs` is the Wayland thread and its
  command handler; `capture.rs`, `cursor.rs`, `outputs.rs`, `input.rs` and
  `clipboard.rs` are the protocols it speaks, and `sway.rs` is sway's IPC, which
  makes and unplugs headless outputs for a client's virtual displays; `audio.rs`, `camera.rs` and
  `microphone.rs` are PipeWire's side, `decode.rs` is the camera's libavcodec decoder
  and `h264/` the H.264 encoder, the host's hardware through libva itself or
  libx264 through its own API, for the video stream and the tiles picture's
  moving regions; `walk.rs` is the walk of the quality and the frame rate a
  link bears, and `wlcap.rs` the capture file of what a session's encoder was
  handed;
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
stream's planes for the video — and encode after
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
ZRLE, the video and tiles — or, for a client that
lists the video encoding, one rectangle of the whole framebuffer
([below](#the-video-encoding)), and for one that lists the tiles encoding
its PNG tiles and region streams ([below](#the-tiles-encoding)).

With Fence negotiated, every pixel update ends with a fence the client echoes,
and the next pixel round waits for the echo. Cursor, audio and geometry
announcements sent with a request precede any pixels for it and have no fences
of their own. One pixel round is in flight at a time, so a slow link is never
flooded and frames coalesce in the framebuffer meanwhile. zDesk negotiates
both ContinuousUpdates and Fence.

What the echo means is the client's to decide, and zDesk holds it while the
picture is the video stream until the browser has taken the frame, rather than
answering the moment it arrives ([below](#the-clients-paint)).

A size change goes out first, as its own update — an ExtendedDesktopSize
rectangle whose reason says who asked (the server, this client, another
client), or a DesktopSize rectangle for a client without the extension — and the
whole framebuffer follows in the next update. A client that negotiated neither
cannot be told and is disconnected at its next update rather than sent pixels at
a size it does not know. The ExtendedDesktopSize announcement that answers the pseudo-encoding is an
update too, and waits for a request like any other.

## The protocol version

The private extensions below have one version between them, and the two ends
speak exactly the same one: none keeps an older layout beside a newer one, so
a gateway and a daemon of different versions would read each other's bytes
wrong rather than less. It is `PROTOCOL_VERSION` in `wlshare-proto`, which is
that crate's major version, the crate being `<N>.0.0`. Any change to the crate
is a new version. The release that brings one tags its commit `proto-v<N>`,
and the zDesk gateway pins that tag, so a release that leaves the crate alone
asks nothing of the gateway; the release workflow refuses a release whose
crate differs from what its version's tag already holds. `wlshare --version`
and the daemon names it when it starts listening.

- **The version**, client → server: the pseudo-encoding `0x574c5600` (`WLV`
  and a zero byte) plus the version, listed in every `SetEncodings` that lists
  anything private — every encoding whose top two bytes are `WL`. A list that
  names any of them without a version, or with two versions, is fatal.
- **The answer**, server → client: the first list that carries a version is
  answered, before anything else the daemon sends and without waiting for a
  request, by an update of one empty rectangle whose encoding names the
  daemon's own version. Where the client's is another, the daemon closes the
  connection after it, and the client, which has the daemon's version from
  the answer, names both. A later list carries the same version and is not
  answered again.

| Offset | Type | Field |
|---|---|---|
| 0 | U8 | `0`, FramebufferUpdate |
| 1 | U8 | padding |
| 2 | U16 | rectangles, `1` |
| 4 | U16 | x, 0 |
| 6 | U16 | y, 0 |
| 8 | U16 | width, 0 |
| 10 | U16 | height, 0 |
| 12 | S32 | encoding, `0x574c5600` + version |

zDesk reads the answer as the first message after its list, and refuses a
`wlshare` target whose server sends anything else first: a server that does
not version the extensions is not one it can speak them to.

## The video encoding

A private encoding, `WLSV` (`0x574c5356`), for the zDesk gateway: the whole
desktop as one H.264 stream, for a client that would rather have a picture
that moves than one that is lossless. While the framebuffer is within its video
ceiling, zDesk lists it for every browser on a target with
`subtype = "wlshare"` whose picture is the video, and passes each frame to the
browser as it came. A client that does not list it is unchanged.

A client that lists it gets it instead of a standard pixel encoding, wherever
in the list it is, and nothing beside it in the list says what the stream is
to be: there is one way to code it, wlshare's, and the gateway has no setting
for it. Each pixel update is then one rectangle covering the whole
framebuffer, whose body is a length word and one access unit:

```text
u32 length   the access unit's bytes
u8[length]   one H.264 access unit, Annex B
```

Successive rectangles are one stream, each frame coded against the ones before it, so a client decodes them
all with one decoder, in order. The coding is the daemon's H.264
(`src/h264/`), the one the tiles picture's regions are coded by
([below](#the-tiles-encoding)): the host's hardware encoder through libva
where the startup finds one, libx264 at its `ultrafast` preset through its
own API otherwise, found once and the same for every session.
`src/rfb/video.rs` is the framebuffer's pixels in and the frame out in the
rectangle's framing, which is `wlshare-proto`'s `video` module — the length
word and its ceiling. What a frame holds is fixed:

- **8-bit 4:2:0, BT.601 at studio swing**, converted from the framebuffer's
  `B, G, R, X` and declared in the stream's VUI, so a decoder converts back
  with the same matrix. The client's pixel format does not apply. The picture
  is the framebuffer's size rounded up to even sides, which libx264 requires
  and 4:2:0 cannot crop below — its last row again below it and black to its
  right — and a decoder draws it cut to the rectangle from its top-left. The
  hardware's at High profile with CABAC, libx264's at Constrained Baseline;
  no B-frames, so each frame in is one out, and no frame is dropped. Every
  IDR has its sequence and picture parameter sets ahead of it, so the gateway
  reads the stream's `avc1` codec string out of the stream itself.
- **Each frame at the quantizer the walk asks.** The 1–100 dial both
  encodings' coders take maps onto H.264's QP 40 down to 16, finest last,
  with no bitrate and no adaptive quantization: every frame is at exactly
  the quality it was asked at. wlshare's dial (`QUALITY` in
  `src/rfb/mod.rs`), 90 or QP 18, is the ceiling, and each frame is coded at
  wlshare's walk's dial below it, the quantizer moving between any two
  frames with no keyframe. libavcodec fixes the quantizer when it opens an
  encoder, its `h264_vaapi` and its libx264 alike, so neither encoder is
  reached through it. The hardware's is the daemon's own encoder over libva:
  it writes the stream's sequence, picture and slice headers (`syntax.rs`)
  and hands the driver them packed, with each picture's parameters, the
  walk's quantizer as the slice's `slice_qp_delta`; libavcodec's
  region-of-interest offset was no substitute, on testdesktop2's shader
  encoder leaving a few macroblocks of every frame at the base and on the
  low-power one ignored beyond about eight either side. libx264's is x264's
  own API, the quantizer forced on each picture by `i_qpplus1` under its
  CRF rate control, whose own choice is never made. What CRF brings is the
  half-size picture x264 estimates its rate on and the motion it searches
  for there, which seeds the full search: `ultrafast`'s own does not follow
  a window dragged far in a frame. What that costs the processor and saves
  in bytes, against a constant quantizer, is in
  [The video's encoder](video-performance.md).
- **A quality and a frame rate that follow the link.** What a link that
  falls behind gives up is quality, and frames once the quality is on its
  floor: the walk of `src/walk.rs`, which walks the tiles' dial too
  (`QualityWalk::new`), from the ceiling to 20 in steps of ten, twenty or
  thirty points the further behind the link is, and back in steps of three
  that double while the link takes them, to 24, short of a quality it
  refused for fifteen seconds. A frame's queueing is its fence's round trip —
  answered once the client has the frame, which for zDesk means once the
  browser has taken it ([below](#the-clients-paint)) — less the shortest of
  the last minute's, so distance does not read as queueing; an IDR counts
  towards that floor but is no verdict. Without Fence it is how long writing
  the frame blocked, 20 ms of it being behind. Two behind frames (60 ms of queueing or more) among the last
  four take a step down, and on the floor double the capture's frame
  interval, at most three doublings: 33 ms to 267 at the default `max_fps`;
  400 ms of queueing halves the frames whatever the quality. Frames come back
  before quality. The interval is read when the next frame is
  due, so a step the last frame's fence brings paces that frame. The cursor,
  the audio announcement and a desktop that changed size never wait for a
  slowed frame's turn. A step is taken at most once a second — half a second
  after one while the lag is 150 ms or more — and while the lag is still
  falling a fifth per second from the step before — the queue it left
  draining — no further step is taken, since the step was enough; after an
  IDR the verdicts wait two seconds, since the frames behind it queue behind
  its crossing. A second of clear frames (30 ms or less), four at the least,
  takes a step back, and after a step back the link took, half a second of
  them is enough while none has more than 15 ms queued; how the walk went
  over the hardware's encoder is in
  [The walk on the hardware](video-performance.md#the-walk-on-the-hardware).
  A slowed session
  paces its own frames; an unslowed one is paced by the capture and its one
  fence in flight.
- **IDRs only when a decoder needs one**: the first frame after the encoding
  is listed, the first at a new size (the encoder is made again for it), and
  the frame that answers a non-incremental request. There is
  no periodic keyframe; nothing is lost on TCP, and a step of the walk is
  the next frame's quantizer, not a new stream.

A video update is sent when anything is damaged; a non-incremental request
can also owe one. The frame is the whole picture, and what did
not change costs it little: the session hands the encoder the damage since the
client's last frame, the encoder converts the rectangles of the damage,
widened to the 2×2 blocks they touch, into the NV12 planes it keeps of the
whole picture, and the rest of the planes is the picture the client already
holds, which the encoder codes as skipped macroblocks. The conversion is
straight out of the framebuffer, under its lock, into the encoder's planes,
and the frame is coded from those once the lock is let go: the pixels are
copied nowhere between, unless the frames are captured. A stream's first frame
and a client the damage log no longer reaches back for are converted whole.
The encode runs on the session's worker, which is told it is blocking, and the
fence keeps one frame in flight as it does a standard pixel update. A
`SetEncodings` that drops the encoding is answered with the whole framebuffer
in the standard encoding it selected — ZRLE when listed, Raw otherwise —
since the client is holding a lossy picture.

The stream's tests read every frame back with libavcodec's software H.264
decoder (`src/decode.rs`), which shares nothing with either encoder, through
the rectangle's framing. A coder that cannot be opened, or fails a frame,
ends the session, as it does a tiles picture's; one that makes no frame sends
nothing: the damage stays unseen and the IDR owed, for the frame the next
change brings.

### Capturing what the encoder is handed

A daemon started with `--capture-frames DIR` writes, for every session, what
its video encoder is handed, losslessly: each frame's time, its size, the dial,
whether a keyframe was asked, the damage rectangles or that there were none,
and the rows the rectangles span as the framebuffer holds them,
`B, G, R, X`. The file is the capture file of `src/wlcap.rs`, whose layout is
written out there: this repository's own, read and written here, and owing
nothing to the codec, since a capture is the pixels in front of an encoder
whichever encoder that is. There is one `<unix millis>-client<id>.wlcap` per
session, written at the one call that hands the encoder its pixels and so
spanning every encoder the session makes across resizes. Nothing on the way
is lossy, which is the point: an encoder is run again on the pictures a
desktop is actually shown as and the rectangles they came with, not on a
decode of a stream. The files are uncompressed and large — a 4K whole frame
is 33 MB — and are `zstd` compressed afterwards. `examples/wlcap.rs` says
what a capture holds, lists its frames' times, plays a run of a size out as
raw pictures and cuts part of it into a capture of its own.

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
authentication on loopback, and `video-sink` (`examples/`),
the one client the container has, which lists the video, coded at the
daemon's dial, with Fence, ContinuousUpdates, ExtendedDesktopSize and Cursor, sets the desktop's
size, echoes the fences, throws the frames away, on cue reads late for a
while, and types what the scenarios have it type, so the keys reach the
desktop through the daemon as a person's do. The
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
so the fences come back late and the frames slow, then come back, then the
video paused where it stands — each scenario its own session and so its own
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
this: its Mac in Standard mode and its Windows over RDP hand it the pixels
of the same kind of desktop to code, so a wlshare capture at their sizes stands for
them.

To watch one, `scripts/capture-mp4.sh` makes an MP4 of it: `wlcap frames`
writes the capture's pictures at a constant rate, its own timing kept, into
ffmpeg's libx264, so nothing passes through the daemon's encoder.

The captures are the source an encoder is measured on, and for a quick run
parts of them: `scripts/capture-samples.sh` cuts each capture to 120 frames
under its own name with `wlcap trim --frames`, a quiet scenario's first and
a busy one's busiest, the cut's first frame the picture as it stood, whole,
with a keyframe asked; `walk` it copies whole, since its frames slow and
come back over all of it, and `mixed`, minutes at the desk of which a cut
would keep only the busiest, the video full screen.

The walk is measured on the captures the same way, without a link being
shaped: `walk-sim` (`examples/`) plays one or several of them through the
video encoder and `QualityWalk` as a session does, one frame in flight, to a
model of zDesk passing the stream to a browser (`examples/sim`) — the frame
handed to the browser's link as it arrives, its fence echoed at once while
nothing handed before it is unpainted and otherwise once the oldest such has
been owed past the link's ping round trip, 500 ms at most — over a link of a
given rate, changing at given times, and round trip. It says how long each
change of the desktop took to be painted, the frames a second, the link it
used, the quality the motion was coded at and the encoder's processor
time, and with `--timeline` every frame,
as JSON lines, a JSON object a line. The time is the model's, so a run takes
seconds; `--encode` fixes what an encode takes, so that two walks are
compared on the same frames. Its stream is the hardware's where the machine
has a VA-API encoder and libx264's otherwise, or with `--libx264`.

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
the zDesk gateway on a link with room to spare: the desktop as lossless PNG
tiles where it is still, and a video stream of its own for each region that
moves, H.264 — the host's hardware, or libx264. It is a choice the gateway makes per target, not a replacement:
`WLSV`, one stream over the whole desktop walked on the link, stays the
picture for a slow link, where a lossy stream in fewer bytes is what the link
bears. This one spends bytes the link has, for less of the encoder's time and
a picture that ends lossless, on a list that holds its dial; and on a list
that lets its dial walk, it follows the link as `WLSV` does, every tile a
RemoteFX Progressive tile, still or moving, with no stream and nothing owed a
lossless picture, so a client that wants the walk more than a picture that
ends lossless gets that too; and on a list that asks for it lossless, every
tile that changed, still or moving, is RDP's planar codec of its pixels, with
no H.264 stream, so a client on a LAN that wants every picture it is shown exact,
as a Windows host draws its pipeline under the Lossless policy, gets that
(experimental). It is zDesk's Motion, which that gateway dropped
because it sat between a native protocol and the page — on RDP it decoded what
the host had encoded only to encode it again, on a Mac it was layered over
ZRLE — and which belongs here, where the compositor's pixels and damage are
coded once. `src/rfb/tiles.rs` is the encoder, and
`wlshare-proto`'s `tiles` module its framing.

A client that lists it, with `WLSD` (`0x574c5344`) beside it where it wants
the dial held rather than walked, or `WLSL` (`0x574c534c`) where it wants
every tile lossless, gets it instead of a standard pixel
encoding, wherever in the list it is; a walked list, which names neither,
gets every tile progressive, and a lossless one every tile planar, below.
The dial is wlshare's, as the video's is
([above](#the-video-encoding)): nothing in the list names one. A list that
names both it and `WLSV` is fatal: they are two pictures, of which a client
wants one; and so is one that names both `WLSD` and `WLSL`. A
pixel update is then any number of rectangles of the encoding, drawn in
order, each a rectangle of the desktop whose body begins with a kind:

```text
u8 kind
0 PNG     u32 length | u8[length]                          a PNG of exactly the rectangle's pixels
1 START   u8 stream | u32 length | u8[length]              a region stream's first frame
2 FRAME   u8 stream | u32 length | u8[length]              the stream's next frame
3 END     u8 stream                                        the stream is over
4 COPY    u16 x | u16 y                                    what the client holds at (x, y), drawn here
5 CACHED  u16 slot                                         the tile kept in slot N, drawn here
6 STORE   u16 slot                                         what the client holds here, kept in slot N
7 PROGRESSIVE u32 length | u8[length]                      a RemoteFX Progressive tile block of the rectangle's tile
8 PLANAR  u32 length | u8[length]                          a zstd frame of a planar bitmap of exactly the rectangle's pixels
9 STILL   u32 length | u8[length]                          the still stream's next planar bitmap, of exactly the rectangle's pixels
```

A PNG is the desktop's pixels there, lossless, 8-bit RGB or a palette of it,
depending on nothing before it; it is every still of a list that holds its
dial, and sent on no other. A START begins a stream on its id over its
rectangle, an H.264 keyframe of that size rounded up to even sides, a picture
a client cuts to the rectangle from its top-left; each FRAME on the id is the
next frame of that stream over the same rectangle; an END says it is over, its last picture
staying until something draws over it. A COPY draws at its rectangle what
the client holds at `(x, y)`, the same size, as the rectangles before it left
it: a scroll, or a window moved, as RFB's CopyRect draws one, the two
overlapping as a picture drawn onto itself does, its source read whole first.
A STORE keeps what the client holds at its rectangle, one 64×64 tile on the
grid from (0,0), in slot `N` of the client's cache of 4096, over whatever the
slot held; a CACHED draws the tile slot `N` keeps at its rectangle, the same
size, and drawing from a slot nothing was stored in is fatal. The cache
outlives a picture that starts over — a new size, a non-incremental request,
a list naming another walk — for as long as the list names the
encoding. The encoding's one message, `0xE6`, a byte alone (`CacheEmpty`), is
the client's word that its cache holds nothing the server can count on, a
page that attached anew or missed a rectangle: the server draws from no slot
until it stores it again, and its next update is the picture whole, so none
of damage goes between it and the non-incremental request the client sends
after, in whatever read that comes.
A PROGRESSIVE is the rectangle's tile, one 64×64 on the grid cut by the
desktop's edge, as one tile block of RemoteFX Progressive ([MS-RDPEGFX]
2.2.4.2.1.5): a first pass at a stage, in place of what the client held of
the tile or as the difference from it, or an upgrade, the bits the stage
before held back. The tables it is coded with are the protocol's and never
sent — `wlshare-proto`'s `progressive` module holds them, and the stream a
standard decoder reads a block from — and the client decodes every block in
order, keeping each tile's coefficients from one to the next. It is every
rectangle of a walked list but a COPY, a CACHED and a STORE.
A PLANAR is the desktop's pixels there, lossless, as RDP 6.0 bitmap
compression codes them ([MS-RDPEGDI] 2.2.2.5.1), the graphics pipeline's
planar codec: a format header of no alpha plane and no colour loss or
subsampling, then the red, green and blue planes, top row first, each
run-length coded over its rows' differences, or plain with the padding byte
that form ends with; the body is that bitmap as one zstd frame, and nothing
after it. It depends on nothing before it, and is every rectangle of a
lossless list where the desktop moves but a COPY, a CACHED and a STORE. A
STILL is the same bitmap in the still stream, one zstd stream across every
STILL of the session, its body the stream's bytes through a flush after the
bitmap, which read after the bodies before it is that bitmap exactly; a body
that begins a zstd frame starts the stream over, and its window is at most
8 MiB. It is every rectangle of a lossless list where the desktop is still
but a COPY, a CACHED and a STORE, and the stream starts over where the
picture does and after a client's `CacheEmpty`.
A PNG over a stream's rectangle only
ever follows its END, so drawing every rectangle where it says, in order, is
the picture.

- **Still is a still.** The desktop is in 64×64 tiles from (0,0). The damage
  since the client's last update is copied out of the framebuffer under the
  lock into a picture the encoder keeps, and compared there tile by tile with
  what the client holds, so a compositor redrawing what it shows sends
  nothing. A tile that changed and that no stream carries goes as a still —
  on a held list a PNG, on a lossless one a planar bitmap, on a walked one
  each tile progressive, below — a row's
  run of them, cut where the tile rows meet into bands of at most 64 rows,
  one image, so that a tile the damage covers is in one still whole. A picture that
  starts over — the encoding newly listed, a new size, a non-incremental
  request — goes the same way at once, whole, where Motion streamed it first
  and sent every tile as a still after it anyway: on a link with room that stream
  is work for nothing. The PNG is the gateway's tile coder: a palette
  unfiltered where the pixels have at most 256 colours, RGB with each row's
  filter chosen otherwise, at `png`'s fast compression.
- **What moved is a copy.** Where a read changed enough tiles that no
  stream carries and that have not been changing in place — 4, none of them
  changed in 2 of the last 8 tenths, unless a copy was found within those
  8 tenths: then what churns may be a scroll the search missed, and left out,
  two misses ended the copies for the rest of the scroll — runs of 16 pixels
  of the new picture, up to 64 of them, none of one colour, are looked for in
  the old one, along every fourth of its rows within the damage and 512
  pixels of the tiles, by a rolling hash, and along the rest of them where
  nothing that found grew into a copy. A run found at more than 32 places, in a texture or a flat
  fill with an edge, says nothing and is dropped, where one in a line of text,
  found on many of a page's lines, still counts. Each offset a run was found
  at is checked against every run, its 16 pixels against the old picture's
  there: a run on a row the scan stepped over is found at no offset, and on a
  fast scroll, whose new rows hold most of the rest, the scan alone left
  fewer than two to agree. The offsets the last read's copies were at are
  checked the same way whatever the scan found, every run found there
  counting, those at many places too: a window dragged on moves as it did,
  and a terminal's text, found at too many places to say anything, left the
  scan too few runs to agree, so a drag across a large desktop went as
  stills ([measured](tiles-performance.md#adapting-to-the-link)). An offset
  two runs agree on
  is grown from each into the largest rectangle whose every row is the old
  picture's at that offset, and sent as a COPY ahead of the update's stills,
  at most 8 a read, where it draws some changed tile whole: one that draws
  only parts of tiles saves nothing, as each is a still anyway. Its tiles are no still and, with those it reads
  from, no churn, so a scroll or a window dragged stays copies and the strip
  it uncovers stills, and never becomes a region's stream. A copy reads only
  what the client holds by then — no tile a stream carries, none owed a still
  not yet sent, nothing another copy of the read draws — and draws on no
  tile a stream carries; every tile it draws on is owed what its source is
  owed, so a copy of a stream's picture settles as the stream does. On a
  walked list a copy from a settled source is settled too, as a cached tile
  stored at the full stage is: what it draws is the full stage's pixels,
  moved, though the client holds no progressive coefficients of them where
  they now are, so the next change to one of its tiles is a first pass that
  is no difference. A page scrolled and left goes again only where the
  scroll uncovered it, not whole at the full stage.
  It is looked for on a held list and a walked one alike: a scroll sends a
  few bytes and the strip it uncovered, a page scrolled a third of the bytes
  it did held ([measured](tiles-performance.md#copies)).
- **What it was shown is a cached tile.** The client keeps a cache of 4096
  tiles, 64 MiB of pixels, and wlshare the record of it: after every still
  goes out, each whole 64×64 tile of it that is not one colour is stored in a
  slot (`STORE`), the least recently used once all are full, and the record
  keeps the hash of its pixels. A changed tile whose whole pixels the record
  finds in a slot goes as a draw of that slot (`CACHED`), 15 bytes, and the
  still's run is coded around it: a window switched back to, a menu opened
  again, a page scrolled back a whole tile, text moved between windows. What
  it draws is what the slot was stored as, so it is owed what that still was
  owed: a tile stored lossless, or at the full stage, is settled, and a
  progressive tile stored below the full stage is owed it, as a copy of one
  is; once its upgrades settle it, it is stored again in its slot, settled.
  A flat tile is never stored:
  its PNG is a few bytes and a slot is worth more. The client keeps only
  what it is told to and draws only what it is told, so the record decides
  everything and the two agree by construction. A picture that starts over
  keeps the record, as the client keeps its slots, so the whole picture is a
  draw of a slot for every tile of it a slot still keeps: an output switched
  back to, a new size — the record handed to the encoder made for it — or
  another dial. A tile a walked list stored is the full stage's at best,
  short of the lossless pixels a held or a lossless list is owed, so neither
  draws any of them until each is stored again, lossless. The record is forgotten only where the client says its cache is gone
  (`CacheEmpty`), as the gateway does at every repaint it asks, a page
  attached anew holding nothing, or where the list stops naming the encoding.
  Over a session at the desk it serves about a seventh of the stills
  ([measured](tiles-performance.md#the-cache)).
- **On a held list, what moves is video.** Motion's rules: a tile is moving when it changed in
  4 of the last 8 tenths of a second; moving tiles join into 4-connected
  components, one under 5 tiles is left to PNG, boxes merge unless the merged
  box would be more than twice the moving tiles, and at most 4 regions stream
  at once. The regions are laid out again at most every 500 ms: one that
  shrinks keeps its stream, one that grows is started again at a keyframe, one
  with no motion for a second ends. What changed under a stream that ends
  that no frame of it carried goes as stills after its END in the same update,
  or the client would hold it as the stream's last frame left it, older than
  the picture. A region is at most the desktop, which
  has the ceiling `WLSV`'s whole-desktop stream has: the gateway lists this
  encoding only for a desktop its browser's decoder takes, as it does `WLSV`,
  so wlshare holds a region to no ceiling of its own. Each region
  is an encoder over its rectangle alone, told the damage inside it as
  `WLSV`'s is, so the encoder's time follows the area that moves rather than
  the desktop. It is H.264 at 4:2:0, below, which every browser decodes, its
  hardware decoders included; what it loses lasts only while the region
  moves, since the settle sends it again as a still. Its dial is wlshare's,
  held: a walked list streams nothing.
- **The dial walks, unless the list holds it.** wlshare's dial is a
  ceiling, and the session's walk of the link — the same one, with the same
  thresholds and steps, that walks `WLSV`'s quality and frames
  ([above](#the-video-encoding)), the quality down before the frames — moves
  this picture's below it
  on each update's fence, and on how long
  its write blocked where there is no fence. An update that carries the
  picture whole crosses the link as a keyframe does and is no verdict, and
  nor is one of the tick's alone, what tiles already sent are owed. Once the
  desktop has been still for half a second that is what its stopping owes:
  the walk keeps its place through the quiet, its verdicts starting over at
  the first. One between a typist's keys leaves the walk where it was going,
  since a tick that paid the last key's upgrades started it over before it
  had the second of clear updates a step up needs, and kept it on the floor
  while it typed. Any other is a verdict, a START
  among it: a region's IDR is the load a desktop that moves keeps sending,
  which the walk is there to judge, and were its verdicts held two seconds
  after each, as a keyframe holds them, the walk would hear almost nothing
  of a video in a window
  ([measured](tiles-performance.md#the-walks-verdicts)). A slowed walk paces
  the updates as it paces `WLSV`'s frames, and the dial is what a
  progressive tile's first pass follows, below. Nothing is owed a lossless
  picture: a client that lists the walk wants it more than a picture that
  ends lossless.
  A first pass below the full stage is owed its upgrades, and the debts
  wait while the walk is slowed: a link that is behind is not handed finer
  pictures of what it already has. They wait until the desktop has been
  still for two seconds, and no longer: a still desktop sends no update
  whose verdict could lift the walk. With `WLSD` beside the list the dial
  is held, every still is lossless, the streams stay at the dial, and a walk moved by
  blocked writes only paces the updates: for a link with room, where a walk
  would only ever stand at the ceiling, and a picture that ends lossless. The
  walk is `WLSV`'s rather than one of its own because the verdict is the same
  — an update's queueing on the link — and a second set of thresholds would
  be a second thing to measure. With `WLSL` beside the list there is no dial
  to walk: the walk's ceiling is its floor, so the lag its fences show slows
  the updates and nothing else, as the frames of a walk on its floor are
  slowed.

- **On a walked list, every tile progressive (alpha).** A list whose dial walks
  gets no PNG or stream: every tile that changed, still or moving, is a
  PROGRESSIVE, as a Windows host draws its desktop where it has no H.264,
  and copies and the cache as above. `src/rfb/progressive.rs`
  is the encoder: a tile to YCbCr in 11.5 fixed point, three levels of the
  reduce-extrapolate 5/3 wavelet, quantized by the protocol's table, and sent
  in passes. A first pass holds back the low bits of every coefficient at a
  stage — 75 while the walk is at the ceiling and not slowed, 50 while its
  dial is at 70 or above, and below that a stage of the protocol's own that
  holds back a bit more in every band, a third fewer bytes than 50, since the
  host's first pass stays at 50 however slow the link and a walk that lowers
  its dial would shrink nothing — or is the full stage at once where that
  costs at most a quarter more, or 64 bytes, as a flat tile's does. The full
  stage is coded to compare only where it could win: where the bits the
  stage holds back of the coefficients, in bytes, and the coefficients it
  holds back whole come to at most twice that margin. A changed tile the
  client holds goes as the difference of its coefficients from what the
  client holds where that has more zeros, which is how motion is carried
  without a stream. The encoder keeps what the client holds of every tile,
  so each pass is coded against it; a tile a copy or a cache slot draws over
  is forgotten, and goes next as a first pass that is no difference. An
  update's first passes are coded ahead on every core, four tiles a task,
  and sent in order; an update of fewer than four codes them as it sends
  them.
  What is owed is the stages after: the settle sends a tile's upgrade, one
  stage at a time, the bits the stage before held back, until it stands at
  full, and nothing is owed after. Its tests read every pass back with a
  decoder of their own, and `examples/tiles-walk-sim.rs` walks it
  ([measured](tiles-performance.md#progressive-tiles)).

- **On a lossless list, every tile planar (experimental).** A list with
  `WLSL` gets no PNG, progressive tile or H.264 stream: every tile that
  changed, still or moving, goes as a still, a row's run of them one planar
  bitmap, a STILL where its tiles are still and a PLANAR where they move,
  by the churn a held list lays its regions out by, and copies and the
  cache as above, as a Windows host whose policy asks for a
  lossless picture draws its whole pipeline with the planar codec, H.264 and
  RemoteFX Progressive refused. What the client is shown is exact from the
  first update, so nothing is owed and nothing settles: a desktop that moves
  is sent as often as the link takes it, each update the pixels, and a link
  short of the bytes gets fewer updates, never coarser ones.
  `src/rfb/planar.rs` is the coder: a rectangle's pixels cut into red, green
  and blue planes, each row below the first as its difference from the row
  above, folded so a small step either way is a small byte, and each row of
  those as segments of bytes as they are and a run of the last of them; a
  flat 64×64 tile is a few hundred bytes, and a rectangle whose segments would
  come to more than its planes as they are, noise or a photograph's grain,
  goes plain. A row the same colours as the row above is one coding of a
  row of zeroes, and the planes are cut from the pixels with AVX2 where the
  processor has it. Each bitmap is then zstd, as a Windows host's bulk
  compression wraps its pipeline — level 1's fast strategy with a small
  table and long matches, a moving picture's frame being its literals'
  entropy coding. Where the desktop moves, it is a frame of its own, so
  every moving run of an update is packed at once on every core and read
  alone. Where it is still, it is the still stream's, a 1 MiB window across
  the session flushed at each bitmap, coded in order after the bitmaps are
  cut on every core: a page, a terminal or a menu shown again is matched
  against what the stream carried, which takes a third to three fifths off
  what a desktop of pages comes to in frames of their own, and a moving picture,
  which it would take nothing off, stays framed and on every core. The
  stream is state both ends keep, so it starts over with the picture, and
  with a client's `CacheEmpty`, which a page that cannot read a STILL sends
  as one that missed a rectangle does. A still or a scroll is then a quarter
  of a PNG's bytes or less, and a desktop that moves several times the held
  tiles' bytes and a fifth more of their processor, for a LAN
  ([measured](tiles-performance.md#lossless-tiles)). Its tests read every
  body back with a reader of their own, written from the specification,
  the zstd by ruzstd, a decoder that shares nothing with libzstd, from
  andrewtheguy's fork, which adds `FrameDecoder::peek` for the still
  stream's newest bytes, held back while its frame goes on; the page
  decodes it with the same and the planar codec of the compositor that
  draws an RDP host's pipeline.

- **H.264, which wlshare chooses: the host's hardware, or libx264.** Every
  region of a held list is H.264, by an encoder found once at startup; the client has no
  say, and a START names no codec. On a host whose GPU has an H.264 encoder a VA-API
  driver reaches — looked for with `hardware_h264 = true` in the
  configuration, the default — it is that one, which is what this picture is
  for: the processor pays for the conversion to NV12 and nothing of the
  search. On a host with none, or with the key off, it is libx264 at its
  `ultrafast` preset, which cost the processor about a third of VP9's time a
  frame for about twice its bytes, on a link that has them ([measured](tiles-performance.md#the-regions-encoder)).
  `src/h264/` is that encoder, the GPU's through libva itself or libx264
  through its own API ([above](#the-video-encoding)), linked from the
  system's libva and libx264, and the `H264Coder` of the daemon's tiles
  encoder, which opens a stream of it for each region, as the video encoder
  opens one over the whole desktop. The coding
  is the video stream's: each frame at the quantizer it is handed, the dial mapped onto QP 40 down
  to 16, no bitrate, nothing dropped, no B-frames, one frame in and one out,
  BT.601 at studio swing in the stream's VUI; the picture the region's size
  rounded up to even sides, which libx264 requires, so a region of an odd
  side, as one at the edge of an odd desktop is, is coded a pixel larger, its
  last row again below it and black to its right, which the gateway clips; each frame one Annex B access
  unit, the parameter sets ahead of every IDR, and an IDR at the stream's
  START; a region's dial is wlshare's, held. The
  hardware's is High profile with CABAC and the 8×8 transform, one slice, the
  frame before its one reference, no reordering, which its VUI says so a
  decoder holds no picture back; libx264's is Constrained Baseline,
  which `ultrafast` comes to, tuned `zerolatency` — no lookahead, the
  picture sliced among the machine's cores less two, at most eight — with no scene
  cut, and with its version SEI ahead of a stream's first IDR, which a
  decoder passes over. The gateway announces the stream from that parameter
  set, `avc1.` and its profile and level, and the browser's decoder takes it
  in hardware where it has one. At startup the daemon tries each render node,
  the low-power encoder first — Intel's fixed-function block, which leaves
  the execution units to the desktop and codes CQP without the HuC firmware —
  and the shader one after it, then libx264, each on two frames of a small
  picture at two quantizers, decoded and held to them, since a driver lists
  more than it codes; a VA-API encoder must also take the daemon's sequence
  parameter set packed, whose colour description VA's parameters cannot
  carry. A frame the driver flags as overflowed or a bad bitstream fails its
  stream, and is neither sent nor made the reference. The log says
  what it found, and with nothing found the daemon does not start: neither a
  region nor the video stream has another codec. A coder that cannot be opened for a session's tiles
  picture, refuses a stream or fails one midway ends that session, as any
  encoder's failure does. Each session opens the device itself, so a VA
  display is one session's.
  `examples/tiles-sink.rs` in the daemon's crate is a client that keeps
  each stream, for FFmpeg's own decoders to read back whole. The daemon's unit
  tests code frames on libx264, on every machine, and on the hardware, and
  decode them with libavcodec's software decoder, which also reads back each
  macroblock's quantizer over a stream walked across the dial: libx264's
  every one at the QP asked, the hardware's at its median and within three,
  since a driver's macroblocks are its own; the hardware's passes with a
  word on a machine with no encoder. ffmpeg's `trace_headers` reads the
  daemon's headers back field by field.
- **It settles.** Every tile a stream carried is owed a PNG, and every
  progressive tile below the full stage its next stage's upgrade, once it
  has been still for half a second. What
  is owed is looked for every quarter of a
  second, each look an update that waits for a request and the fence as any
  other: the 320 tiles owed longest at most, as runs along a row, the
  coarsest stage first, then the oldest, until the look's bytes have gone —
  an upgrade is no change, so a tile's next stage is ready at once. While
  the walk is at the ceiling those are 256 KiB. Below it, where the link is
  short of bytes, they are 8 KiB while the desktop moves; once it has been
  still for half a second, what the link was found to take: twice the last
  look's after an update that carried one and queued under 30 ms, its
  fence's round trip less the link's own as the walk keeps it, half after
  one that queued 60 ms or more, from 8 to 256 KiB, and 8 again whenever
  the walk steps down, which says the link is short of what it was. On a
  still desktop
  a look its bytes cut short is owed again at once, after its update's
  fence, so the settle goes as fast as the link carries it rather than a
  look a quarter of a second, 40 tiles of a 4K desktop at a time: one that
  stopped on a 10 Mbit/s link took 21 s where it now takes about one
  ([measured](tiles-performance.md#adapting-to-the-link)). None is sent
  while the walk is slowed until the desktop has been still for two
  seconds. So a desktop that stops is lossless on the page on a held list and
  at the full stage on a walked one, at 4K within 4.3 s held where it took
  13 ([measured](tiles-performance.md#the-settle)), and once it is
  nothing more is sent.
- **Rounds as any other.** Each update ends with its fence, and one is in
  flight at a time. A new size ends every stream with no END — the size goes
  out first, and the picture whole after it — and a non-incremental request
  ends them with theirs. A list that drops the encoding is answered with the
  whole framebuffer in the standard encoding it selected.

`examples/motion-bench.rs` in the daemon's crate plays the kept captures
through this encoder and through `WLSV`, both decoded as a page would, the
tiles' updates through the client's parser, a copy from the page's own
picture, and `WLSV` and each region by libavcodec's software H.264 decoder,
the dial held and every still lossless, or with `--lossless` every tile
planar. It writes JSON lines as `walk-sim` does,
and `scripts/motion-tables.py` makes Markdown tables of either's logs. Its
regions and `WLSV` are the daemon's own H.264 — `h264/`, found as the
daemon finds it, or libx264 with `--libx264` — so it runs where the daemon
builds. `WLSV` is held at wlshare's dial, or at `--quality Q`, a dial the walk
can stand at, for the encoder's cost there.
`--tiles-only` codes no `WLSV`, so a change to the tiles is measured
against the `WLSV` of a run before. Its `check` holds the encoder to sending nothing once settled,
through 5 s of quiet and 10 redraws of the pixels the page already has, and
to ending lossless; the `still` captures are for it and no measurement. What
it measured, and what the rules beyond Motion's were chosen by, is in
[Tiles performance](tiles-performance.md).

`examples/tiles-walk-sim.rs`, beside it, plays the captures through this
encoder and the walk over `walk-sim`'s model of the gateway and the
browser's link (`examples/sim`), one update in flight,
as a session does on a walked list, every tile progressive, one that
holds its dial, or a lossless one, and says of a run what `walk-sim` says; what it found is in [Tiles
performance](tiles-performance.md#the-walks-verdicts).

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
  what differs, under the same rule as a resize — a headless output — and
  **answers every declaration** with an OutputScale: after
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
encoding — the video paced to its own link, where it lists it — and its own
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
  message, as the tiles picture's choices are, so it rides the list that asks for
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
The daemon's tests read every frame back with symphonia's decoder, which
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
The daemon's tests wrap the packets in an Ogg stream behind the header a
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

Every client that lists the pseudo-encoding is answered with the announcement:
no configuration turns the sound off, and the package depends on the PipeWire,
WirePlumber and pipewire-pulse it plays through.

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
so a running one must be restarted after a backend is installed. The package
recommends `xdg-desktop-portal` and `xdg-desktop-portal-gtk` for this, rather
than depending on them: the daemon uses neither, and an application that opens
PipeWire itself needs no portal. Chrome also
reaches cameras through PipeWire only with
`chrome://flags/#enable-webrtc-pipewire-camera` enabled; without the flag, or
without the portal's Camera interface, it looks at `/dev/video*` alone and lists
no camera. Checked 2026-09-13 with Chrome 153 on a labwc session with
xdg-desktop-portal 1.20.3: with only `xdg-desktop-portal-wlr` installed, the
portal exported no `org.freedesktop.portal.Camera`, and with
`xdg-desktop-portal-gtk` added and the portal restarted, it did.

Every client that lists the pseudo-encoding is answered with the announcement:
no configuration turns the camera off.

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

Every client that lists the pseudo-encoding is answered with the announcement:
no configuration turns the microphone off.

## Resize

`SetDesktopSize` sets a custom mode on the shared output, with the same rules.
A request from a client that did not list ExtendedDesktopSize is not answered
at all. One naming other than one screen, or no pixels, is answered *invalid
layout*. A request for the
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
outputs are laid out anew, and whenever it reloads its configuration, it maps
every pointer back to the output its device names, none for a virtual one, and
absolute positions then span the whole layout — stretched across it, on every
output, once there are two. On every wlroots each wlr-output-management `done`,
which follows every change to the layout, remakes every desk's pointer against
its output, the new one before the old one goes, letting go of the buttons held
and none of the keys. A reload changes no output and is followed by no `done`:
the wlroots the APT repository builds carries a patch that gives a virtual
pointer its output's name, which labwc maps it back to; on any other wlroots a
labwc reload leaves the pointers stretched until the layout next changes.
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
the username empty. `src/rfb/rsa_aes.rs` has the exchange byte by
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
private ones, the video and the tiles' region streams: the gateway re-encodes every tile anyway, and ZRLE is the standard's best
lossless choice. A video quality above the daemon's dial, however much
room the link has, and the video encoding for any client
that does not list it.
8- and 16-bit pixel formats and colour maps. Moving the client's pointer: the
PointerPos pseudo-encoding would carry a warp the compositor made, and the
cursor session does report positions, but only when the output repaints.
Multiple outputs in one framebuffer — a client picks one of them, or connects
again for another beside it. More than two outputs at once. A
control socket. A microphone format beside the one the server names. A V4L2 camera device for the client's camera: a PipeWire node
needs no kernel module and no privilege, at the cost of applications that open
`/dev/video*` alone not seeing it. Camera formats beside I420, and scaling a
camera picture to a size an application asks for.
