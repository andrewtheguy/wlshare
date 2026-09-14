//! The camera extension: how a client lends the desktop the camera in front of
//! its user.
//!
//! Standard RFB carries nothing from the client but input and a clipboard, and
//! no registered extension carries video in that direction. This private one is
//! the third of wlshare's, in the same shape as the density and outputs
//! extensions ([`crate::density`], [`crate::outputs`]): one pseudo-encoding and
//! one message type, used in both directions.
//!
//! - The client lists [`crate::ENCODING_CAMERA`] in `SetEncodings`. A server that
//!   does not know it ignores it, as RFB requires.
//! - The server answers **every** such `SetEncodings` with a
//!   [`camera_available`] — that answer is the only way support is ever
//!   announced.
//! - The client plugs a camera ([`ClientCamera::Plug`]), naming the H.264 it will
//!   send: its geometry and frame rate. The server makes a camera of it that the
//!   desktop's applications can open, and unplugs it again on
//!   [`ClientCamera::Unplug`] or when the client leaves.
//! - Whether anything is sent is the desktop's decision, not the client's: the
//!   server sends [`camera_start`] when an application opens the camera and
//!   [`camera_stop`] when the last one closes it. Between the two the client
//!   sends samples ([`ClientCamera::Sample`]), one H.264 access unit each, and
//!   outside them it sends nothing.
//! - A server that lost samples, or could not decode one, sends
//!   [`camera_keyframe`]: H.264 cannot resume mid-GOP, so the next sample worth
//!   sending is a keyframe, and the stream opened by a start begins at one too.
//!
//! Samples are Annex B, parameter sets inline on a keyframe, so the wire needs
//! no side channel for them. Nothing here looks inside one: that is the
//! decoder's business.

use thiserror::Error;

/// The message type, used by both directions; outside every registered type.
pub const MSG_CAMERA: u8 = 0xE2;

/// Client operation: a camera producing H.264 in the format that follows.
pub const CLIENT_CAMERA_PLUG: u8 = 0;
/// Client operation: the camera is gone.
pub const CLIENT_CAMERA_UNPLUG: u8 = 1;
/// Client operation: one encoded access unit follows, after its length.
pub const CLIENT_CAMERA_SAMPLE: u8 = 2;

/// Server operation: this server takes a camera. The answer to `SetEncodings`.
pub const SERVER_CAMERA_AVAILABLE: u8 = 0;
/// Server operation: an application opened the camera; samples are wanted.
pub const SERVER_CAMERA_START: u8 = 1;
/// Server operation: the last application closed it; send no more.
pub const SERVER_CAMERA_STOP: u8 = 2;
/// Server operation: the next sample must be a keyframe.
pub const SERVER_CAMERA_KEYFRAME: u8 = 3;

/// A sample's flags, bit 0: the access unit is a keyframe.
pub const SAMPLE_KEYFRAME: u8 = 1;

/// The bytes of a plug, and of a start, type included.
pub const CAMERA_FORMAT_LEN: usize = 16;
/// The bytes of an unplug, and of every server message but a start.
pub const CAMERA_BARE_LEN: usize = 4;
/// The bytes of a sample before its access unit.
pub const CAMERA_SAMPLE_HEADER_LEN: usize = 8;

/// The longest access unit a client may send.
///
/// The field is a `u32` and a server buffers the whole unit before it can hand
/// it on, so it is bounded where cut text is. An intra frame of 4K Constrained
/// Baseline at a high bitrate is well under a megabyte; four is a client that
/// means something else by the field.
pub const MAX_SAMPLE: usize = 4 * 1024 * 1024;

/// The H.264 a camera produces: its geometry, and a frame rate as a ratio,
/// because a browser reports rates like 29.97.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraFormat {
    pub width: u16,
    pub height: u16,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
}

/// A camera submessage from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientCamera {
    Plug(CameraFormat),
    Unplug,
    /// One Annex B access unit.
    Sample { keyframe: bool, data: Vec<u8> },
}

/// Why a client's camera message could not be one.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CameraParseError {
    #[error("camera operation {0} is not plug, unplug or sample")]
    UnknownOperation(u8),
    #[error("a camera of {width}x{height}")]
    EmptyGeometry { width: u16, height: u16 },
    #[error("a camera frame rate of {numerator}/{denominator}")]
    BadRate { numerator: u32, denominator: u32 },
    #[error("a camera sample of {0} bytes is over the {MAX_SAMPLE}-byte ceiling")]
    SampleTooLong(usize),
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_be_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Parse the camera message at the front of `buf`, whose first byte is
/// [`MSG_CAMERA`]. `Ok(None)` means more bytes are needed.
///
/// A plug with no pixels or no rate is fatal rather than refused: it is a
/// client that does not mean what the extension means by the fields, and
/// nothing it sends after it could be read the way it intends.
pub fn parse_client(buf: &[u8]) -> Result<Option<(ClientCamera, usize)>, CameraParseError> {
    debug_assert_eq!(buf.first(), Some(&MSG_CAMERA));
    if buf.len() < CAMERA_BARE_LEN {
        return Ok(None);
    }
    match buf[1] {
        CLIENT_CAMERA_PLUG => {
            if buf.len() < CAMERA_FORMAT_LEN {
                return Ok(None);
            }
            let format = CameraFormat {
                width: u16_at(buf, 4),
                height: u16_at(buf, 6),
                fps_numerator: u32_at(buf, 8),
                fps_denominator: u32_at(buf, 12),
            };
            if format.width == 0 || format.height == 0 {
                return Err(CameraParseError::EmptyGeometry { width: format.width, height: format.height });
            }
            if format.fps_numerator == 0 || format.fps_denominator == 0 {
                return Err(CameraParseError::BadRate { numerator: format.fps_numerator, denominator: format.fps_denominator });
            }
            Ok(Some((ClientCamera::Plug(format), CAMERA_FORMAT_LEN)))
        }
        CLIENT_CAMERA_UNPLUG => Ok(Some((ClientCamera::Unplug, CAMERA_BARE_LEN))),
        CLIENT_CAMERA_SAMPLE => {
            if buf.len() < CAMERA_SAMPLE_HEADER_LEN {
                return Ok(None);
            }
            let len = u32_at(buf, 4) as usize;
            if len > MAX_SAMPLE {
                return Err(CameraParseError::SampleTooLong(len));
            }
            let end = CAMERA_SAMPLE_HEADER_LEN + len;
            if buf.len() < end {
                return Ok(None);
            }
            let keyframe = buf[2] & SAMPLE_KEYFRAME != 0;
            Ok(Some((ClientCamera::Sample { keyframe, data: buf[CAMERA_SAMPLE_HEADER_LEN..end].to_vec() }, end)))
        }
        other => Err(CameraParseError::UnknownOperation(other)),
    }
}

fn bare(operation: u8) -> [u8; CAMERA_BARE_LEN] {
    [MSG_CAMERA, operation, 0, 0]
}

/// Server → client: this server takes a camera.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE2` |
/// | 1 | U8 | operation, 0 |
/// | 2 | U16 | padding |
pub fn camera_available() -> [u8; CAMERA_BARE_LEN] {
    bare(SERVER_CAMERA_AVAILABLE)
}

/// Server → client: an application opened the camera, which is being fed in
/// `format` — the format the client plugged, since it is the only one offered.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE2` |
/// | 1 | U8 | operation, 1 |
/// | 2 | U16 | padding |
/// | 4 | U16 | width |
/// | 6 | U16 | height |
/// | 8 | U32 | frame rate numerator |
/// | 12 | U32 | frame rate denominator |
///
/// A plug is the same sixteen bytes with operation 0.
pub fn camera_start(format: CameraFormat) -> [u8; CAMERA_FORMAT_LEN] {
    let mut msg = [0u8; CAMERA_FORMAT_LEN];
    msg[..CAMERA_BARE_LEN].copy_from_slice(&bare(SERVER_CAMERA_START));
    msg[4..6].copy_from_slice(&format.width.to_be_bytes());
    msg[6..8].copy_from_slice(&format.height.to_be_bytes());
    msg[8..12].copy_from_slice(&format.fps_numerator.to_be_bytes());
    msg[12..16].copy_from_slice(&format.fps_denominator.to_be_bytes());
    msg
}

/// Server → client: the camera is no longer open; send no more samples.
pub fn camera_stop() -> [u8; CAMERA_BARE_LEN] {
    bare(SERVER_CAMERA_STOP)
}

/// Server → client: samples were lost or could not be decoded; the next one
/// must be a keyframe.
pub fn camera_keyframe() -> [u8; CAMERA_BARE_LEN] {
    bare(SERVER_CAMERA_KEYFRAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VGA: CameraFormat = CameraFormat { width: 640, height: 480, fps_numerator: 30_000, fps_denominator: 1_001 };

    /// A server message as the documented layouts describe it, decoded without
    /// the builders.
    #[derive(Debug, PartialEq, Eq)]
    enum Decoded {
        Available,
        Start(CameraFormat),
        Stop,
        Keyframe,
    }

    fn decode(bytes: &[u8]) -> Decoded {
        assert_eq!(bytes[0], 0xE2);
        assert_eq!(&bytes[2..4], &[0, 0], "the padding is zero");
        let decoded = match bytes[1] {
            0 => Decoded::Available,
            1 => Decoded::Start(CameraFormat {
                width: u16::from_be_bytes([bytes[4], bytes[5]]),
                height: u16::from_be_bytes([bytes[6], bytes[7]]),
                fps_numerator: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
                fps_denominator: u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            }),
            2 => Decoded::Stop,
            3 => Decoded::Keyframe,
            other => panic!("operation {other} is not a server message"),
        };
        let len = if matches!(decoded, Decoded::Start(_)) { 16 } else { 4 };
        assert_eq!(bytes.len(), len, "the message is exactly its layout");
        decoded
    }

    #[test]
    fn server_messages_decode_to_what_they_were_built_from() {
        assert_eq!(decode(&camera_available()), Decoded::Available);
        assert_eq!(decode(&camera_start(VGA)), Decoded::Start(VGA));
        assert_eq!(decode(&camera_stop()), Decoded::Stop);
        assert_eq!(decode(&camera_keyframe()), Decoded::Keyframe);
    }

    #[test]
    fn a_start_has_the_documented_layout() {
        assert_eq!(
            camera_start(VGA),
            [
                0xE2, 1, 0, 0, //
                0x02, 0x80, // 640
                0x01, 0xE0, // 480
                0x00, 0x00, 0x75, 0x30, // 30000
                0x00, 0x00, 0x03, 0xE9, // 1001
            ]
        );
    }

    #[test]
    fn the_client_operations_parse() {
        let plug = [0xE2, 0, 0, 0, 0x02, 0x80, 0x01, 0xE0, 0, 0, 0x75, 0x30, 0, 0, 0x03, 0xE9, 0xFF];
        assert_eq!(parse_client(&plug), Ok(Some((ClientCamera::Plug(VGA), 16))));
        assert_eq!(parse_client(&[0xE2, 1, 0, 0, 0xFF]), Ok(Some((ClientCamera::Unplug, 4))));
        let sample = [0xE2, 2, SAMPLE_KEYFRAME, 0, 0, 0, 0, 3, 0, 0, 1, 0xFF];
        assert_eq!(parse_client(&sample), Ok(Some((ClientCamera::Sample { keyframe: true, data: vec![0, 0, 1] }, 11))));
        let delta = [0xE2, 2, 0, 0, 0, 0, 0, 1, 9];
        assert_eq!(parse_client(&delta), Ok(Some((ClientCamera::Sample { keyframe: false, data: vec![9] }, 9))));
    }

    #[test]
    fn a_partial_message_asks_for_more() {
        assert_eq!(parse_client(&[0xE2, 0, 0]), Ok(None));
        assert_eq!(parse_client(&[0xE2, 0, 0, 0, 0x02, 0x80, 0x01, 0xE0, 0, 0, 0x75, 0x30, 0, 0, 0x03]), Ok(None));
        assert_eq!(parse_client(&[0xE2, 2, 0, 0, 0, 0, 0]), Ok(None));
        assert_eq!(parse_client(&[0xE2, 2, 0, 0, 0, 0, 0, 3, 0, 0]), Ok(None), "the unit is not all here");
    }

    #[test]
    fn a_bad_operation_format_or_length_is_fatal() {
        assert_eq!(parse_client(&[0xE2, 7, 0, 0]), Err(CameraParseError::UnknownOperation(7)));
        assert_eq!(
            parse_client(&[0xE2, 0, 0, 0, 0, 0, 0x01, 0xE0, 0, 0, 0, 30, 0, 0, 0, 1]),
            Err(CameraParseError::EmptyGeometry { width: 0, height: 480 })
        );
        assert_eq!(
            parse_client(&[0xE2, 0, 0, 0, 0x02, 0x80, 0x01, 0xE0, 0, 0, 0, 30, 0, 0, 0, 0]),
            Err(CameraParseError::BadRate { numerator: 30, denominator: 0 })
        );
        let over = (MAX_SAMPLE as u32 + 1).to_be_bytes();
        assert_eq!(
            parse_client(&[0xE2, 2, 0, 0, over[0], over[1], over[2], over[3]]),
            Err(CameraParseError::SampleTooLong(MAX_SAMPLE + 1)),
            "refused before the unit is waited for"
        );
    }
}
