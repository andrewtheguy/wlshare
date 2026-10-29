//! The video encoding: the whole framebuffer as one H.264 stream, for a client
//! that would rather have a picture that moves than one that is exact.
//!
//! A private encoding, [`crate::ENCODING_VIDEO`], which only a client that
//! lists it is ever sent, and nothing beside it in the list says what the
//! stream is to be: there is one way to code it, the server's. Its rectangle
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
//! Every frame is 8-bit 4:2:0 converted from the framebuffer as BT.601 at
//! studio swing, which the stream's VUI says so a decoder does not guess, of
//! the framebuffer's size rounded up to even sides: a decoder draws the
//! picture at the rectangle, cut to it from its top-left. The client's pixel
//! format does not apply to this encoding; what it decodes to is its own
//! business.

use thiserror::Error;

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
mod tests {
    use super::*;

    #[test]
    fn a_frame_header_is_the_length() {
        assert_eq!(frame_header(0x0102_0304), [1, 2, 3, 4]);
        assert_eq!(parse_frame_header(&frame_header(0x0102_0304)), Ok(0x0102_0304));
        let over = crate::MAX_BODY as u32 + 1;
        assert_eq!(parse_frame_header(&frame_header(over)), Err(VideoFrameError::TooLong(over as usize)));
    }
}
