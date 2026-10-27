# VP9 performance

What the `WLSV` stream's encoder costs, where the time goes, what a session
holds, and what was tried. Measured on 2026-10-25 and 2026-10-26 on an
eight-core AMD EPYC (Rome) VM, idle, with the encoder on six threads (the
machine's cores less two), 4:4:4, screen tuning and quality 90, over the kept
captures (`frame-samples/`, 120 frames of each scenario; see [Capturing what
the encoder is handed](architecture.md#capturing-what-the-encoder-is-handed)).
Flood is in the figures for reference and is not a target.

## Where the time goes

The cost is libvpx's, not wlshare's. `vpxenc`, from Debian's libvpx 1.15, coding
the same 120 frames of a 4K video at the same settings took 23.3 s of the
processor against `wlshare-vp9`'s 22.7 s, at the same bytes (134 and 135 kB a
frame). The conversion in front of the encoder is 1–6% of a frame; the profile
shows libvpx's AVX2 and SSE kernels, so the archive's runtime CPU detection
works; and a running encoder takes next to no page faults. Within libvpx the
time is spread: the mode decision (`vp9_pick_inter_mode`) about 41%, the
transform and quantizer about 20%, the bitstream about 7%.

A 4K frame at speed 7 costs about 185 ms of the processor whatever the thread
count: 174 ms a frame on one thread, 98 on two, 55 on four, 42 on six and 37 on
eight, about 16 ms of each frame not parallel. Milliseconds a frame, wall
clock, at speed 7:

| Scenario | 1080p | 4K |
|---|---:|---:|
| terminal | 2.3 | 9.5 |
| drag | 7.9 | 18.7 |
| uncover | 7.9 | 22.5 |
| browser scroll | 16.0 | 42.8 |
| video | 16.2 | 42.2 |
| flood (reference) | 116 | 268 |

A frame that changes almost nothing still costs libvpx a pass over the whole
picture: a 4K terminal frame, a keystroke of 1.5 kB, takes 17–19 ms of the
processor and 6–8 of the clock, and at 1080p 5 and 2. About a quarter of it is
copying every block the frame skips from the frame before into the new one,
about a seventh the sum of differences of each 64×64 block of the source
against the last, and about a seventh copying the source into libvpx's own
buffer, which libvpx 1.16's VP9 does whole on every frame whatever the active
map says. Drag and uncover pay it on top of what they change: 56–63 ms of the
processor a 4K drag frame and 78–86 an uncover one.

What multiplies it, each against the setting the stream has, at 4K:

- **4:4:4** against 4:2:0: a third more time on video (42 ms against 32), a
  tenth more on a scroll.
- **Screen tuning** against libvpx's default: 40% more time on video (43 ms
  against 31), and none on a scroll, which without it sends 4.5 times the
  bytes (354 kB a frame against 79).
- **Speed 7** against 9: 9 is 32% faster on video and 40% on a scroll, for 11%
  more bytes on video, 44% on a drag and 64% on an uncover.
- **Quality 90** against 60: 60 is 15% faster on video, and no faster on a
  scroll.

libx264 at `ultrafast` on the same 4K frames took about 30 ms of the processor
a frame against VP9's 185, at 1.5 times VP9's bytes on video and 5.6 times on
a scroll.

## What a session holds

A stream's memory is libvpx's, held from the encoder's making to its drop,
and measured as the daemon allocates it, through jemalloc. After 40 frames of
video, on six threads:

| Picture | 4:4:4 | 4:2:0 |
|---|---:|---:|
| 1080p | 272 MB | 210 MB |
| 4K | 825 MB | 672 MB |

The thread count moves it little: a 4K 4:4:4 stream held 784 MB on one thread
and 839 on eight. Dropped, all of it goes back to the system at once but the
72 MB jemalloc keeps for the process, so a session that ends or a picture
that is resized leaves nothing behind.

Of a 4K 4:4:4 stream's, a heap profile puts 403 MB in one buffer of
libvpx's tokens, sized for three planes at full resolution whatever the
chroma and zeroed when the encoder is made, so that every page of it is
resident before the first frame; about ten frame buffers of 31 MB each, a
4:4:4 picture with libvpx's 160-pixel border (the source's copies and the
references); and 124 MB of bitstream buffers for the tile workers. Making a
4K encoder takes 56 ms and its first keyframe 47, against 28 for a keyframe
once its memory is in use.

## The walk on a slow link

What the walk ([The VP9 encoding](architecture.md#the-vp9-encoding)) buys a
slow link, by `walk-sim`'s model of the browser's link. Measured 2026-10-26 on
an eight-core AMD EPYC (Milan) VM, idle and with no swap, with this machine's
encode times, on the whole window and mixed captures at 1920×1080,
2880×1800@2 and 3840×2160@2, the dial's ceiling 90, over links of 5 to
50 Mbit/s: at 25 Mbit/s and over the walk barely moves, and walked and held
are the same within a tenth. At 5, over the mixed session at 2880×1800@2, the
walked stream painted 12.8 frames a second, 95% of changes within 236 ms, at
a mean dial of 55, where held painted 5.2 and 1,664 ms; on a link that falls
from 50 Mbit/s to 5 for 8 s, the window at 4K was painted within 270 ms
against 618.

## Done: motion at speed 9

An inter frame that codes half the picture or more is searched at speed 9, and
every other frame at speed 7 ([The VP9 encoding](architecture.md#the-vp9-encoding)).
A 4K scroll went from 45 to 27 ms a frame and a 4K video from 45 to 31; the
processor's time a frame fell 37–44% at 1080p and 4K, for 1–15% more bytes at
4K and 7–37% at 1080p, and at most 0.4 dB below while the picture moved.
Terminal, drag and uncover are where they were: a window dragged or uncovered,
a quarter to a third of the picture, sent 1.4 to 2.3 times the bytes at 9.

## Left to try

Nothing in this repository. One change in libvpx would cut a stream's making
and some of what it holds, and is libvpx-prebuilt's to carry, as a patch on
the commit it pins:

- **The token buffer not zeroed.** libvpx writes each superblock row's tokens
  from the start of its own worst-case slice and reads back only what it
  wrote, so the zeroing buys nothing: built without it, libvpx made the same
  streams byte for byte of a 4K terminal, video and uncover and a 1080p
  scroll. Allocated without it, a 4K 4:4:4 encoder is made in 16 ms at 61 MB
  rather than 56 ms at 445, and its first
  keyframe takes 54 ms against 47. What it holds once coding falls only from
  825 MB to 715 on a machine whose transparent huge pages are `always`, as
  this Debian 13 one's are: a keyframe touches a 2 MB page of most slices.
  Without huge pages it is about half, 410–420 MB against 790 (glibc's
  malloc), but a first 4K keyframe then takes 158 ms against 65.

## Not worth it

- **Tighter damage.** The rectangles a session reads are looser than what
  changed: of the 16×16 blocks they touch at 4K, 58% differ from the frame
  before on an uncover, 62% on a drag and 38% on a scroll, at 1080p 59%, 72%
  and 48%. But libvpx already finds an unchanged block cheap, and what costs
  is the blocks that did change and the pass over the whole picture above.
  Each rectangle narrowed to the box of its blocks that differ took 1–6% off
  the processor's time, 4% at 4K, at bytes within 3%, before the comparison
  that would find them. Narrowed to the blocks themselves, a drag cost no less
  and sent 18% more bytes, the holes breaking libvpx's motion vector
  prediction and its map of the active blocks, and a scroll fell under half
  the picture into speed 7 and cost 61% more; at speed 9 still, it cost 20%
  less for 18% more bytes. To whole 64×64 superblocks: an uncover 8–15% less
  and 7–9% fewer bytes, a scroll 9–15% less for 2% more, and a drag no less for
  6–13% more.
- **Eight threads on eight cores.** Eight took a 4K frame 12–15% less time
  than six on an idle machine (video 29.3 ms against 25.9, a scroll 26.4
  against 23.1, a drag 18.4 against 15.7), and with two cores kept busy by
  another process 5–7% more (31.2, 27.7 and 19.0), where six were no slower
  than idle and the busy process lost nothing either way. The two cores kept
  back pay for themselves when the desktop is busy making the next frame,
  which is when the encoder is busiest too.
- **A thread pool for the conversion.** A whole 4K picture converts in 0.94
  ms on four threads (3.2 on one), and starting and joining its three threads
  costs 80–90 µs: a pool would save under 0.1 ms a frame.
- **Scene detection off** (`VP9E_SET_RTC_EXTERNAL_RATECTRL`), libvpx's sum
  of differences over the source on the encoder's own thread. 15–21% off a 4K
  video (121 ms of the processor a frame against 95) and a 4K terminal (18.8
  against 15.9), but it is also what tells screen tuning that much of the
  picture moved and to search wider: a scroll sent 4.1 times the bytes at 4K
  and 5.1 times at 1080p, a 1080p drag and a 4K uncover 1.9 times. It is set
  for the encoder's life, so it cannot follow the frame.
- **4:2:0 for video.** A third of video's time, but a new chroma is a new
  stream from a keyframe, and a stream's chroma is the browser's.
- **The default tuning for video.** 16–28% off video's time, at speed 9 and
  7, but a scroll and a
  video both change the whole picture, so the damage cannot tell them apart,
  and a scroll without screen tuning is 4.5 times the bytes.
- **Speed 8 for smaller changes.** 5–15% faster on a drag or an uncover for
  6–28% more bytes.
- **A lower dial.** 15% at most.
