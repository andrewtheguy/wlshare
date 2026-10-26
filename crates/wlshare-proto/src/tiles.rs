//! The tiles encoding: the desktop as exact PNG tiles where it is still, and a
//! video stream of its own for each region that moves, for a client on a link
//! with room to spare — or, on a list that lets the dial walk, a lossy WebP
//! where the link has fallen behind, made exact once it has room again.
//!
//! A private encoding, [`crate::ENCODING_TILES`], beside the VP9 encoding
//! ([`crate::vp9`]) rather than in place of it: a client lists one or the
//! other, and beside it the dial and whether it walks ([`TilesPicture`]).
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
//! 4 WEBP    u32 length | u8[length]  a lossy WebP of the rectangle's pixels
//! ```
//!
//! - A **PNG** is the desktop's pixels in the rectangle exactly, 8-bit RGB or
//!   a palette of it, and the picture there until a later rectangle draws
//!   over it. It depends on nothing before it.
//! - A **WEBP** is the same rectangle lossy, a WebP of its pixels at the dial
//!   the walk stands at, and depends on nothing before it either. The server
//!   owes the rectangle exact, as a PNG, once the link has room; it is sent
//!   only on a list whose dial walks.
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

use crate::vp9::{ENCODING_VP9_HELD, ENCODING_VP9_QUALITY_BASE};

/// A PNG of the rectangle's pixels.
pub const KIND_PNG: u8 = 0;
/// A region's first frame, which begins its stream.
pub const KIND_START: u8 = 1;
/// A region's next frame.
pub const KIND_FRAME: u8 = 2;
/// A region's stream is over.
pub const KIND_END: u8 = 3;
/// A lossy WebP of the rectangle's pixels, owed exact later.
pub const KIND_WEBP: u8 = 4;

/// What a client's list asks the tiles picture to be: the dial, the VP9
/// encoding's `WLQ` and the value ([`ENCODING_VP9_QUALITY_BASE`]) beside
/// [`crate::ENCODING_TILES`], and whether it walks — the ceiling the streams
/// and the stills are coded at while the link keeps up, walked down while it
/// does not, with the stills then lossy WebP owed exact — or is held there,
/// `WLSD` ([`ENCODING_VP9_HELD`]) beside it, every still exact and the
/// streams at the dial whatever the link does, as the VP9 encoding reads
/// the same two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TilesPicture {
    pub quality: u8,
    pub adaptive: bool,
}

impl TilesPicture {
    /// The picture `encodings` asks for, or `None` where the list does not
    /// name the encoding. The server has no quality of its own, so a list
    /// that names the encoding without one is fatal, and so is one that
    /// names the VP9 encoding beside it: they are two pictures, of which a
    /// client wants one.
    pub fn listed(encodings: &[i32]) -> Result<Option<Self>, TilesListError> {
        let has = |e: i32| encodings.contains(&e);
        if !has(crate::ENCODING_TILES) {
            return Ok(None);
        }
        if has(crate::ENCODING_VP9) {
            return Err(TilesListError::Both);
        }
        let quality = crate::listed_quality(encodings).ok_or(TilesListError::NoQuality)?;
        Ok(Some(Self { quality, adaptive: !has(ENCODING_VP9_HELD) }))
    }

    /// The list that asks for this picture: the encoding, the quality always,
    /// and the held dial where it is held. A client puts it ahead of the rest
    /// of its list, and lists no VP9 beside it.
    pub fn encodings(self) -> Vec<i32> {
        let quality = self.quality.clamp(crate::QUALITY_MIN, crate::QUALITY_MAX);
        let mut listed = vec![crate::ENCODING_TILES, ENCODING_VP9_QUALITY_BASE + i32::from(quality)];
        if !self.adaptive {
            listed.push(ENCODING_VP9_HELD);
        }
        listed
    }
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
    #[error("a tiles rectangle of kind {0}, which is none of PNG, START, FRAME, END and WEBP")]
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
    /// A lossy WebP of `len` bytes follows.
    Webp { len: usize },
}

impl TileHeader {
    /// The bytes of a header that begins with `kind`, the kind included.
    pub fn len_of(kind: u8) -> Result<usize, TilesParseError> {
        match kind {
            KIND_PNG | KIND_WEBP => Ok(5),
            KIND_START | KIND_FRAME => Ok(6),
            KIND_END => Ok(2),
            other => Err(TilesParseError::UnknownKind(other)),
        }
    }

    /// The same header naming a body of `len` bytes; an END names none. What
    /// a server that writes the body before it knows its length fills its
    /// header in again with, [`Self::to_bytes`] being as long either way.
    pub fn with_len(self, len: usize) -> Self {
        match self {
            Self::Png { .. } => Self::Png { len },
            Self::Start { stream, .. } => Self::Start { stream, len },
            Self::Frame { stream, .. } => Self::Frame { stream, len },
            Self::End { stream } => Self::End { stream },
            Self::Webp { .. } => Self::Webp { len },
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
            Self::Webp { len } => [&[KIND_WEBP][..], &length(len)].concat(),
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
            KIND_WEBP => Self::Webp { len: len(1)? },
            _ => Self::End { stream: buf[1] },
        };
        Ok(Some((header, used)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_names_one_picture_its_dial_and_whether_it_walks() {
        let quality = ENCODING_VP9_QUALITY_BASE + 70;
        let listed = TilesPicture::listed;
        assert_eq!(listed(&[crate::ENCODING_TILES, quality, 16]), Ok(Some(TilesPicture { quality: 70, adaptive: true })));
        assert_eq!(listed(&[crate::ENCODING_TILES, quality, ENCODING_VP9_HELD]), Ok(Some(TilesPicture { quality: 70, adaptive: false })));
        assert_eq!(listed(&[crate::ENCODING_TILES, 16]), Err(TilesListError::NoQuality));
        assert_eq!(listed(&[crate::ENCODING_TILES, ENCODING_VP9_QUALITY_BASE + 101]), Err(TilesListError::NoQuality));
        assert_eq!(listed(&[crate::ENCODING_TILES, crate::ENCODING_VP9, quality]), Err(TilesListError::Both));
        assert_eq!(listed(&[16, quality]), Ok(None));
        let walked = TilesPicture { quality: 80, adaptive: true };
        let held = TilesPicture { quality: 80, adaptive: false };
        assert_eq!(walked.encodings(), [crate::ENCODING_TILES, 0x574c_5150]);
        assert_eq!(held.encodings(), [crate::ENCODING_TILES, 0x574c_5150, ENCODING_VP9_HELD]);
        assert_eq!(listed(&walked.encodings()), Ok(Some(walked)));
        assert_eq!(listed(&held.encodings()), Ok(Some(held)));
    }

    /// Each kind against the documented layout, and read back.
    #[test]
    fn each_header_has_the_documented_layout() {
        for (header, bytes) in [
            (TileHeader::Png { len: 0x0102 }, vec![0, 0, 0, 1, 2]),
            (TileHeader::Start { stream: 3, len: 7 }, vec![1, 3, 0, 0, 0, 7]),
            (TileHeader::Frame { stream: 3, len: 9 }, vec![2, 3, 0, 0, 0, 9]),
            (TileHeader::End { stream: 3 }, vec![3, 3]),
            (TileHeader::Webp { len: 0x0102 }, vec![4, 0, 0, 1, 2]),
        ] {
            assert_eq!(header.to_bytes(), bytes);
            assert_eq!(TileHeader::len_of(bytes[0]), Ok(bytes.len()));
            assert_eq!(TileHeader::parse(&[&bytes[..], &[0xFF]].concat()), Ok(Some((header, bytes.len()))));
            assert_eq!(TileHeader::parse(&bytes[..bytes.len() - 1]), Ok(None), "a partial header asks for more");
        }
    }

    #[test]
    fn a_length_filled_in_keeps_the_header_its_size() {
        for header in [TileHeader::Png { len: 0 }, TileHeader::Start { stream: 2, len: 0 }, TileHeader::Frame { stream: 2, len: 0 }, TileHeader::End { stream: 2 }, TileHeader::Webp { len: 0 }] {
            let filled = header.with_len(9);
            assert_eq!(filled.to_bytes().len(), header.to_bytes().len());
            assert_eq!(filled.to_bytes()[0], header.to_bytes()[0], "the kind is kept");
        }
        assert_eq!(TileHeader::Frame { stream: 2, len: 0 }.with_len(9), TileHeader::Frame { stream: 2, len: 9 });
        assert_eq!(TileHeader::End { stream: 2 }.with_len(9), TileHeader::End { stream: 2 });
    }

    #[test]
    fn a_bad_kind_or_length_is_fatal() {
        assert_eq!(TileHeader::parse(&[5]), Err(TilesParseError::UnknownKind(5)));
        assert_eq!(TileHeader::parse(&[0, 0, 0, 0, 0]), Err(TilesParseError::Length(0)));
        assert_eq!(TileHeader::parse(&[4, 0, 0, 0, 0]), Err(TilesParseError::Length(0)));

        let over = (crate::MAX_BODY as u32 + 1).to_be_bytes();
        assert_eq!(TileHeader::parse(&[2, 1, over[0], over[1], over[2], over[3]]), Err(TilesParseError::Length(crate::MAX_BODY + 1)));
    }
}
