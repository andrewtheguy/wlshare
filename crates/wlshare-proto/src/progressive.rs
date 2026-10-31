//! The progressive tiles' coding: what a [`crate::tiles::KIND_PROGRESSIVE`]
//! rectangle's body is, and the stream a RemoteFX Progressive decoder reads it
//! from.
//!
//! A body is one tile block of RemoteFX Progressive ([MS-RDPEGFX] 2.2.4.2.1.5):
//! an `RFX_PROGRESSIVE_TILE_FIRST`, which the decoder takes in place of what it
//! held of the tile, or as the difference from it where its flags say so; or an
//! `RFX_PROGRESSIVE_TILE_UPGRADE`, which sends the bits the stage before held
//! back. Its block header is the body's first six bytes, and its tile is the
//! rectangle's: `xIdx` and `yIdx` the rectangle's tile on the grid of
//! [`crate::tiles::TILE`] from the desktop's top-left. Its quantizer indices
//! name [`QUANT`], one table for each component, and its quality byte one of
//! [`STAGES`] by its index there, or `0xFF` for the full stage.
//!
//! The tables are this protocol's and are never sent: a decoder reads a body
//! out of [`stream`], which puts it in a region of its own that carries them,
//! and holds every tile's coefficients from one body to the next. A server
//! sends a tile anything else drew over — a copy, a cache slot's draw — as a
//! first pass that is no difference, so the decoder needs no word of it.

/// The quantization table of every component, five bytes of
/// `TS_RFX_CODEC_QUANT`: a Windows host's, a bit finer in every band above
/// RemoteFX's least quantizer, 6.
pub const QUANT: [u8; 5] = [0x66, 0x66, 0x77, 0x88, 0x98];

/// The progressive stages, coarsest first: each a quality label and the
/// `RFX_COMPONENT_CODEC_QUANT` of Y, Cb and Cr, the bits a first pass at it
/// holds back. A Windows host's stages of 50 and 75, which hold back the bit
/// [`QUANT`] gains over its own, and ahead of them one of this protocol's, 50's
/// holding a bit more back in every band, for a link the walk has slowed; and
/// the full stage last, which holds nothing back. A tile's quality byte names
/// one by its index here, and the full stage `0xFF`.
pub const STAGES: [[u8; 16]; 4] = [
    [25, 0x22, 0x32, 0x44, 0x34, 0x33, 0x22, 0x32, 0x44, 0x34, 0x33, 0x22, 0x32, 0x44, 0x34, 0x23],
    [50, 0x11, 0x21, 0x33, 0x23, 0x22, 0x11, 0x21, 0x33, 0x23, 0x22, 0x11, 0x21, 0x33, 0x23, 0x12],
    [75, 0x10, 0x21, 0x22, 0x12, 0x11, 0x10, 0x21, 0x23, 0x12, 0x11, 0x10, 0x21, 0x23, 0x22, 0x11],
    [100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
];

/// A tile's side, in pixels.
const TILE: u8 = 64;
const WBT_SYNC: u16 = 0xCCC0;
const WBT_FRAME_BEGIN: u16 = 0xCCC1;
const WBT_FRAME_END: u16 = 0xCCC2;
const WBT_CONTEXT: u16 = 0xCCC3;
const WBT_REGION: u16 = 0xCCC4;
/// The first pass of a tile, and an upgrade of one.
pub const WBT_TILE_FIRST: u16 = 0xCCC6;
pub const WBT_TILE_UPGRADE: u16 = 0xCCC7;
/// The context's flag: the decoder diffs sub-bands, as a first pass's
/// difference asks of it.
const CONTEXT_SUBBAND_DIFFING: u8 = 0x01;
/// The region's flag: the reduce-extrapolate wavelet.
const DWT_REDUCE_EXTRAPOLATE: u8 = 0x01;

/// The tile `body` is of, `(xIdx, yIdx)`, where it is a tile block, first or
/// upgrade, whose length is the body's; `None` otherwise.
pub fn tile_of(body: &[u8]) -> Option<(u16, u16)> {
    let kind = u16::from_le_bytes([*body.first()?, *body.get(1)?]);
    let len = u32::from_le_bytes(body.get(2..6)?.try_into().ok()?);
    if !matches!(kind, WBT_TILE_FIRST | WBT_TILE_UPGRADE) || len as usize != body.len() {
        return None;
    }
    let at = |i: usize| Some(u16::from_le_bytes([*body.get(i)?, *body.get(i + 1)?]));
    Some((at(9)?, at(11)?))
}

/// A RemoteFX Progressive stream a decoder reads `bodies` from: a sync, frame
/// `frame`, the context, one region over `rects` (`x`, `y`, `w`, `h` in
/// pixels) carrying [`QUANT`] and [`STAGES`] and the bodies in order, and the
/// frame's end.
pub fn stream(frame: u32, rects: &[(u16, u16, u16, u16)], bodies: &[&[u8]]) -> Vec<u8> {
    let mut region = vec![TILE];
    region.extend_from_slice(&(rects.len() as u16).to_le_bytes());
    region.push(3);
    region.push(STAGES.len() as u8);
    region.push(DWT_REDUCE_EXTRAPOLATE);
    region.extend_from_slice(&(bodies.len() as u16).to_le_bytes());
    region.extend_from_slice(&(bodies.iter().map(|b| b.len()).sum::<usize>() as u32).to_le_bytes());
    for &(x, y, w, h) in rects {
        for v in [x, y, w, h] {
            region.extend_from_slice(&v.to_le_bytes());
        }
    }
    for _ in 0..3 {
        region.extend_from_slice(&QUANT);
    }
    for stage in &STAGES {
        region.extend_from_slice(stage);
    }
    for body in bodies {
        region.extend_from_slice(body);
    }
    let mut begin = frame.to_le_bytes().to_vec();
    begin.extend_from_slice(&1u16.to_le_bytes());
    [
        block(WBT_SYNC, &[0xCA, 0xAC, 0xCC, 0xCA, 0x00, 0x01]),
        block(WBT_FRAME_BEGIN, &begin),
        block(WBT_CONTEXT, &[0, TILE, 0, CONTEXT_SUBBAND_DIFFING]),
        block(WBT_REGION, &region),
        block(WBT_FRAME_END, &[]),
    ]
    .concat()
}

/// A block of `kind`: its type, its length with the six bytes of its header,
/// and `body`.
pub fn block(kind: u16, body: &[u8]) -> Vec<u8> {
    let mut v = kind.to_le_bytes().to_vec();
    v.extend_from_slice(&(body.len() as u32 + 6).to_le_bytes());
    v.extend_from_slice(body);
    v
}

#[cfg(test)]
mod tests;
