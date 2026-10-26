//! The VP9 encoding: the whole framebuffer as one VP9 stream, for a client
//! that would rather have a picture that moves than one that is exact.
//!
//! A private encoding, [`crate::ENCODING_VP9`], which only a client that lists
//! it is ever sent. Its rectangle always covers the whole framebuffer, and its
//! body is the dial the frame was coded at, a length word and one VP9 frame
//! ([`frame_header`]):
//!
//! ```text
//! u8 quality   the 1–100 dial the frame was coded at
//! u32 length   the frame's bytes, at most MAX_BODY
//! u8[length]   one VP9 frame (a superframe counts as one)
//! ```
//!
//! The quality is what the server's walk had the encoder at for this frame, or
//! the ceiling for a settle's: the one thing a client that passes the frames on
//! undecoded cannot read from them. The rectangles of successive updates are
//! one stream, each frame coded against the frames before it, so none of them
//! means anything alone. The stream begins with a keyframe, and so does every
//! picture a decoder has to be able to start from: the first after the
//! encoding is listed, the first at a new framebuffer size, and the one that
//! answers a non-incremental request.
//!
//! Every frame is 8-bit VP9 converted from the framebuffer as BT.601 at studio
//! swing, which the keyframe header says so a decoder does not guess. Its
//! chroma is **4:4:4** (profile 1) unless the client's list asks for **4:2:0**
//! (profile 0, [`ENCODING_VP9_SUBSAMPLED`]), which is what a browser whose own
//! decoder takes no profile 1 is sent. The client's pixel format does not apply
//! to this encoding; what it decodes to is its own business.
//!
//! What the stream is to be rides the encoding list beside the encoding
//! ([`Vp9Stream`]).

use thiserror::Error;

use crate::{QUALITY_MAX, QUALITY_MIN};

/// Listed beside [`crate::ENCODING_VP9`] with a quality 1–100 added, names the
/// ceiling the stream's walk never goes above: the ASCII bytes `WLQ` and the
/// value, the way Tight's quality levels ride `SetEncodings`. A value outside
/// the dial is not one of these. A VP9 list carries one: the server has no
/// quality of its own. The tiles encoding takes its dial the same way.
pub const ENCODING_VP9_QUALITY_BASE: i32 = 0x574c_5100;

/// Listed beside [`crate::ENCODING_VP9`], holds the dial at its ceiling: the
/// ASCII bytes `WLSD`. The walk then hears nothing in a fence, and only a
/// blocked write moves it.
pub const ENCODING_VP9_HELD: i32 = 0x574c_5344;

/// Listed beside [`crate::ENCODING_VP9`], asks for the stream at 4:2:0, VP9
/// profile 0, rather than 4:4:4: the ASCII bytes `WLSS`, subsampled. What
/// every browser's decoder takes, its hardware one included, where some take
/// no profile 1 at all.
pub const ENCODING_VP9_SUBSAMPLED: i32 = 0x574c_5353;

/// The bytes of a rectangle's body before its frame: the dial and the length.
pub const FRAME_HEADER_LEN: usize = 5;

/// How a stream samples colour, which is its VP9 profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    /// 4:4:4, profile 1: a colour sample per pixel.
    Yuv444,
    /// 4:2:0, profile 0: a colour sample per 2×2 pixels.
    Yuv420,
}

/// What the client wants the VP9 stream to be, read from its `SetEncodings`:
/// the pseudo-encodings above, beside the encoding itself. Pseudo-encodings
/// rather than a message because a server that is not wlshare ignores an
/// encoding it does not know, where a message it does not know ends the
/// connection; and they ride the very list that names the encoding, so the
/// first frame is already what was asked for. A list that names VP9 without a
/// quality is fatal ([`Vp9ListError::NoQuality`]): the server has no quality of
/// its own. Without `WLSD` it walks, and without `WLSS` it is 4:4:4.
///
/// A change of quality or of the walk moves the running encoder's dial
/// without a keyframe; a change of chroma is a new stream, from a keyframe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vp9Stream {
    pub quality: u8,
    pub adaptive: bool,
    pub chroma: Chroma,
}

impl Vp9Stream {
    /// The stream `encodings` asks for: `None` where the list does not name
    /// [`crate::ENCODING_VP9`], and an error where it does and names no
    /// quality on the dial beside it.
    pub fn listed(encodings: &[i32]) -> Result<Option<Self>, Vp9ListError> {
        let has = |e: i32| encodings.contains(&e);
        if !has(crate::ENCODING_VP9) {
            return Ok(None);
        }
        let quality = crate::listed_quality(encodings).ok_or(Vp9ListError::NoQuality)?;
        let chroma = if has(ENCODING_VP9_SUBSAMPLED) { Chroma::Yuv420 } else { Chroma::Yuv444 };
        Ok(Some(Self { quality, adaptive: !has(ENCODING_VP9_HELD), chroma }))
    }

    /// The list that asks for this stream: the encoding, the quality always,
    /// the held dial where it is held, and 4:2:0 where it is subsampled. A
    /// client puts it ahead of the rest of its list, where a list read as a
    /// preference puts what it would rather have.
    pub fn encodings(self) -> Vec<i32> {
        let quality = self.quality.clamp(QUALITY_MIN, QUALITY_MAX);
        let mut listed = vec![crate::ENCODING_VP9, ENCODING_VP9_QUALITY_BASE + i32::from(quality)];
        if !self.adaptive {
            listed.push(ENCODING_VP9_HELD);
        }
        if self.chroma == Chroma::Yuv420 {
            listed.push(ENCODING_VP9_SUBSAMPLED);
        }
        listed
    }
}

/// Why a list that names VP9 asks for no stream the server codes. Fatal to the
/// connection, as a malformed message is.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Vp9ListError {
    #[error("the list names VP9 without a quality 1–100 beside it")]
    NoQuality,
}

/// Why a VP9 rectangle's body is not one.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Vp9FrameError {
    #[error("a VP9 frame coded at q{0}, which is not on the 1 to 100 dial")]
    Quality(u8),
    #[error("a VP9 frame of {0} bytes, past the {max} bytes one can be", max = crate::MAX_BODY)]
    TooLong(usize),
}

/// The head of a VP9 rectangle's body: the dial the frame was coded at and the
/// length of the frame that follows.
pub fn frame_header(quality: u8, len: u32) -> [u8; FRAME_HEADER_LEN] {
    let [a, b, c, d] = len.to_be_bytes();
    [quality, a, b, c, d]
}

/// The dial and the frame's length a VP9 rectangle's body begins with: an
/// error for a dial off it, or a length past [`crate::MAX_BODY`], which a
/// server never sends.
pub fn parse_frame_header(header: &[u8; FRAME_HEADER_LEN]) -> Result<(u8, usize), Vp9FrameError> {
    let quality = header[0];
    if !(QUALITY_MIN..=QUALITY_MAX).contains(&quality) {
        return Err(Vp9FrameError::Quality(quality));
    }
    let len = crate::u32_at(header, 1) as usize;
    if len > crate::MAX_BODY {
        return Err(Vp9FrameError::TooLong(len));
    }
    Ok((quality, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pseudo_encodings_are_their_ascii_names() {
        assert_eq!(ENCODING_VP9_QUALITY_BASE, i32::from_be_bytes(*b"WLQ\0"));
        assert_eq!(ENCODING_VP9_HELD, i32::from_be_bytes(*b"WLSD"));
        assert_eq!(ENCODING_VP9_SUBSAMPLED, i32::from_be_bytes(*b"WLSS"));
    }

    /// The stream a list asks for, and the list a stream is asked with, each
    /// read back by the other.
    #[test]
    fn a_stream_is_what_its_list_asks_for() {
        let q = |quality: i32| ENCODING_VP9_QUALITY_BASE + quality;
        assert_eq!(Vp9Stream::listed(&[16, q(60)]), Ok(None), "no VP9, no stream");
        assert_eq!(Vp9Stream::listed(&[crate::ENCODING_VP9, 16]), Err(Vp9ListError::NoQuality));
        assert_eq!(Vp9Stream::listed(&[crate::ENCODING_VP9, q(0), q(101)]), Err(Vp9ListError::NoQuality));
        assert_eq!(
            Vp9Stream::listed(&[crate::ENCODING_VP9, q(60), 16]),
            Ok(Some(Vp9Stream { quality: 60, adaptive: true, chroma: Chroma::Yuv444 }))
        );
        assert_eq!(
            Vp9Stream::listed(&[ENCODING_VP9_SUBSAMPLED, q(90), ENCODING_VP9_HELD, crate::ENCODING_VP9]),
            Ok(Some(Vp9Stream { quality: 90, adaptive: false, chroma: Chroma::Yuv420 }))
        );
        for adaptive in [true, false] {
            for chroma in [Chroma::Yuv444, Chroma::Yuv420] {
                let stream = Vp9Stream { quality: 75, adaptive, chroma };
                assert_eq!(Vp9Stream::listed(&stream.encodings()), Ok(Some(stream)));
            }
        }
        let held = Vp9Stream { quality: 90, adaptive: false, chroma: Chroma::Yuv420 };
        assert_eq!(held.encodings(), [crate::ENCODING_VP9, 0x574c_515a, ENCODING_VP9_HELD, ENCODING_VP9_SUBSAMPLED]);
    }

    #[test]
    fn a_frame_header_is_the_dial_and_the_length() {
        assert_eq!(frame_header(60, 0x0102_0304), [60, 1, 2, 3, 4]);
        assert_eq!(parse_frame_header(&frame_header(60, 0x0102_0304)), Ok((60, 0x0102_0304)));
        assert_eq!(parse_frame_header(&frame_header(0, 1)), Err(Vp9FrameError::Quality(0)));
        assert_eq!(parse_frame_header(&frame_header(101, 1)), Err(Vp9FrameError::Quality(101)));
        let over = crate::MAX_BODY as u32 + 1;
        assert_eq!(parse_frame_header(&frame_header(100, over)), Err(Vp9FrameError::TooLong(over as usize)));
    }
}
