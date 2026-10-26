//! The tiles encoding: the desktop as exact PNG tiles where it is still, and a
//! video stream of its own for each region that moves, for a client on a link
//! with room to spare.
//!
//! A private encoding, [`crate::ENCODING_TILES`], beside the VP9 encoding
//! ([`crate::vp9`]) rather than in place of it: a client lists one or the
//! other, and the dial beside it ([`listed`]).
//!
//! A pixel update in this encoding is any number of rectangles of it, drawn in
//! order, each a rectangle of the desktop whose body begins with a kind
//! ([`TileHeader`]):
//!
//! ```text
//! u8 kind
//! 0 PNG     u32 length | u8[length]  a PNG of exactly the rectangle's pixels
//! 1 START   u8 stream | u32 length | u8[length]  a region's first frame
//! 2 FRAME   u8 stream | u32 length | u8[length]  a region's next frame
//! 3 END     u8 stream                            the region's stream is over
//! ```
//!
//! - A **PNG** is the desktop's pixels in the rectangle exactly, 8-bit RGB or
//!   a palette of it, and the picture there until a later rectangle draws
//!   over it. It depends on nothing before it.
//! - A **START** begins a stream on `stream`, over its rectangle: an H.264
//!   keyframe, one access unit in Annex B with its parameter sets, 8-bit
//!   4:2:0, BT.601 at studio swing, of the rectangle's size rounded up to even
//!   sides. Each **FRAME** on that id is the next frame of that stream, over
//!   the same rectangle, coded against the frames before it. A decoder draws
//!   each frame's picture at the rectangle, cut to it from its top-left.
//! - An **END** says the stream on `stream` is over: its last picture stays
//!   until something draws over it, and no frame follows on the id until a
//!   START begins another. The rectangle is the stream's.
//!
//! A PNG over a stream's rectangle only ever follows the stream's END, so a
//! client draws every rectangle where it says, in order, and the picture is
//! right. A change of the desktop's size, which is sent first as an update of
//! its own, ends every stream with no END, and the next update sends the
//! picture again whole. Every body is 1 to [`crate::MAX_BODY`] bytes.

use thiserror::Error;

use crate::vp9::ENCODING_VP9_QUALITY_BASE;

/// A PNG of the rectangle's pixels.
pub const KIND_PNG: u8 = 0;
/// A region's first frame, which begins its stream.
pub const KIND_START: u8 = 1;
/// A region's next frame.
pub const KIND_FRAME: u8 = 2;
/// A region's stream is over.
pub const KIND_END: u8 = 3;

/// The dial the client's list asks the tiles picture's streams held at — the
/// VP9 encoding's `WLQ` and the value ([`ENCODING_VP9_QUALITY_BASE`]) beside
/// [`crate::ENCODING_TILES`] — or `None` where the list does not name the
/// encoding. The server has no quality of its own, so a list that names the
/// encoding without one is fatal, and so is one that names the VP9 encoding
/// beside it: they are two pictures, of which a client wants one.
pub fn listed(encodings: &[i32]) -> Result<Option<u8>, TilesListError> {
    if !encodings.contains(&crate::ENCODING_TILES) {
        return Ok(None);
    }
    if encodings.contains(&crate::ENCODING_VP9) {
        return Err(TilesListError::Both);
    }
    crate::listed_quality(encodings).map(Some).ok_or(TilesListError::NoQuality)
}

/// The list that asks for the tiles picture with its streams at `quality`. A
/// client puts it ahead of the rest of its list, and lists no VP9 beside it.
pub fn encodings(quality: u8) -> Vec<i32> {
    let quality = quality.clamp(crate::QUALITY_MIN, crate::QUALITY_MAX);
    vec![crate::ENCODING_TILES, ENCODING_VP9_QUALITY_BASE + i32::from(quality)]
}

/// Why a list that names the tiles encoding asks for no picture the server
/// sends. Fatal to the connection, as a malformed message is.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TilesListError {
    #[error("the list names the tiles encoding without a quality 1–100 beside it")]
    NoQuality,
    #[error("the list names both the tiles and the VP9 encoding")]
    Both,
}

/// Why a tiles rectangle's body is not one.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TilesParseError {
    #[error("a tiles rectangle of kind {0}, which is none of PNG, START, FRAME and END")]
    UnknownKind(u8),
    #[error("a tiles rectangle of {0} bytes, where one is 1 to {max} bytes", max = crate::MAX_BODY)]
    Length(usize),
}

/// The head of a tiles rectangle's body: its kind, its stream where it has one,
/// and the length of what follows where anything does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileHeader {
    /// A PNG of `len` bytes follows.
    Png { len: usize },
    /// The first frame of a stream on `stream`, `len` bytes, follows.
    Start { stream: u8, len: usize },
    /// The next frame of the stream on `stream`, `len` bytes, follows.
    Frame { stream: u8, len: usize },
    /// The stream on `stream` is over; nothing follows.
    End { stream: u8 },
}

impl TileHeader {
    /// The bytes of a header that begins with `kind`, the kind included.
    pub fn len_of(kind: u8) -> Result<usize, TilesParseError> {
        match kind {
            KIND_PNG => Ok(5),
            KIND_START | KIND_FRAME => Ok(6),
            KIND_END => Ok(2),
            other => Err(TilesParseError::UnknownKind(other)),
        }
    }

    /// The header's bytes. The length of a body is the server's to keep to
    /// [`crate::MAX_BODY`].
    pub fn to_bytes(self) -> Vec<u8> {
        let length = |len: usize| (len as u32).to_be_bytes();
        match self {
            Self::Png { len } => [&[KIND_PNG][..], &length(len)].concat(),
            Self::Start { stream, len } => [&[KIND_START, stream][..], &length(len)].concat(),
            Self::Frame { stream, len } => [&[KIND_FRAME, stream][..], &length(len)].concat(),
            Self::End { stream } => vec![KIND_END, stream],
        }
    }

    /// The header at the front of `buf`, a rectangle's body, and its length,
    /// or `Ok(None)` while it is not all there. An unknown kind, or a body
    /// of none or past [`crate::MAX_BODY`] bytes, is an error.
    pub fn parse(buf: &[u8]) -> Result<Option<(Self, usize)>, TilesParseError> {
        let Some(&kind) = buf.first() else { return Ok(None) };
        let used = Self::len_of(kind)?;
        if buf.len() < used {
            return Ok(None);
        }
        let len = |at: usize| {
            let len = crate::u32_at(buf, at) as usize;
            if len == 0 || len > crate::MAX_BODY { Err(TilesParseError::Length(len)) } else { Ok(len) }
        };
        let header = match kind {
            KIND_PNG => Self::Png { len: len(1)? },
            KIND_START => Self::Start { stream: buf[1], len: len(2)? },
            KIND_FRAME => Self::Frame { stream: buf[1], len: len(2)? },
            _ => Self::End { stream: buf[1] },
        };
        Ok(Some((header, used)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_names_one_picture_and_its_dial() {
        let quality = ENCODING_VP9_QUALITY_BASE + 70;
        assert_eq!(listed(&[crate::ENCODING_TILES, quality, 16]), Ok(Some(70)));
        assert_eq!(listed(&[crate::ENCODING_TILES, 16]), Err(TilesListError::NoQuality));
        assert_eq!(listed(&[crate::ENCODING_TILES, ENCODING_VP9_QUALITY_BASE + 101]), Err(TilesListError::NoQuality));
        assert_eq!(listed(&[crate::ENCODING_TILES, crate::ENCODING_VP9, quality]), Err(TilesListError::Both));
        assert_eq!(listed(&[16, quality]), Ok(None));
        assert_eq!(encodings(80), [crate::ENCODING_TILES, 0x574c_5150]);
        assert_eq!(listed(&encodings(80)), Ok(Some(80)));
    }

    /// Each kind against the documented layout, and read back.
    #[test]
    fn each_header_has_the_documented_layout() {
        for (header, bytes) in [
            (TileHeader::Png { len: 0x0102 }, vec![0, 0, 0, 1, 2]),
            (TileHeader::Start { stream: 3, len: 7 }, vec![1, 3, 0, 0, 0, 7]),
            (TileHeader::Frame { stream: 3, len: 9 }, vec![2, 3, 0, 0, 0, 9]),
            (TileHeader::End { stream: 3 }, vec![3, 3]),
        ] {
            assert_eq!(header.to_bytes(), bytes);
            assert_eq!(TileHeader::len_of(bytes[0]), Ok(bytes.len()));
            assert_eq!(TileHeader::parse(&[&bytes[..], &[0xFF]].concat()), Ok(Some((header, bytes.len()))));
            assert_eq!(TileHeader::parse(&bytes[..bytes.len() - 1]), Ok(None), "a partial header asks for more");
        }
    }

    #[test]
    fn a_bad_kind_or_length_is_fatal() {
        assert_eq!(TileHeader::parse(&[4]), Err(TilesParseError::UnknownKind(4)));
        assert_eq!(TileHeader::parse(&[0, 0, 0, 0, 0]), Err(TilesParseError::Length(0)));
        let over = (crate::MAX_BODY as u32 + 1).to_be_bytes();
        assert_eq!(TileHeader::parse(&[2, 1, over[0], over[1], over[2], over[3]]), Err(TilesParseError::Length(crate::MAX_BODY + 1)));
    }
}
