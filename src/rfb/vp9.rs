//! The VP9 encoding's server side: the framebuffer's pixels in, and each
//! frame out in the rectangle's body the encoding has ([`wlshare_proto::vp9`]):
//! the dial it was coded at, its length and the frame.
//!
//! **What a frame holds.** Every frame is 8-bit VP9 converted from the
//! framebuffer as BT.601 at studio swing. Its chroma is **4:4:4** (profile 1) —
//! a colour sample per pixel, since the loss 4:2:0 costs a desktop is its
//! text's colour and not its edges — unless the client's list asks for
//! **4:2:0** (profile 0), which the zDesk gateway does for a browser whose own
//! decoder takes no profile 1. The quantizer is pinned by a 1–100 quality
//! dial, which only the encoder's owner moves ([`Vp9Encoder::set_quality`]): no
//! bitrate, no adaptive quantization, no dropped frames, so every frame is sent
//! at exactly the dial's quality at the time.
//!
//! The coding itself is [`wlshare_vp9`]'s, by libvpx as that crate sets it up
//! for a desktop. What this module owns is the framebuffer's pixels in, and the
//! frame framed as [`wlshare_proto::vp9::frame_header`] lays it out.

use thiserror::Error;

pub use wlshare_vp9::{Chroma, QUALITY_MAX, QUALITY_MIN, Rect};

/// The codec's chroma for the one a client's list asks for
/// ([`wlshare_proto::vp9::Vp9Stream`]).
pub fn chroma(listed: wlshare_proto::vp9::Chroma) -> Chroma {
    match listed {
        wlshare_proto::vp9::Chroma::Yuv444 => Chroma::Yuv444,
        wlshare_proto::vp9::Chroma::Yuv420 => Chroma::Yuv420,
    }
}

/// Why a picture could not be encoded, or in the tests a frame decoded.
#[derive(Debug, Error)]
pub enum Vp9Error {
    /// The encoder, or the pixels in front of it, refused; in the tests the
    /// decoder too.
    #[error(transparent)]
    Codec(#[from] wlshare_vp9::Error),
    #[error("a frame of {0} bytes is over the {max}-byte ceiling", max = wlshare_proto::MAX_BODY)]
    FrameTooLong(usize),
    #[cfg(test)]
    #[error("the frame is {0}x{1} and its rectangle {2}x{3}")]
    Size(u32, u32, usize, usize),
}

/// How many threads the tests' decoder gets: half the machine, at most four.
/// A decode gains nothing past the stream's tile columns.
#[cfg(test)]
fn decoder_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get() / 2).clamp(1, 4)
}

/// One connection's VP9 stream, server side: a stream at one picture size,
/// which converts the framebuffer and codes it. A framebuffer of another size
/// needs another of these, whose first frame is a keyframe by construction.
pub struct Vp9Encoder {
    stream: wlshare_vp9::Stream,
}

impl Vp9Encoder {
    /// An encoder for a `width`×`height` picture at `quality` (1–100) and
    /// `chroma`, on as many threads as this machine gives one.
    pub fn new(width: u16, height: u16, quality: u8, chroma: Chroma) -> Result<Self, Vp9Error> {
        Self::with_threads(width, height, quality, chroma, wlshare_vp9::encoder_threads())
    }

    /// [`Self::new`] on `threads` threads, which a stream's tile columns
    /// follow: for a stream that is to be the same on any machine.
    pub fn with_threads(width: u16, height: u16, quality: u8, chroma: Chroma, threads: usize) -> Result<Self, Vp9Error> {
        Ok(Self { stream: wlshare_vp9::Stream::new(width, height, quality, chroma, threads)? })
    }

    /// The picture size this encoder codes.
    pub fn size(&self) -> (u16, u16) {
        self.stream.size()
    }

    /// How this encoder's stream samples colour.
    #[cfg(test)]
    pub fn chroma(&self) -> Chroma {
        self.stream.chroma()
    }

    /// The quality the next frame is coded at, clamped to the dial.
    pub fn quality(&self) -> u8 {
        self.stream.quality()
    }

    /// The coarsest quality any block of the client's picture was last coded
    /// at: below the stream's ceiling while a frame the link coarsened has left
    /// blocks no finer frame has coded since, which a frame of damage does not
    /// do for the blocks it skips and a whole frame does for all of them.
    pub fn coarsest(&self) -> u8 {
        self.stream.coarsest()
    }

    /// Move the dial on the running encoder, clamped to it. The next frame is
    /// coded at the new quantizer against the frames before it: rebuilding the
    /// encoder would cost a keyframe, the most bytes a frame can be, at the
    /// moment a slow link can least afford them. A libvpx that will not be
    /// taken to the dial is the error of the frame that would be coded at it.
    pub fn set_quality(&mut self, quality: u8) {
        self.stream.set_quality(quality);
    }

    /// Read the picture — [`Self::size`] of `B, G, R, X` pixels whose rows
    /// are `stride` bytes apart — for the next frame to carry
    /// ([`Self::encode`]). It is read into the encoder's own planes, so
    /// `pixels` is the framebuffer itself, for as long as the conversion
    /// takes and no longer: nothing of it is kept.
    ///
    /// `changed` is the damage since the picture read before, or `None` for
    /// a picture that may differ anywhere. With it only the rectangles are
    /// read from `pixels`, at 4:2:0 the 2×2 blocks they touch, which the
    /// framebuffer holds as it holds the rest. An encoder's first read is the
    /// whole picture whatever `changed` says.
    pub fn read(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> Result<(), Vp9Error> {
        Ok(self.stream.read_bgrx(pixels, stride, changed)?)
    }

    /// Encode the picture as it was last read and append the rectangle's body
    /// to `out`: the dial, the length word and the frame. `keyframe` makes it one a
    /// decoder can start from; an encoder's first frame is one either way.
    ///
    /// Only the blocks the damage read since the last frame touches are
    /// coded: the rest of the frame is the picture the client already holds,
    /// at the quality it holds it. A keyframe, a picture read whole and the
    /// frame after one that produced nothing are the whole picture.
    ///
    /// Returns whether a body was appended: `false`, with `out` as it was,
    /// when libvpx made no frame of the picture
    /// ([`wlshare_vp9::Error::NoFrame`]), since an empty rectangle is not a
    /// frame a client can decode and the pixels are the next frame's to
    /// carry. An error leaves `out` as it was too.
    pub fn encode(&mut self, keyframe: bool, out: &mut Vec<u8>) -> Result<bool, Vp9Error> {
        let quality = self.stream.quality();
        self.framed(out, quality, |stream, out| stream.encode(keyframe, out))
    }

    /// Settle the picture at `quality`: encode it as it was last read, whole
    /// and at that quality, and leave the dial where it was. What a desktop
    /// that went quiet while its link had it coarse is sent once: an inter
    /// frame unless `keyframe` asks, which sharpens every block, with the
    /// frames after it at what the link bears again. The body says it was
    /// coded at `quality`, not at the dial.
    ///
    /// Returns whether a body was appended, as [`Self::encode`] does.
    pub fn settle(&mut self, quality: u8, keyframe: bool, out: &mut Vec<u8>) -> Result<bool, Vp9Error> {
        let quality = quality.clamp(QUALITY_MIN, QUALITY_MAX);
        self.framed(out, quality, |stream, out| stream.settle(quality, keyframe, out))
    }

    /// Append a rectangle's body to `out`: `quality`, the dial the frame is
    /// coded at, the length word, and the frame `encode` appends. Nothing is
    /// appended for an encode that produced no frame, failed, or made a frame
    /// over the ceiling.
    fn framed(&mut self, out: &mut Vec<u8>, quality: u8, encode: impl FnOnce(&mut wlshare_vp9::Stream, &mut Vec<u8>) -> Result<bool, wlshare_vp9::Error>) -> Result<bool, Vp9Error> {
        let body_at = out.len();
        out.extend_from_slice(&wlshare_proto::vp9::frame_header(quality, 0));
        match encode(&mut self.stream, out) {
            Ok(_) => {}
            Err(wlshare_vp9::Error::NoFrame) => {
                out.truncate(body_at);
                return Ok(false);
            }
            Err(e) => {
                out.truncate(body_at);
                return Err(e.into());
            }
        }
        let len = out.len() - body_at - wlshare_proto::vp9::FRAME_HEADER_LEN;
        if len > wlshare_proto::MAX_BODY {
            out.truncate(body_at);
            return Err(Vp9Error::FrameTooLong(len));
        }
        out[body_at..body_at + wlshare_proto::vp9::FRAME_HEADER_LEN].copy_from_slice(&wlshare_proto::vp9::frame_header(quality, len as u32));
        Ok(true)
    }
}

/// One connection's VP9 stream, client side, which the tests read the encoder
/// back with: every VP9 rectangle is decoded by the same decoder, libvpx's,
/// in the order they arrive, since each frame is coded against the ones
/// before it.
#[cfg(test)]
struct Vp9Decoder {
    decoder: wlshare_vp9::Decoder,
}

#[cfg(test)]
impl Vp9Decoder {
    fn new() -> Result<Self, Vp9Error> {
        Ok(Self { decoder: wlshare_vp9::Decoder::new(decoder_threads())? })
    }

    /// Decode a VP9 rectangle payload — the frame, without its dial and length word —
    /// of `width`×`height` pixels into `out`, whose first byte is the
    /// rectangle's first pixel and whose rows are `stride` bytes apart, as
    /// `B, G, R, X`.
    fn decode_rect(&mut self, payload: &[u8], width: usize, height: usize, out: &mut [u8], stride: usize) -> Result<(), Vp9Error> {
        let decoded = self.decoder.decode(payload)?;
        let (w, h) = decoded.size();
        if (w as usize, h as usize) != (width, height) {
            return Err(Vp9Error::Size(w, h, width, height));
        }
        Ok(decoded.write_bgrx(out, stride)?)
    }
}

/// The encoder read back by the decoder, through the rectangle's framing. The
/// coding itself is proved in `wlshare-vp9`; what these
/// hold is the half this module owns — the quality byte, the length word, its ceiling, the pixels in and out — and
/// that a client's decoder refuses a frame of another size.
#[cfg(test)]
mod tests {
    use super::*;

    /// A dark terminal with one-pixel coloured glyph stems at odd columns: the
    /// picture 4:2:0 could not carry, since each stem would share its colour
    /// sample with three pixels of background.
    fn stems(width: usize, height: usize) -> (Vec<u8>, Vec<(usize, usize)>) {
        let mut pixels: Vec<u8> = [30u8, 30, 30, 0].repeat(width * height);
        let mut at = Vec::new();
        for y in (0..height).step_by(2) {
            for x in (1..width).step_by(4) {
                pixels[(y * width + x) * 4..][..4].copy_from_slice(&[198, 121, 255, 0]);
                at.push((x, y));
            }
        }
        (pixels, at)
    }

    /// Encode `pixels` and read back the dial and the length word, returning the frame.
    fn encode(encoder: &mut Vp9Encoder, pixels: &[u8], keyframe: bool) -> Vec<u8> {
        let (width, _) = encoder.size();
        let mut out = vec![0xEE];
        encoder.read(pixels, usize::from(width) * 4, None).expect("a read");
        assert!(encoder.encode(keyframe, &mut out).expect("an encode"), "a frame");
        assert_eq!(out[0], 0xEE, "appended, not overwritten");
        assert_eq!(out[1], encoder.quality(), "the body says the dial the frame was coded at");
        let len = u32::from_be_bytes(out[2..6].try_into().unwrap()) as usize;
        assert_eq!(len, out.len() - 6, "the length word is the frame's");
        out.split_off(6)
    }

    /// The settle's rectangle is framed as any other and says the quality it was
    /// coded at, sharpens the picture a coarse dial left without a keyframe, and
    /// leaves the dial where it was.
    #[test]
    fn a_settle_sharpens_the_picture_and_leaves_the_dial() {
        let (width, height) = (64, 48);
        let (pixels, _) = stems(width, height);
        let mut encoder = Vp9Encoder::new(width as u16, height as u16, QUALITY_MIN, Chroma::Yuv444).unwrap();
        let mut decoder = Vp9Decoder::new().unwrap();
        let mut back = vec![0; width * height * 4];
        let mut error = |frame: &[u8]| {
            decoder.decode_rect(frame, width, height, &mut back, width * 4).unwrap();
            back.chunks(4).zip(pixels.chunks(4)).map(|(got, want)| (0..3).map(|c| u64::from(got[c].abs_diff(want[c]))).sum::<u64>()).sum::<u64>()
        };
        let coarse = error(&encode(&mut encoder, &pixels, false));

        let mut out = vec![0xEE];
        assert!(encoder.settle(QUALITY_MAX, false, &mut out).expect("a settle"), "a frame");
        assert_eq!((out[0], out[1]), (0xEE, QUALITY_MAX), "the body says the settle's quality, not the dial");
        assert_eq!(u32::from_be_bytes(out[2..6].try_into().unwrap()) as usize, out.len() - 6, "the length word is the frame's");
        let frame = out.split_off(6);
        assert!(wlshare_vp9::frame_header(&frame).is_some_and(|header| !header.keyframe), "the settle cost a keyframe");
        let settled = error(&frame);
        assert!(settled < coarse / 2, "the settle left the picture at error {settled} of {coarse}");
        assert_eq!((encoder.quality(), encoder.coarsest()), (QUALITY_MIN, QUALITY_MAX));

        // An encoder nothing was read into has nothing to settle.
        let mut out = vec![0xEE];
        let mut unread = Vp9Encoder::new(width as u16, height as u16, QUALITY_MIN, Chroma::Yuv444).unwrap();
        assert!(matches!(unread.settle(QUALITY_MAX, false, &mut out), Err(Vp9Error::Codec(wlshare_vp9::Error::Unread))));
        assert_eq!((out, unread.quality()), (vec![0xEE], QUALITY_MIN), "a settle that failed left something behind");
    }

    #[test]
    fn a_444_stream_keeps_a_one_pixel_stem_its_colour() {
        let (width, height) = (64, 48);
        let (pixels, at) = stems(width, height);
        let mut encoder = Vp9Encoder::new(width as u16, height as u16, QUALITY_MAX, Chroma::Yuv444).unwrap();
        let frame = encode(&mut encoder, &pixels, false);
        assert!(wlshare_vp9::frame_header(&frame).is_some_and(|header| header.keyframe && header.profile == Chroma::Yuv444.profile()), "an encoder's first frame is a 4:4:4 keyframe");

        let mut decoder = Vp9Decoder::new().unwrap();
        let stride = width * 4 + 8;
        let mut out = vec![0; stride * height];
        decoder.decode_rect(&frame, width, height, &mut out, stride).unwrap();
        let worst = at
            .iter()
            .map(|&(x, y)| {
                let got = &out[y * stride + x * 4..][..3];
                got.iter().zip([198u8, 121, 255]).map(|(a, b)| a.abs_diff(b)).max().unwrap()
            })
            .max()
            .unwrap();
        assert!(worst <= 24, "a stem pixel came back {worst} code values off");
    }

    /// A subsampled stream is profile 0 and decodes to its picture, framed as
    /// any other.
    #[test]
    fn a_420_stream_is_profile_0_and_decodes() {
        let (width, height) = (66, 34);
        let pixels = [40u8, 180, 90, 0].repeat(width * height);
        let mut encoder = Vp9Encoder::new(width as u16, height as u16, QUALITY_MAX, Chroma::Yuv420).unwrap();
        assert_eq!(encoder.chroma(), Chroma::Yuv420);
        let frame = encode(&mut encoder, &pixels, false);
        assert!(wlshare_vp9::frame_header(&frame).is_some_and(|header| header.keyframe && header.profile == 0), "an encoder's first frame is a 4:2:0 keyframe");
        let mut decoder = Vp9Decoder::new().unwrap();
        let mut out = vec![0; width * height * 4];
        decoder.decode_rect(&frame, width, height, &mut out, width * 4).unwrap();
        let last = &out[((height - 1) * width + width - 1) * 4..][..3];
        assert!(last.iter().zip([40u8, 180, 90]).all(|(a, b)| a.abs_diff(b) <= 8), "the last pixel came back {last:?}");
    }

    /// Only the frames that are asked to be keyframes are, the ones between
    /// decode against them, and the dial moves without one.
    #[test]
    fn keyframes_come_when_asked_and_the_dial_moves_between_them() {
        let (width, height) = (64, 32);
        let mut encoder = Vp9Encoder::new(width as u16, height as u16, 60, Chroma::Yuv444).unwrap();
        let mut decoder = Vp9Decoder::new().unwrap();
        let mut out = vec![0; width * height * 4];
        for step in 0..5usize {
            let mut pixels = [80u8, 40, 20, 0].repeat(width * height);
            for x in step * 4..step * 4 + 8 {
                for y in 0..height {
                    pixels[(y * width + x) * 4..][..3].copy_from_slice(&[240, 240, 240]);
                }
            }
            if step == 2 {
                encoder.set_quality(QUALITY_MAX);
                assert_eq!(encoder.quality(), QUALITY_MAX);
            }
            let frame = encode(&mut encoder, &pixels, step == 3);
            let header = wlshare_vp9::frame_header(&frame).expect("a VP9 frame");
            assert_eq!(header.keyframe, step == 0 || step == 3, "frame {step}");
            decoder.decode_rect(&frame, width, height, &mut out, width * 4).unwrap();
            let lit = &out[(step * 4 + 4) * 4..][..3];
            assert!(lit.iter().all(|&c| c > 200), "frame {step} did not decode to its own picture: {lit:?}");
        }
    }

    /// A frame of damage reads the rows its rectangles span and nothing else
    /// of the buffer, and leaves the rest of the client's picture as it was.
    /// An encoder's first frame and a keyframe have no picture to be damage
    /// to, and are read whole.
    #[test]
    fn a_frame_of_damage_reads_and_codes_only_what_it_names() {
        let (width, height) = (96usize, 80usize);
        let stride = width * 4;
        const BACK: [u8; 4] = [80, 40, 20, 0];
        const LIT: [u8; 4] = [240, 240, 240, 0];
        let damage = Rect { x: 32, y: 16, width: 32, height: 16 };
        let lit = |pixels: &mut [u8], rect: Rect| {
            for y in usize::from(rect.y)..usize::from(rect.y + rect.height) {
                for x in usize::from(rect.x)..usize::from(rect.x + rect.width) {
                    pixels[y * stride + x * 4..][..4].copy_from_slice(&LIT);
                }
            }
        };
        let at = |out: &[u8], x: usize, y: usize| -> [u8; 3] { out[y * stride + x * 4..][..3].try_into().unwrap() };
        let near = |got: [u8; 3], want: [u8; 4]| (0..3).all(|c| got[c].abs_diff(want[c]) <= 24);
        let frame = |encoder: &mut Vp9Encoder, pixels: &[u8], changed: Option<&[Rect]>, keyframe: bool| {
            let mut out = Vec::new();
            encoder.read(pixels, stride, changed).expect("a read");
            assert!(encoder.encode(keyframe, &mut out).expect("an encode"), "a frame");
            out.split_off(5)
        };
        let mut encoder = Vp9Encoder::new(width as u16, height as u16, QUALITY_MAX, Chroma::Yuv444).unwrap();
        let mut decoder = Vp9Decoder::new().unwrap();
        let mut out = vec![0; stride * height];
        let whole = BACK.repeat(width * height);

        // The first frame is told of damage it cannot be damage to.
        let first = frame(&mut encoder, &whole, Some(&[damage]), false);
        assert!(wlshare_vp9::frame_header(&first).is_some_and(|header| header.keyframe));
        decoder.decode_rect(&first, width, height, &mut out, stride).unwrap();
        assert!(near(at(&out, 4, 70), BACK) && near(at(&out, 48, 24), BACK), "the first frame was not the whole picture");

        // Damage, in a buffer whose other rows are not the picture.
        let mut rows = [0xAAu8, 0x55, 0xAA, 0].repeat(width * height);
        let span = usize::from(damage.y) * stride..usize::from(damage.y + damage.height) * stride;
        rows[span.clone()].copy_from_slice(&whole[span]);
        lit(&mut rows, damage);
        let second = frame(&mut encoder, &rows, Some(&[damage]), false);
        assert!(wlshare_vp9::frame_header(&second).is_some_and(|header| !header.keyframe));
        decoder.decode_rect(&second, width, height, &mut out, stride).unwrap();
        assert!(near(at(&out, 48, 24), LIT), "the damage came back {:?}", at(&out, 48, 24));
        for (x, y) in [(4, 4), (4, 24), (90, 24), (48, 4), (48, 70)] {
            assert!(near(at(&out, x, y), BACK), "({x}, {y}), outside the damage, came back {:?}", at(&out, x, y));
        }

        // A picture read whole is the whole buffer, and a keyframe where
        // one is asked.
        let mut all = whole.clone();
        lit(&mut all, Rect { x: 0, y: 64, width: 96, height: 16 });
        let third = frame(&mut encoder, &all, None, true);
        assert!(wlshare_vp9::frame_header(&third).is_some_and(|header| header.keyframe));
        decoder.decode_rect(&third, width, height, &mut out, stride).unwrap();
        assert!(near(at(&out, 48, 24), BACK) && near(at(&out, 48, 70), LIT), "the keyframe was not the whole picture");
    }

    #[test]
    fn a_frame_of_another_size_or_no_frame_at_all_is_an_error() {
        let mut encoder = Vp9Encoder::new(32, 16, 60, Chroma::Yuv444).unwrap();
        let frame = encode(&mut encoder, &[0u8; 32 * 16 * 4], false);
        let mut out = vec![0; 64 * 64 * 4];
        let mut decoder = Vp9Decoder::new().unwrap();
        assert!(matches!(decoder.decode_rect(&frame, 16, 16, &mut out, 64), Err(Vp9Error::Size(32, 16, 16, 16))));
        assert!(decoder.decode_rect(&[0xFF, 0x00, 0x12], 32, 16, &mut out, 128).is_err());
        assert!(matches!(decoder.decode_rect(&frame, 32, 16, &mut out[..10], 128), Err(Vp9Error::Codec(_))));
        assert!(matches!(Vp9Encoder::new(0, 16, 60, Chroma::Yuv444), Err(Vp9Error::Codec(wlshare_vp9::Error::Empty(0, 16)))));
        assert!(matches!(encoder.read(&[0; 12], 128, None), Err(Vp9Error::Codec(wlshare_vp9::Error::Buffer { .. }))));
    }
}

