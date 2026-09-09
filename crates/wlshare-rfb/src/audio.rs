//! The QEMU Audio extension: how the desktop's sound reaches a client over the
//! RFB connection it already has.
//!
//! This is the one audio extension `rfbproto` registers — pseudo-encoding
//! [`crate::ENCODING_QEMU_AUDIO`] (-259) and message type 255, submessage 1, in
//! both directions — spoken by QEMU as a server and gtk-vnc as a client, and by
//! the remotex gateway. Nothing about it is private, which is why it was taken
//! over a second `WLSH`-style message: a client that already speaks it hears
//! wlshare with nothing new to learn.
//!
//! - The client lists the pseudo-encoding in `SetEncodings`. The server
//!   announces support with an empty pseudo-rectangle of that encoding
//!   ([`audio_rect`]) in a `FramebufferUpdate` — the only way support is ever
//!   announced, as with ExtendedDesktopSize.
//! - The client then sets the sample format it wants
//!   ([`ClientAudio::SetFormat`]) and enables audio ([`ClientAudio::Enable`]);
//!   it may disable it again ([`ClientAudio::Disable`]).
//! - The server sends [`audio_begin`] when a stream starts, [`audio_data`]
//!   messages carrying raw samples while it runs, and [`audio_end`] when it
//!   stops.
//!
//! `rfbproto` says nothing about the byte order of a sample wider than eight
//! bits. QEMU writes host-native samples and gtk-vnc reads little-endian ones;
//! host-native is safe because every host this runs on is little-endian, so
//! **samples are little-endian** here. The sample format, channel count and frequency are
//! the client's to choose; the server converts whatever the desktop plays into
//! them.

use thiserror::Error;

/// The message type both directions use, shared with every other QEMU
/// extension; [`SUBMESSAGE_AUDIO`] names this one.
pub const MSG_QEMU: u8 = 255;
/// The submessage type under [`MSG_QEMU`] that is audio.
pub const SUBMESSAGE_AUDIO: u8 = 1;

/// Client operation: start sending audio.
pub const CLIENT_AUDIO_ENABLE: u16 = 0;
/// Client operation: stop sending audio.
pub const CLIENT_AUDIO_DISABLE: u16 = 1;
/// Client operation: the sample format the client wants, which follows.
pub const CLIENT_AUDIO_SET_FORMAT: u16 = 2;

/// Server operation: the stream stopped.
pub const SERVER_AUDIO_END: u16 = 0;
/// Server operation: a stream started.
pub const SERVER_AUDIO_BEGIN: u16 = 1;
/// Server operation: samples follow.
pub const SERVER_AUDIO_DATA: u16 = 2;

/// The bytes of an enable or a disable, type and submessage included.
pub const CLIENT_AUDIO_SWITCH_LEN: usize = 4;
/// The bytes of a set-format, type and submessage included.
pub const CLIENT_AUDIO_FORMAT_LEN: usize = 10;

/// The highest sampling frequency a client may ask for.
///
/// The field is a `u32` and the extension names no bound, but a server that
/// takes it at its word does arithmetic on it — the buffer size in frames, the
/// bytes a second — and `u32::MAX` overflows the first of those before anything
/// else notices. 192 kHz is above every rate real audio hardware or file format
/// uses, so nothing a client could legitimately want is refused here, and a
/// number outside it is a client that means something else by the field.
pub const MAX_FREQUENCY: u32 = 192_000;

/// A sample's encoding, as the extension numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    U8 = 0,
    S8 = 1,
    U16 = 2,
    S16 = 3,
    U32 = 4,
    S32 = 5,
}

impl SampleFormat {
    /// The format a wire byte names, if any.
    pub fn from_wire(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::U8,
            1 => Self::S8,
            2 => Self::U16,
            3 => Self::S16,
            4 => Self::U32,
            5 => Self::S32,
            _ => return None,
        })
    }

    /// One sample's width in bytes.
    pub fn bytes(self) -> usize {
        match self {
            Self::U8 | Self::S8 => 1,
            Self::U16 | Self::S16 => 2,
            Self::U32 | Self::S32 => 4,
        }
    }
}

/// The format a client asked for: what every sample in an [`audio_data`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample: SampleFormat,
    /// 1 or 2.
    pub channels: u8,
    /// Samples per second per channel.
    pub frequency: u32,
}

impl AudioFormat {
    /// What QEMU streams to a client that enables audio without setting a
    /// format: CD-quality signed 16-bit stereo.
    pub const DEFAULT: Self = Self { sample: SampleFormat::S16, channels: 2, frequency: 44_100 };

    /// The bytes of one frame: one sample per channel.
    pub fn frame_bytes(&self) -> usize {
        self.sample.bytes() * usize::from(self.channels)
    }
}

/// An audio submessage from the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAudio {
    Enable,
    Disable,
    SetFormat(AudioFormat),
}

/// Why a client's audio submessage could not be one.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AudioParseError {
    #[error("QEMU submessage {0} is not audio, the one QEMU extension spoken here")]
    UnknownSubmessage(u8),
    #[error("audio operation {0} is not enable, disable or set-format")]
    UnknownOperation(u16),
    #[error("sample format {0} is not one of the six the extension names")]
    UnknownSampleFormat(u8),
    #[error("{0} channels; the extension allows 1 or 2")]
    BadChannels(u8),
    #[error("a frequency of 0")]
    ZeroFrequency,
    #[error("a frequency of {0} Hz, over the {MAX_FREQUENCY} Hz this server accepts")]
    FrequencyTooHigh(u32),
}

/// Parse the audio submessage at the front of `buf`, whose first byte is
/// [`MSG_QEMU`]. `Ok(None)` means more bytes are needed.
pub fn parse_client(buf: &[u8]) -> Result<Option<(ClientAudio, usize)>, AudioParseError> {
    debug_assert_eq!(buf.first(), Some(&MSG_QEMU));
    if buf.len() < CLIENT_AUDIO_SWITCH_LEN {
        return Ok(None);
    }
    if buf[1] != SUBMESSAGE_AUDIO {
        return Err(AudioParseError::UnknownSubmessage(buf[1]));
    }
    let operation = u16::from_be_bytes([buf[2], buf[3]]);
    match operation {
        CLIENT_AUDIO_ENABLE => Ok(Some((ClientAudio::Enable, CLIENT_AUDIO_SWITCH_LEN))),
        CLIENT_AUDIO_DISABLE => Ok(Some((ClientAudio::Disable, CLIENT_AUDIO_SWITCH_LEN))),
        CLIENT_AUDIO_SET_FORMAT => {
            if buf.len() < CLIENT_AUDIO_FORMAT_LEN {
                return Ok(None);
            }
            let sample = SampleFormat::from_wire(buf[4]).ok_or(AudioParseError::UnknownSampleFormat(buf[4]))?;
            let channels = buf[5];
            if !(1..=2).contains(&channels) {
                return Err(AudioParseError::BadChannels(channels));
            }
            let frequency = u32::from_be_bytes([buf[6], buf[7], buf[8], buf[9]]);
            if frequency == 0 {
                return Err(AudioParseError::ZeroFrequency);
            }
            if frequency > MAX_FREQUENCY {
                return Err(AudioParseError::FrequencyTooHigh(frequency));
            }
            Ok(Some((ClientAudio::SetFormat(AudioFormat { sample, channels, frequency }), CLIENT_AUDIO_FORMAT_LEN)))
        }
        other => Err(AudioParseError::UnknownOperation(other)),
    }
}

/// The announcement: an empty pseudo-rectangle of encoding -259, sent inside a
/// `FramebufferUpdate` as its whole content.
pub fn audio_rect() -> [u8; 12] {
    crate::msg::rect_header(0, 0, 0, 0, crate::ENCODING_QEMU_AUDIO)
}

fn server_op(operation: u16) -> [u8; 4] {
    let op = operation.to_be_bytes();
    [MSG_QEMU, SUBMESSAGE_AUDIO, op[0], op[1]]
}

/// Server → client: a stream is starting; data follows.
pub fn audio_begin() -> [u8; 4] {
    server_op(SERVER_AUDIO_BEGIN)
}

/// Server → client: the stream stopped.
pub fn audio_end() -> [u8; 4] {
    server_op(SERVER_AUDIO_END)
}

/// Server → client: `samples`, interleaved little-endian in the client's
/// format, whose length is a whole number of frames.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | 255 |
/// | 1 | U8 | 1 |
/// | 2 | U16 | operation, 2 |
/// | 4 | U32 | length of the samples |
/// | 8 | U8[] | samples |
pub fn audio_data(samples: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(8 + samples.len());
    msg.extend_from_slice(&server_op(SERVER_AUDIO_DATA));
    msg.extend_from_slice(&(samples.len() as u32).to_be_bytes());
    msg.extend_from_slice(samples);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_client_operations_parse() {
        assert_eq!(parse_client(&[255, 1, 0, 0]), Ok(Some((ClientAudio::Enable, 4))));
        assert_eq!(parse_client(&[255, 1, 0, 1, 9]), Ok(Some((ClientAudio::Disable, 4))));
        let set = [255, 1, 0, 2, 3, 2, 0, 0, 0xBB, 0x80];
        assert_eq!(
            parse_client(&set),
            Ok(Some((ClientAudio::SetFormat(AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 }), 10)))
        );
        assert_eq!(parse_client(&[255, 1, 0]), Ok(None));
        assert_eq!(parse_client(&set[..9]), Ok(None));
    }

    #[test]
    fn a_bad_submessage_operation_or_format_is_fatal() {
        assert_eq!(parse_client(&[255, 2, 0, 0]), Err(AudioParseError::UnknownSubmessage(2)));
        assert_eq!(parse_client(&[255, 1, 0, 3]), Err(AudioParseError::UnknownOperation(3)));
        assert_eq!(parse_client(&[255, 1, 0, 2, 6, 2, 0, 0, 0xBB, 0x80]), Err(AudioParseError::UnknownSampleFormat(6)));
        assert_eq!(parse_client(&[255, 1, 0, 2, 3, 3, 0, 0, 0xBB, 0x80]), Err(AudioParseError::BadChannels(3)));
        assert_eq!(parse_client(&[255, 1, 0, 2, 3, 1, 0, 0, 0, 0]), Err(AudioParseError::ZeroFrequency));
    }

    /// The frequency is bounded at both ends. A `u32::MAX` accepted here would
    /// overflow the frame count the capture is asked for, so the ceiling is a
    /// parse rule rather than a check at the point of use.
    #[test]
    fn a_frequency_past_the_ceiling_is_fatal() {
        let set_at = |frequency: u32| {
            let f = frequency.to_be_bytes();
            parse_client(&[255, 1, 0, 2, 3, 2, f[0], f[1], f[2], f[3]])
        };
        assert!(matches!(set_at(MAX_FREQUENCY), Ok(Some((ClientAudio::SetFormat(_), _)))));
        assert_eq!(set_at(MAX_FREQUENCY + 1), Err(AudioParseError::FrequencyTooHigh(MAX_FREQUENCY + 1)));
        assert_eq!(set_at(u32::MAX), Err(AudioParseError::FrequencyTooHigh(u32::MAX)));
        // Every rate that survives leaves the daemon's frame count well inside
        // a u32 (`frequency * 20 / 1000` in `wlshare::audio`).
        assert!(MAX_FREQUENCY.checked_mul(20).is_some());
        // And the rates real clients ask for are all far below it.
        for rate in [8_000, 44_100, 48_000, 96_000] {
            assert!(matches!(set_at(rate), Ok(Some((ClientAudio::SetFormat(_), _)))));
        }
    }

    #[test]
    fn sample_widths_and_frame_sizes() {
        assert_eq!(SampleFormat::from_wire(0), Some(SampleFormat::U8));
        assert_eq!(SampleFormat::from_wire(5), Some(SampleFormat::S32));
        assert_eq!(AudioFormat::DEFAULT.frame_bytes(), 4);
        assert_eq!(AudioFormat { sample: SampleFormat::U8, channels: 1, frequency: 8000 }.frame_bytes(), 1);
        assert_eq!(AudioFormat { sample: SampleFormat::S32, channels: 2, frequency: 96_000 }.frame_bytes(), 8);
    }

    /// The server messages, read back by a decoder written from the
    /// extension's text rather than from the builders.
    #[test]
    fn server_messages_have_the_documented_layouts() {
        assert_eq!(audio_rect(), [0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFE, 0xFD]);
        assert_eq!(i32::from_be_bytes([0xFF, 0xFF, 0xFE, 0xFD]), -259);
        assert_eq!(audio_begin(), [255, 1, 0, 1]);
        assert_eq!(audio_end(), [255, 1, 0, 0]);
        let data = audio_data(&[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(data[0], 255);
        assert_eq!(data[1], 1);
        assert_eq!(u16::from_be_bytes([data[2], data[3]]), SERVER_AUDIO_DATA);
        let len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        assert_eq!(len, 8);
        assert_eq!(&data[8..8 + len], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(data.len(), 8 + len);
    }
}
