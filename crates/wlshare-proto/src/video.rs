//! The video encoding: the whole framebuffer as one H.264 stream, for a client
//! that would rather have a picture that moves than one that is lossless.
//!
//! A private encoding, [`crate::ENCODING_VIDEO`], which only a client that
//! lists it is ever sent. What the list says of the stream is its chroma
//! ([`Chroma`]) and nothing else: the rest is the server's, which has one way
//! to code it. Its rectangle
//! always covers the whole framebuffer, and its body is a length word and one
//! access unit ([`frame_header`]):
//!
//! ```text
//! u32 length   the access unit's bytes, at most MAX_BODY
//! u8[length]   one H.264 access unit, Annex B
//! ```
//!
//! The rectangles of successive updates are one stream, each frame coded
//! against the frames before it, so none of them means anything alone. The
//! stream begins with an IDR picture, its sequence and picture parameter sets
//! ahead of it, and so does every picture a decoder has to be able to start
//! from: the first after the encoding is listed, the first at a new
//! framebuffer size, and the one that answers a non-incremental request. No
//! frame is held back for another: each one in is one out, in the order coded.
//!
//! Every frame is 8 bits a sample, converted from the framebuffer as BT.601
//! at studio swing, which the stream's VUI says so a decoder does not guess,
//! of the framebuffer's size rounded up to even sides: a decoder draws the
//! picture at the rectangle, cut to it from its top-left. At 4:2:0 the
//! stream is High profile, one colour sample for each 2×2 block of pixels,
//! which every decoder takes, the hardware's among them; at 4:4:4, which a
//! list asks for with [`crate::ENCODING_VIDEO_444`] beside the encoding, it
//! is High 4:4:4 Predictive, a colour sample for every pixel, so coloured
//! text keeps its edges, for a client whose decoder takes that. The client's
//! pixel format does not apply to this encoding; what it decodes to is its
//! own business.
//!
//! A stream whose desktop has stopped changing is sent frames of no damage
//! that refine the picture it was left with, as any other frame of the
//! stream: nothing in the rectangle tells them apart, and a decoder decodes
//! them as it does the rest.

use thiserror::Error;

/// How much of a picture's colour the stream carries, which is all a list
/// says of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chroma {
    /// The encoding alone: 4:2:0, High profile.
    Yuv420,
    /// `WLS4` ([`crate::ENCODING_VIDEO_444`]) beside it: 4:4:4, High 4:4:4
    /// Predictive.
    Yuv444,
}

impl Chroma {
    /// The chroma `encodings` asks the stream to be, or `None` where the list
    /// does not name the video encoding.
    pub fn listed(encodings: &[i32]) -> Option<Self> {
        let has = |e: i32| encodings.contains(&e);
        has(crate::ENCODING_VIDEO).then(|| {
            if has(crate::ENCODING_VIDEO_444) {
                Self::Yuv444
            } else {
                Self::Yuv420
            }
        })
    }

    /// The list that asks for the stream at this chroma: the encoding, and
    /// `WLS4` where it is 4:4:4. A client puts it ahead of the rest of its
    /// list, and lists no tiles beside it.
    pub fn encodings(self) -> Vec<i32> {
        match self {
            Self::Yuv420 => vec![crate::ENCODING_VIDEO],
            Self::Yuv444 => vec![crate::ENCODING_VIDEO, crate::ENCODING_VIDEO_444],
        }
    }
}

/// The bytes of a rectangle's body before its access unit: the length.
pub const FRAME_HEADER_LEN: usize = 4;

/// Why a video rectangle's body is not one.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VideoFrameError {
    #[error("a video frame of {0} bytes, past the {max} bytes one can be", max = crate::MAX_BODY)]
    TooLong(usize),
}

/// The head of a video rectangle's body: the length of the access unit that
/// follows.
pub fn frame_header(len: u32) -> [u8; FRAME_HEADER_LEN] {
    len.to_be_bytes()
}

/// The access unit's length a video rectangle's body begins with: an error
/// for a length past [`crate::MAX_BODY`], which a server never sends.
pub fn parse_frame_header(header: &[u8; FRAME_HEADER_LEN]) -> Result<usize, VideoFrameError> {
    let len = crate::u32_at(header, 0) as usize;
    if len > crate::MAX_BODY {
        return Err(VideoFrameError::TooLong(len));
    }
    Ok(len)
}

#[cfg(test)]
mod tests;
