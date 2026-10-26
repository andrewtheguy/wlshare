//! wlshare's private RFB extensions, both ends of them.
//!
//! The daemon speaks RFB 3.8 as RFC 6143 has it, and beside it a handful of
//! extensions of its own whose one client is the zDesk gateway. Every byte of
//! those is built and parsed here: what the daemon sends and the gateway reads,
//! and what the gateway sends and the daemon reads. Both ends depend on this
//! crate, so a layout is written once and the two cannot disagree about it.
//! Standard RFB, and the codecs behind the private encodings, are each end's
//! own.
//!
//! - **The VP9 encoding** ([`ENCODING_VP9`]), the whole framebuffer as one VP9
//!   stream, and what the client lists beside it ([`vp9`]).
//! - **The tiles encoding** ([`ENCODING_TILES`]), the desktop as exact PNG tiles
//!   where it is still and an H.264 stream for each region that moves
//!   ([`tiles`]).
//! - **The density extension** ([`ENCODING_DENSITY`]), the scale the
//!   framebuffer is drawn at ([`density`]).
//! - **The outputs extension** ([`ENCODING_OUTPUTS`]), which outputs the
//!   compositor has and which one is shared ([`outputs`]).
//! - **The audio extension** ([`ENCODING_AUDIO`]), the desktop's sound as FLAC
//!   or Opus ([`audio`]).
//! - **The camera extension** ([`ENCODING_CAMERA`]) and **the microphone
//!   extension** ([`ENCODING_MICROPHONE`]), the client's devices lent to the
//!   desktop ([`camera`], [`microphone`]).
//! - **The scroll message**, a distance where RFB's wheel has notches
//!   ([`scroll`]).
//! - **The beside ClientInit byte** ([`CLIENT_INIT_BESIDE`]), a second
//!   connection shown another output.
//! - **The protocol version** ([`PROTOCOL_VERSION`]), which revision of all of
//!   these both ends speak, exactly ([`version`]).
//!
//! Each builder here has a parser beside it, and each is tested against a
//! decoder written from the documented layout rather than from the other.
//! Integers are big-endian, as everywhere in RFB. A parser takes the bytes from
//! the message's type onward and answers `Ok(None)` while they are not all
//! there yet, consuming nothing.

pub mod audio;
pub mod camera;
pub mod density;
pub mod microphone;
pub mod outputs;
pub mod scroll;
pub mod tiles;
pub mod version;
pub mod vp9;

pub use version::PROTOCOL_VERSION;

/// The VP9 encoding, the ASCII bytes `WLSV`: a client that lists it, with a
/// quality beside it ([`vp9::Vp9Stream`]), is sent every picture as one
/// rectangle over the whole framebuffer, a frame of one VP9 stream ([`vp9`]).
/// It wins over ZRLE wherever it is listed; listed without a quality it is
/// fatal, since the server has none of its own.
pub const ENCODING_VP9: i32 = 0x574c_5356;
/// The tiles encoding, the ASCII bytes `WLST`: a client that lists it, with a
/// quality beside it ([`tiles::listed`]), is sent the desktop as exact PNG
/// tiles where it is still and a video stream for each region that moves
/// ([`tiles`]). It wins over ZRLE wherever it is listed; listed without a
/// quality, or beside [`ENCODING_VP9`], it is fatal.
pub const ENCODING_TILES: i32 = 0x574c_5354;
/// The density extension's pseudo-encoding, the ASCII bytes `WLSH`.
pub const ENCODING_DENSITY: i32 = 0x574c_5348;
/// The outputs extension's pseudo-encoding, the ASCII bytes `WLSO`.
pub const ENCODING_OUTPUTS: i32 = 0x574c_534f;
/// The camera extension's pseudo-encoding, the ASCII bytes `WLSC`.
pub const ENCODING_CAMERA: i32 = 0x574c_5343;
/// The microphone extension's pseudo-encoding, the ASCII bytes `WLSM`.
pub const ENCODING_MICROPHONE: i32 = 0x574c_534d;
/// The audio extension's pseudo-encoding, the ASCII bytes `WLSF`: a client that
/// lists it can take the desktop's sound, as FLAC unless it asks otherwise, and
/// is told so by an empty rectangle of this encoding ([`audio`]).
pub const ENCODING_AUDIO: i32 = 0x574c_5346;
/// Listed beside [`ENCODING_AUDIO`], asks for the sound as Opus in place of
/// FLAC: the ASCII bytes `WLOP` ([`audio::Codec`]).
pub const ENCODING_AUDIO_OPUS: i32 = 0x574c_4f50;

/// Whether `encoding` is one of wlshare's own: every private encoding and
/// pseudo-encoding is `WL` and two bytes more, and a list that names one names
/// the protocol version beside it ([`version::listed`]).
pub const fn private(encoding: i32) -> bool {
    encoding >> 16 == 0x574c
}

/// The lowest quality on the 1–100 dial a VP9 stream and a region's stream are
/// coded at.
pub const QUALITY_MIN: u8 = 1;
/// The highest quality on the dial.
pub const QUALITY_MAX: u8 = 100;

/// The most bytes one VP9 frame, and one tiles rectangle's PNG or region frame,
/// may be. A 4:4:4 keyframe of the largest desktop at the finest quantizer is a
/// few megabytes; a length past this is a server that has lost its framing,
/// and the server sends none.
pub const MAX_BODY: usize = 64 << 20;

/// The ClientInit byte with which a connection asks to be shown another output
/// beside the client on the desktop, which it leaves there. RFB reads the byte
/// as a shared flag, zero or not; wlshare takes the desktop for every other
/// value, and a server that is not wlshare reads this one as shared.
pub const CLIENT_INIT_BESIDE: u8 = 0xB5;

/// A rectangle header: position, size and encoding, as RFC 6143 lays it out.
/// The audio extension's announcement is one with nothing after it.
fn rect_header(x: u16, y: u16, width: u16, height: u16, encoding: i32) -> [u8; 12] {
    let mut h = [0u8; 12];
    h[0..2].copy_from_slice(&x.to_be_bytes());
    h[2..4].copy_from_slice(&y.to_be_bytes());
    h[4..6].copy_from_slice(&width.to_be_bytes());
    h[6..8].copy_from_slice(&height.to_be_bytes());
    h[8..12].copy_from_slice(&encoding.to_be_bytes());
    h
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}

fn i16_at(b: &[u8], i: usize) -> i16 {
    i16::from_be_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// The quality on the dial `encodings` names beside a private encoding: `WLQ`
/// and the value ([`vp9::ENCODING_VP9_QUALITY_BASE`]), the first that is on the
/// dial.
fn listed_quality(encodings: &[i32]) -> Option<u8> {
    encodings
        .iter()
        .filter_map(|&e| u8::try_from(e.checked_sub(vp9::ENCODING_VP9_QUALITY_BASE)?).ok())
        .find(|q| (QUALITY_MIN..=QUALITY_MAX).contains(q))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encodings_are_their_ascii_names() {
        assert_eq!(ENCODING_VP9, i32::from_be_bytes(*b"WLSV"));
        assert_eq!(ENCODING_TILES, i32::from_be_bytes(*b"WLST"));
        assert_eq!(ENCODING_DENSITY, i32::from_be_bytes(*b"WLSH"));
        assert_eq!(ENCODING_OUTPUTS, i32::from_be_bytes(*b"WLSO"));
        assert_eq!(ENCODING_CAMERA, i32::from_be_bytes(*b"WLSC"));
        assert_eq!(ENCODING_MICROPHONE, i32::from_be_bytes(*b"WLSM"));
        assert_eq!(ENCODING_AUDIO, i32::from_be_bytes(*b"WLSF"));
        assert_eq!(ENCODING_AUDIO_OPUS, i32::from_be_bytes(*b"WLOP"));
    }

    #[test]
    fn every_encoding_here_is_private() {
        let quality = vp9::ENCODING_VP9_QUALITY_BASE;
        for e in [
            ENCODING_VP9,
            ENCODING_TILES,
            ENCODING_DENSITY,
            ENCODING_OUTPUTS,
            ENCODING_CAMERA,
            ENCODING_MICROPHONE,
            ENCODING_AUDIO,
            ENCODING_AUDIO_OPUS,
            vp9::ENCODING_VP9_HELD,
            vp9::ENCODING_VP9_SUBSAMPLED,
            quality + 1,
            quality + 100,
            version::encoding(1),
        ] {
            assert!(private(e), "{e:#x}");
        }
        // RFB's own: ZRLE, Cursor, and the QEMU Audio pseudo-encoding.
        for e in [16, -239, -259] {
            assert!(!private(e), "{e}");
        }
    }

    #[test]
    fn a_quality_is_the_first_one_on_the_dial() {
        let base = vp9::ENCODING_VP9_QUALITY_BASE;
        assert_eq!(listed_quality(&[ENCODING_VP9, base + 60]), Some(60));
        assert_eq!(listed_quality(&[base, base + 101, base + 1]), Some(1), "0 and 101 are not on it");
        assert_eq!(listed_quality(&[ENCODING_VP9, base - 1]), None);
    }
}
