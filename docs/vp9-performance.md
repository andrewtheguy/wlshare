# VP9 performance

What the `WLSV` stream's encoder costs, where the time goes, and what is left
to try. Measured on 2026-10-25 on an eight-core AMD EPYC (Rome) VM, idle, with
the encoder on six threads (the machine's cores less two), 4:4:4, screen
tuning and quality 90, over the kept captures (`frame-samples/`, 120 frames of
each scenario; see [Capturing what the encoder is handed](architecture.md#capturing-what-the-encoder-is-handed)).
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

## Done: motion at speed 9

An inter frame that codes half the picture or more is searched at speed 9, and
every other frame at speed 7 ([The VP9 encoding](architecture.md#the-vp9-encoding)).
A 4K scroll went from 45 to 27 ms a frame and a 4K video from 45 to 31; the
processor's time a frame fell 37–44% at 1080p and 4K, for 1–15% more bytes at
4K and 7–37% at 1080p, and at most 0.4 dB below while the picture moved.
Terminal, drag and uncover are where they were.

## Left to try

In the order they look worth it.

1. **Drag and uncover at 4K.** Still 60–90 ms of the processor a frame,
   uncover's p95 145 ms. Uncover's frames say a third of the picture changed
   and cost 45% of a whole video frame (90 ms against 199), more than their
   share. If the damage is looser than what changed, the encoder searches
   blocks that did not: compare, on the captures, the blocks the rectangles
   touch with the blocks whose pixels differ from the frame before. Tighter
   damage would cut this without touching the coding.
2. **The thread count.** Six threads on eight cores kept 4.4 cores busy; eight
   took a 4K frame from 42 to 37 ms, 13%, by giving the encoder the two cores
   kept for the compositor and the session. It matters only below ten cores,
   where the cap of eight is not reached, and is a judgement of what else the
   machine runs rather than a fix.
3. **An encoder's making.** A 4K encoder and its first frame fault in 0.4–0.7
   GB, which every session and every resize pays: a fresh process's first 4K
   keyframe took 60–90 ms against about 24 once the memory was warm, and the
   memory is held for the session. It is libvpx's own buffers; what a session
   actually holds is the thing to measure before deciding anything.
4. **Small.** The conversion is 3–5% of a frame and spawns its threads for
   every frame, where a pool kept for the stream would save a millisecond or
   two at 4K. The part of a frame that is not parallel, about 16 ms at 4K, is
   libvpx's bitstream and its copy of the source, out of reach.

## Not worth it

- **4:2:0 for video.** A third of video's time, but a new chroma is a new
  stream from a keyframe, and a stream's chroma is the browser's.
- **The default tuning for video.** 16–28% off video's time, at speed 9 and
  7, but a scroll and a
  video both change the whole picture, so the damage cannot tell them apart,
  and a scroll without screen tuning is 4.5 times the bytes.
- **Speed 8 for smaller changes.** 5–15% faster on a drag or an uncover for
  6–28% more bytes.
- **A lower dial.** 15% at most.
