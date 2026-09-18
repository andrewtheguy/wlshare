# Roadmap

Work that is planned but not started. Once something here is done, its design
moves to [`architecture.md`](architecture.md) and it comes off this list.

## A lossless audio codec

The QEMU Audio extension carries raw PCM, and the silence extension (`WLSA`,
see [Silence](architecture.md#silence)) only removes the cost of samples that
are exactly silent. Anything audible still goes out at full rate: 48 kHz
stereo 16-bit is 192 kB/s, about 1.5 Mbit/s, on the same TCP stream as the
pixels.

A lossless codec would cut that for audible samples too, and still deliver
exactly the PCM the capture produced, so the gateway's Opus encode (the only
lossy step) stays the only one.

**FLAC** is the candidate:

- Music and speech usually compress to about 50–60% of PCM. Silence and
  near-silence compress almost to nothing, because a constant or low-energy
  subframe is a few bytes. Near-silence is the dither and noise floor the
  exact-silence rule of `WLSA` lets through.
- Each frame decodes independently, so a buffer dropped by a session that
  fell behind costs nothing but its own samples.
- At the fastest compression levels, encoding costs very little CPU at 48 kHz
  stereo. It adds no latency beyond the block size, which can match the 20 ms
  capture buffer (960 frames).
- Pure-Rust implementations exist on both sides, `flacenc` to encode and
  `claxon` to decode, so neither crate would take on a C dependency. Whether
  they are good enough is still to be measured.

### Shape

- A private pseudo-encoding next to `-259`, listed only by the remotex
  gateway, as `WLSA` is. A server message carries one FLAC frame, sent where
  an `audio_data` would have been. `begin`, `end` and the client's choice of
  format stay as they are. The FLAC stream header (`STREAMINFO`) is never
  sent: the format is the one the client set, and each frame's own header
  carries its block size.
- FLAC stores only signed samples, and a client may choose U8, U16 or U32.
  The encoder flips the top bit of each unsigned sample, which is the same
  as subtracting the midpoint. That maps the unsigned range exactly onto the
  signed one of the same width, with silence landing on zero. The decoder
  flips the bit back, and since the flip is its own inverse, the client gets
  the original values bit for bit. Signed formats are encoded as they are.
- It would replace the silence extension rather than sit beside it, because
  FLAC already encodes silence in a few bytes. Once it lands, `WLSA` and
  `0xE4` go (no legacy paths).
- The encoder belongs in `wlshare-rfb`, like ZRLE. Its tests decode every
  frame with an independent decoder and check that the output matches the
  input sample for sample.

### Open questions

- Is the pure-Rust encoder good enough in ratio and CPU, or would
  `libFLAC` be worth its system dependency?
- What compression level? Measure ratio against CPU on the capture's
  thread or the session's.
- Should encoding happen on the capture thread (off the session's task) or
  in `flush_audio`, where the silence rule runs now?
