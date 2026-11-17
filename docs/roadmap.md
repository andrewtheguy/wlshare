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

## The encoding agent's pictures through shared memory

The agent was made for a virtual machine and its host's container, and there
every picture still crosses a network: virtio-net, the bridge, a veth
([The encoding agent](architecture.md#the-encoding-agent)). Measured on that
link on 2026-11-17, a whole 1920×1080 picture, 3.11 MB, crosses in 1.2–1.3
ms; the daemon's send costs 0.16–0.22 ms of the system's time since it went
zero-copy, and the agent's read about 0.77 ms. That grows with the picture
and the frame rate: a 3840×2160 picture is four times it.

- **The pictures in a region both machines map.** QEMU's `ivshmem-plain`
  device gives the guest a PCI device whose memory is a file in the host's
  `/dev/shm`; Proxmox makes one from the VM's configuration (`ivshmem:
  size=64,name=wlshare`, `/dev/shm/pve-shm-wlshare`), and the agent's
  container is given the file by a bind mount. Proxmox makes the file anew
  each time the VM starts and removes it when the VM stops, and a bind mount
  holds the file as it was when the container started, so after the VM
  restarts the two map different memory: the container is restarted after
  the VM, by a hookscript on the VM's `post-start`, or the agent opens the
  file through a bind mount of a directory the hookscript links it into,
  and opens it again with each stream. The daemon maps the device's
  memory (BAR2, through sysfs's `resource2_wc`: write-combined, since the
  daemon only writes it; uncached `resource2` is slow). The region holds
  one picture a stream, whole, where it stays from frame to frame, as the
  agent's copy of the picture stays now: the daemon writes what changed
  into it, so what did not change is still there from the frames before,
  and the socket carries the rest, the frame's header and its rectangles,
  and the access unit back. The agent uploads the picture to the VA surface
  from where it lies, so its read is gone. The link is request and reply
  already, so the picture is the daemon's to write only between an access
  unit back and the next header, when the agent is done with it, and a
  store fence (`sfence`) after the writes puts them ahead of the header
  that names them. Further on, the daemon's
  conversion to NV12 could write into the region and leave its send no copy
  at all. Looking Glass's `kvmfr` module binds the same device in a Linux
  guest as a device node with permissions of its own, in place of sysfs.
- **What it costs.** It is a second transport beside TCP, not one in its
  place: an agent across a LAN, or anywhere but the guest's own hypervisor,
  is still TCP. It is new messages on the link, and its `VERSION` with them.
  And the pictures are past the key: whoever can open the file in the
  container, or the PCI resource in the guest, reads the desktop and can
  write into it, so the file's permissions are its access control, and the
  key holds the socket alone. What it would save is estimated, not
  measured.

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

What is left of the lossless picture, planar where the desktop is still and
the median predictor's residuals where it moves
([On a lossless list, every tile exact](architecture.md#the-tiles-encoding)).
They are for a desktop client on a LAN, never a phone, a tablet or a page zDesk
flags as tight on memory, none of which they accommodate, so a client decoder
that takes more memory or more of the processor than RDP's planar codec is no
cost against them. Where the desktop is still they are already about a ninth of the
held picture's bytes or less; where it moves the planar runs it had were 6
to 12 times them, nearly all of their 17 GB over the samples, and the median
predictor's that replaced them two thirds of that
([measured](tiles-performance.md#lossless-tiles), with what the encoder's time
went to, and [the predictor's](tiles-performance.md#the-median-predictor)). An
item is measured as the lossless picture was before it is
planned: `motion-bench --tiles-only --lossless` over the samples at every size
on lab1, against it.

### Under consideration

#### JPEG-LS by CharLS for the moving runs

Measured in the daemon's encoder without being built into it: a scratch copy
of the tree whose moving runs go to CharLS, through `motion-bench` over the
samples at every size on lab1, every frame exact
([Through motion-bench](tiles-performance.md#through-motion-bench), after
[the other codecs](tiles-performance.md#other-coders-for-the-lossless-tiles-moving-runs)
that came no nearer). The page's decoding is not measured.
Its cost is what holds it back: against the planar moving runs the lossless
picture then had, 0.56 of their bytes where the median predictor, its moving
runs since, took 0.66, for six times the predictor's processor on the host, a
video at 1920x1080 87 ms a frame. The predictor is JPEG-LS's alone, its
entropy coding left to zstd with no context. JPEG-LS is a codec built for
lossless pictures, as FLAC is for sound: that predictor, a reversible colour
transform, and Golomb-Rice coding by context, which is the most there is to
take off where the desktop moves. CharLS is its library, in Debian and Ubuntu
as `libcharls-dev`, linked from the system as libx264 and libva are. The page
needs a decoder of its own (CharLS in WebAssembly) in place of the
predictor's, and the protocol a body kind for it, `PROTOCOL_VERSION` with it.
A run is coded on any thread, as a median one is, so an update's wait is its
largest run's. It is planned only once the host's processor at the frame rate
is settled, a faster coder or fewer of the runs; then the page's decoding in
WebAssembly, measured on testdesktop2's Chrome.
