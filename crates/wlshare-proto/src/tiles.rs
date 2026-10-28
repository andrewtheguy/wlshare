//! The tiles encoding: the desktop as exact PNG tiles where it is still, and a
//! video stream of its own for each region that moves, for a client on a link
//! with room to spare — or, on a list that lets the dial walk, every tile a
//! RemoteFX Progressive tile, still or moving, and no stream, for a client
//! that wants the walk more than a picture that ends exact.
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
//! 4 COPY    u16 x | u16 y        the picture at (x, y) the rectangle's size
//! 5 CACHED  u16 slot             the tile kept in slot N, drawn at the rectangle
//! 6 STORE   u16 slot             what the client holds at the rectangle, kept in slot N
//! 7 PROGRESSIVE u32 length | u8[length]  a RemoteFX Progressive tile block of the rectangle's tile
//! ```
//!
//! - A **PNG** is the desktop's pixels in the rectangle exactly, 8-bit RGB or
//!   a palette of it, and the picture there until a later rectangle draws
//!   over it. It depends on nothing before it. It is every still of a list
//!   that holds the dial, and nothing else's.
//! - A **PROGRESSIVE** is the rectangle's tile — one [`TILE`] square on the
//!   tile grid from the desktop's top-left, cut by the desktop's edge — as a
//!   RemoteFX Progressive tile block ([`crate::progressive`]): a first pass at
//!   a stage, which replaces what the client held of the tile or adds to it as
//!   a difference, or an upgrade, the bits the stage before held back. The
//!   client decodes every one in order and holds each tile's coefficients from
//!   one to the next; what it draws is the tile as those coefficients show
//!   it. It is every rectangle of a list whose dial walks but a COPY, a CACHED
//!   and a STORE: no PNG or stream. A first pass below the full stage is
//!   owed its upgrades once the link has room, and the full stage is the
//!   settled picture, lossy still: nothing is owed exact.
//! - A **START** begins a stream on `stream`, over its rectangle: an H.264
//!   keyframe, one access unit in Annex B with its parameter sets, 8-bit
//!   4:2:0, BT.601 at studio swing, of the rectangle's size rounded up to even
//!   sides. Each **FRAME** on that id is the next frame of that stream, over
//!   the same rectangle, coded against the frames before it. A decoder draws
//!   each frame's picture at the rectangle, cut to it from its top-left.
//! - An **END** says the stream on `stream` is over: its last picture stays
//!   until something draws over it, and no frame follows on the id until a
//!   START begins another. The rectangle is the stream's.
//! - A **COPY** draws at the rectangle what the client holds, as the
//!   rectangles before it left it, at the same size from `(x, y)`, which is
//!   inside the desktop: a scroll, or a window moved, as RFB's CopyRect draws
//!   one. The two may overlap, and the picture is then as if the source were
//!   copied out before any of it was drawn. Whatever the source held — exact,
//!   a progressive tile's, a stream's picture — the rectangle holds the same, and is owed
//!   what the source was owed. It has no body past its header.
//! - A **STORE** keeps what the client holds at the rectangle, as the
//!   rectangles before it left it, in slot `slot` of its cache of tiles,
//!   over whatever the slot held: the rectangle is one tile, [`TILE`] square
//!   and on the tile grid from the desktop's top-left, and the slot one of
//!   [`CACHE_SLOTS`]. A **CACHED** draws the tile slot `slot` keeps at the
//!   rectangle, the same size, and the rectangle is owed what that tile was
//!   owed when it was stored: a tile stored exact, or at the full stage, is
//!   settled, and one stored below it is owed the full stage, as a copy's is.
//!   A slot nothing was stored in is fatal to draw from. Neither has a body
//!   past its header. What is stored, and when, is the server's to decide:
//!   the cache is the server's record of what the client was shown, kept so
//!   that a tile shown again — a window switched back to, a menu opened
//!   again, a page scrolled back — is a few bytes and not a still.
//!
//! A PNG over a stream's rectangle only ever follows the stream's END, so a
//! client draws every rectangle where it says, in order, and the picture is
//! right. A change of the desktop's size, which is sent first as an update of
//! its own, ends every stream with no END, and the next update sends the
//! picture again whole. Every body a length names is 1 to [`crate::MAX_BODY`]
//! bytes.
//!
//! The cache outlives a picture that starts over — a new size, a
//! non-incremental request, a list that names another dial or walk — and
//! the server draws from what it stored before, as long as the list names
//! the encoding. A client whose cache is gone, or that missed a rectangle,
//! says so with a [`cache_empty`], and the server stores from nothing again,
//! its next update the picture whole; a list without the encoding forgets
//! the cache too.
//!
//! Client → server `CacheEmpty`, the one message of the encoding, one byte:
//!
//! | Offset | Type | Field |
//! |---|---|---|
//! | 0 | U8 | `0xE6` |

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
/// What the client holds elsewhere, drawn at the rectangle.
pub const KIND_COPY: u8 = 4;
/// The tile a cache slot keeps, drawn at the rectangle.
pub const KIND_CACHED: u8 = 5;
/// What the client holds at the rectangle, kept in a cache slot.
pub const KIND_STORE: u8 = 6;
/// A RemoteFX Progressive tile block of the rectangle's tile, on a list
/// whose dial walks.
pub const KIND_PROGRESSIVE: u8 = 7;

/// A tile's side: the desktop is in tiles this square from its top-left, and
/// a STORE or a CACHED rectangle is one of them.
pub const TILE: u16 = 64;
/// The tiles a client's cache keeps, numbered from 0: 64 MiB of pixels, as
/// many as serve nearly every tile a long session shows again.
pub const CACHE_SLOTS: u16 = 4096;

/// The type of [`cache_empty`]; outside every registered type.
pub const MSG_CACHE_EMPTY: u8 = 0xE6;

/// The bytes of a `CacheEmpty`, its type included.
pub const CACHE_EMPTY_LEN: usize = 1;

/// Client → server `CacheEmpty`: the client's cache of tiles holds nothing,
/// or nothing the server can count on, so no slot is drawn from until it is
/// stored again, and the server's next update is the picture whole, as a
/// non-incremental request's is: a client that lost its cache holds nothing
/// else to draw damage on. Sent where the client's picture starts over with
/// nothing of what it held, ahead of the non-incremental request that asks
/// for it, so no update between the two is damage, whichever read of the
/// server's the request comes in.
pub fn cache_empty() -> [u8; CACHE_EMPTY_LEN] {
    [MSG_CACHE_EMPTY]
}

/// The length of the `CacheEmpty` at the front of `buf`, whose first byte is
/// [`MSG_CACHE_EMPTY`].
pub fn parse_cache_empty(buf: &[u8]) -> usize {
    debug_assert_eq!(buf.first(), Some(&MSG_CACHE_EMPTY));
    CACHE_EMPTY_LEN
}

/// What a client's list asks the tiles picture to be: the dial, the VP9
/// encoding's `WLQ` and the value ([`ENCODING_VP9_QUALITY_BASE`]) beside
/// [`crate::ENCODING_TILES`], and whether it walks — progressive, every tile
/// a RemoteFX Progressive tile, the ceiling its first passes are coded
/// toward while the link keeps up, walked down while it does not, nothing
/// owed exact — or is held there, `WLSD` ([`ENCODING_VP9_HELD`]) beside it,
/// every still exact and the streams at the dial whatever the link does, as
/// the VP9 encoding reads the same two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TilesPicture {
    pub quality: u8,
    pub progressive: bool,
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
        Ok(Some(Self { quality, progressive: !has(ENCODING_VP9_HELD) }))
    }

    /// The list that asks for this picture: the encoding, the quality always,
    /// and the held dial where it is held. A client puts it ahead of the rest
    /// of its list, and lists no VP9 beside it.
    pub fn encodings(self) -> Vec<i32> {
        let quality = self.quality.clamp(crate::QUALITY_MIN, crate::QUALITY_MAX);
        let mut listed = vec![crate::ENCODING_TILES, ENCODING_VP9_QUALITY_BASE + i32::from(quality)];
        if !self.progressive {
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
    #[error("a tiles rectangle of kind {0}, which is none of PNG, START, FRAME, END, COPY, CACHED, STORE and PROGRESSIVE")]
    UnknownKind(u8),
    #[error("a tiles rectangle of cache slot {0}, where the cache has {CACHE_SLOTS}")]
    Slot(u16),
    #[error("a tiles rectangle of {0} bytes, where one is 1 to {max} bytes", max = crate::MAX_BODY)]
    Length(usize),
}

/// The head of a tiles rectangle's body: its kind, its stream where it has one,
/// the length of what follows where anything does, and a copy's source.
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
    /// The picture at `(x, y)`, the rectangle's size, drawn at the rectangle;
    /// nothing follows.
    Copy { x: u16, y: u16 },
    /// The tile the client keeps in `slot`, drawn at the rectangle; nothing
    /// follows.
    Cached { slot: u16 },
    /// What the client holds at the rectangle, kept in `slot`; nothing
    /// follows.
    Store { slot: u16 },
    /// A progressive tile block of `len` bytes follows.
    Progressive { len: usize },
}

impl TileHeader {
    /// The bytes of a header that begins with `kind`, the kind included.
    pub fn len_of(kind: u8) -> Result<usize, TilesParseError> {
        match kind {
            KIND_PNG | KIND_COPY | KIND_PROGRESSIVE => Ok(5),
            KIND_START | KIND_FRAME => Ok(6),
            KIND_END => Ok(2),
            KIND_CACHED | KIND_STORE => Ok(3),
            other => Err(TilesParseError::UnknownKind(other)),
        }
    }

    /// The same header naming a body of `len` bytes; an END, a COPY, a
    /// CACHED or a STORE names none. What
    /// a server that writes the body before it knows its length fills its
    /// header in again with, [`Self::to_bytes`] being as long either way.
    pub fn with_len(self, len: usize) -> Self {
        match self {
            Self::Png { .. } => Self::Png { len },
            Self::Start { stream, .. } => Self::Start { stream, len },
            Self::Frame { stream, .. } => Self::Frame { stream, len },
            Self::End { stream } => Self::End { stream },
            Self::Copy { x, y } => Self::Copy { x, y },
            Self::Cached { slot } => Self::Cached { slot },
            Self::Store { slot } => Self::Store { slot },
            Self::Progressive { .. } => Self::Progressive { len },
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
            Self::Copy { x, y } => [&[KIND_COPY][..], &x.to_be_bytes(), &y.to_be_bytes()].concat(),
            Self::Cached { slot } => [&[KIND_CACHED][..], &slot.to_be_bytes()].concat(),
            Self::Store { slot } => [&[KIND_STORE][..], &slot.to_be_bytes()].concat(),
            Self::Progressive { len } => [&[KIND_PROGRESSIVE][..], &length(len)].concat(),
        }
    }

    /// The header at the front of `buf`, a rectangle's body, and its length,
    /// or `Ok(None)` while it is not all there. An unknown kind, a body
    /// of none or past [`crate::MAX_BODY`] bytes, or a cache slot past
    /// [`CACHE_SLOTS`], is an error.
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
            KIND_PROGRESSIVE => Self::Progressive { len: len(1)? },
            KIND_COPY => Self::Copy { x: u16::from_be_bytes([buf[1], buf[2]]), y: u16::from_be_bytes([buf[3], buf[4]]) },
            KIND_CACHED | KIND_STORE => {
                let slot = u16::from_be_bytes([buf[1], buf[2]]);
                if slot >= CACHE_SLOTS {
                    return Err(TilesParseError::Slot(slot));
                }
                if kind == KIND_CACHED { Self::Cached { slot } } else { Self::Store { slot } }
            }
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
        assert_eq!(listed(&[crate::ENCODING_TILES, quality, 16]), Ok(Some(TilesPicture { quality: 70, progressive: true })));
        assert_eq!(listed(&[crate::ENCODING_TILES, quality, ENCODING_VP9_HELD]), Ok(Some(TilesPicture { quality: 70, progressive: false })));
        assert_eq!(listed(&[crate::ENCODING_TILES, 16]), Err(TilesListError::NoQuality));
        assert_eq!(listed(&[crate::ENCODING_TILES, ENCODING_VP9_QUALITY_BASE + 101]), Err(TilesListError::NoQuality));
        assert_eq!(listed(&[crate::ENCODING_TILES, crate::ENCODING_VP9, quality]), Err(TilesListError::Both));
        assert_eq!(listed(&[16, quality]), Ok(None));
        let progressive = TilesPicture { quality: 80, progressive: true };
        let held = TilesPicture { quality: 80, progressive: false };
        assert_eq!(progressive.encodings(), [crate::ENCODING_TILES, 0x574c_5150]);
        assert_eq!(held.encodings(), [crate::ENCODING_TILES, 0x574c_5150, ENCODING_VP9_HELD]);
        assert_eq!(listed(&progressive.encodings()), Ok(Some(progressive)));
        assert_eq!(listed(&held.encodings()), Ok(Some(held)));
    }

    #[test]
    fn a_cache_empty_is_its_type_alone() {
        assert_eq!(cache_empty(), [0xE6]);
        assert_eq!(parse_cache_empty(&[0xE6, 0xFF]), 1);
    }

    /// Each kind against the documented layout, and read back.
    #[test]
    fn each_header_has_the_documented_layout() {
        for (header, bytes) in [
            (TileHeader::Png { len: 0x0102 }, vec![0, 0, 0, 1, 2]),
            (TileHeader::Start { stream: 3, len: 7 }, vec![1, 3, 0, 0, 0, 7]),
            (TileHeader::Frame { stream: 3, len: 9 }, vec![2, 3, 0, 0, 0, 9]),
            (TileHeader::End { stream: 3 }, vec![3, 3]),
            (TileHeader::Copy { x: 0x0102, y: 0x0304 }, vec![4, 1, 2, 3, 4]),
            (TileHeader::Cached { slot: 0x0102 }, vec![5, 1, 2]),
            (TileHeader::Store { slot: 0x0FFF }, vec![6, 0x0F, 0xFF]),
            (TileHeader::Progressive { len: 0x0102 }, vec![7, 0, 0, 1, 2]),
        ] {
            assert_eq!(header.to_bytes(), bytes);
            assert_eq!(TileHeader::len_of(bytes[0]), Ok(bytes.len()));
            assert_eq!(TileHeader::parse(&[&bytes[..], &[0xFF]].concat()), Ok(Some((header, bytes.len()))));
            assert_eq!(TileHeader::parse(&bytes[..bytes.len() - 1]), Ok(None), "a partial header asks for more");
        }
    }

    #[test]
    fn a_length_filled_in_keeps_the_header_its_size() {
        for header in [TileHeader::Png { len: 0 }, TileHeader::Start { stream: 2, len: 0 }, TileHeader::Frame { stream: 2, len: 0 }, TileHeader::End { stream: 2 }, TileHeader::Copy { x: 1, y: 2 }, TileHeader::Cached { slot: 3 }, TileHeader::Store { slot: 4 }, TileHeader::Progressive { len: 0 }] {
            let filled = header.with_len(9);
            assert_eq!(filled.to_bytes().len(), header.to_bytes().len());
            assert_eq!(filled.to_bytes()[0], header.to_bytes()[0], "the kind is kept");
        }
        assert_eq!(TileHeader::Frame { stream: 2, len: 0 }.with_len(9), TileHeader::Frame { stream: 2, len: 9 });
        assert_eq!(TileHeader::End { stream: 2 }.with_len(9), TileHeader::End { stream: 2 });
        assert_eq!(TileHeader::Copy { x: 1, y: 2 }.with_len(9), TileHeader::Copy { x: 1, y: 2 }, "a copy's source is no length");
        assert_eq!(TileHeader::Store { slot: 4 }.with_len(9), TileHeader::Store { slot: 4 }, "a slot is no length");
    }

    #[test]
    fn a_bad_kind_length_or_slot_is_fatal() {
        assert_eq!(TileHeader::parse(&[8]), Err(TilesParseError::UnknownKind(8)));
        assert_eq!(TileHeader::parse(&[7, 0, 0, 0, 0]), Err(TilesParseError::Length(0)));
        assert_eq!(TileHeader::parse(&[5, 0x10, 0]), Err(TilesParseError::Slot(4096)));
        assert_eq!(TileHeader::parse(&[6, 0xFF, 0xFF]), Err(TilesParseError::Slot(65535)));
        assert_eq!(TileHeader::parse(&[5, 0x0F, 0xFF, 9]), Ok(Some((TileHeader::Cached { slot: 4095 }, 3))), "the last slot");
        assert_eq!(TileHeader::parse(&[0, 0, 0, 0, 0]), Err(TilesParseError::Length(0)));

        let over = (crate::MAX_BODY as u32 + 1).to_be_bytes();
        assert_eq!(TileHeader::parse(&[2, 1, over[0], over[1], over[2], over[3]]), Err(TilesParseError::Length(crate::MAX_BODY + 1)));
    }
}
