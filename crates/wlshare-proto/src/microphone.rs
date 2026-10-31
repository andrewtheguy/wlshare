//! The microphone extension: how a client lends the desktop the microphone in
//! front of its user.
//!
//! The camera extension's twin ([`crate::camera`]), and the fourth of wlshare's
//! private pairs: one pseudo-encoding and one message type, used in both
//! directions. The audio extension ([`crate::audio`]) carries sound from the
//! server alone, and no registered extension carries it the other way.
//!
//! - The client lists [`crate::ENCODING_MICROPHONE`] in `SetEncodings`. A server
//!   that does not know it ignores it, as RFB requires.
//! - The server answers **every** such `SetEncodings` with a
//!   [`microphone_available`] — that answer is the only way support is ever
//!   announced.
//! - The client plugs a microphone ([`plug`]). The server makes a microphone of
//!   it that the desktop's applications can record from, and unplugs it again
//!   on an [`unplug`] or when the client leaves.
//! - Whether anything is sent is the desktop's decision, not the client's: the
//!   server sends [`microphone_start`] when an application starts recording, naming
//!   the format it wants the samples in, and [`microphone_stop`] when the last one
//!   stops. Between the two the client sends samples ([`sample`]), and outside
//!   them it sends nothing.
//!
//! The format is the server's to choose, as a host's is over RDP: samples are
//! always signed 16-bit little-endian and interleaved, and the start names the
//! channel count and rate. A client converts what its microphone captures into
//! them. Unlike H.264 a sample depends on nothing before it, so there is no
//! keyframe to ask for: a lost one is a moment of silence.

use thiserror::Error;

/// The message type, used by both directions; outside every registered type.
pub const MSG_MICROPHONE: u8 = 0xE3;

/// Client operation: a microphone is lent.
pub const CLIENT_MICROPHONE_PLUG: u8 = 0;
/// Client operation: the microphone is gone.
pub const CLIENT_MICROPHONE_UNPLUG: u8 = 1;
/// Client operation: samples follow, after their length.
pub const CLIENT_MICROPHONE_SAMPLE: u8 = 2;

/// Server operation: this server takes a microphone. The answer to `SetEncodings`.
pub const SERVER_MICROPHONE_AVAILABLE: u8 = 0;
/// Server operation: an application is recording; samples in the format that
/// follows are wanted.
pub const SERVER_MICROPHONE_START: u8 = 1;
/// Server operation: the last application stopped; send no more.
pub const SERVER_MICROPHONE_STOP: u8 = 2;

/// The bytes of a plug, an unplug, an available and a stop, type included.
pub const MICROPHONE_BARE_LEN: usize = 4;
/// The bytes of a start, type included.
pub const MICROPHONE_START_LEN: usize = 12;
/// The bytes of a sample before its PCM.
pub const MICROPHONE_SAMPLE_HEADER_LEN: usize = 8;

/// The longest run of PCM one sample may carry.
///
/// The field is a `u32` and a server buffers the whole sample before it can hand
/// it on, so it is bounded where cut text and camera samples are. A client sends
/// a few tens of milliseconds at a time; this is over a second of 48 kHz stereo,
/// and past it is a client that means something else by the field.
pub const MAX_SAMPLE: usize = 256 * 1024;

/// The PCM a server wants: signed 16-bit little-endian, interleaved, at this
/// channel count and rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MicrophoneFormat {
    pub channels: u16,
    /// Samples per second per channel.
    pub frequency: u32,
}

impl MicrophoneFormat {
    /// The bytes of one frame: a 16-bit sample per channel.
    pub fn frame_bytes(&self) -> usize {
        2 * usize::from(self.channels)
    }
}

/// A microphone submessage from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMicrophone {
    Plug,
    Unplug,
    /// Interleaved PCM in the format the last start named.
    Sample(Vec<u8>),
}

/// Why a client's microphone message could not be one.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MicrophoneParseError {
    #[error("microphone operation {0} is not one this end takes from the other")]
    UnknownOperation(u8),
    #[error("a microphone sample of {0} bytes is over the {MAX_SAMPLE}-byte ceiling")]
    SampleTooLong(usize),
}

/// Parse the microphone message at the front of `buf`, whose first byte is
/// [`MSG_MICROPHONE`]. `Ok(None)` means more bytes are needed.
pub fn parse_client(buf: &[u8]) -> Result<Option<(ClientMicrophone, usize)>, MicrophoneParseError> {
    debug_assert_eq!(buf.first(), Some(&MSG_MICROPHONE));
    if buf.len() < MICROPHONE_BARE_LEN {
        return Ok(None);
    }
    match buf[1] {
        CLIENT_MICROPHONE_PLUG => Ok(Some((ClientMicrophone::Plug, MICROPHONE_BARE_LEN))),
        CLIENT_MICROPHONE_UNPLUG => Ok(Some((ClientMicrophone::Unplug, MICROPHONE_BARE_LEN))),
        CLIENT_MICROPHONE_SAMPLE => {
            if buf.len() < MICROPHONE_SAMPLE_HEADER_LEN {
                return Ok(None);
            }
            let len = crate::u32_at(buf, 4) as usize;
            if len > MAX_SAMPLE {
                return Err(MicrophoneParseError::SampleTooLong(len));
            }
            let end = MICROPHONE_SAMPLE_HEADER_LEN + len;
            if buf.len() < end {
                return Ok(None);
            }
            Ok(Some((ClientMicrophone::Sample(buf[MICROPHONE_SAMPLE_HEADER_LEN..end].to_vec()), end)))
        }
        other => Err(MicrophoneParseError::UnknownOperation(other)),
    }
}

fn bare(operation: u8) -> [u8; MICROPHONE_BARE_LEN] {
    [MSG_MICROPHONE, operation, 0, 0]
}

/// Server → client: this server takes a microphone.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE3` |
/// | 1 | U8 | operation, 0 |
/// | 2 | U16 | padding |
pub fn microphone_available() -> [u8; MICROPHONE_BARE_LEN] {
    bare(SERVER_MICROPHONE_AVAILABLE)
}

/// Server → client: an application is recording, and wants signed 16-bit
/// little-endian PCM in `format`.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE3` |
/// | 1 | U8 | operation, 1 |
/// | 2 | U16 | padding |
/// | 4 | U16 | channels |
/// | 6 | U16 | padding |
/// | 8 | U32 | frequency |
pub fn microphone_start(format: MicrophoneFormat) -> [u8; MICROPHONE_START_LEN] {
    let mut msg = [0u8; MICROPHONE_START_LEN];
    msg[..MICROPHONE_BARE_LEN].copy_from_slice(&bare(SERVER_MICROPHONE_START));
    msg[4..6].copy_from_slice(&format.channels.to_be_bytes());
    msg[8..12].copy_from_slice(&format.frequency.to_be_bytes());
    msg
}

/// Server → client: nothing is recording any more; send no more samples.
pub fn microphone_stop() -> [u8; MICROPHONE_BARE_LEN] {
    bare(SERVER_MICROPHONE_STOP)
}

/// Client → server: a microphone is lent.
pub fn plug() -> [u8; MICROPHONE_BARE_LEN] {
    bare(CLIENT_MICROPHONE_PLUG)
}

/// Client → server: the microphone is gone.
pub fn unplug() -> [u8; MICROPHONE_BARE_LEN] {
    bare(CLIENT_MICROPHONE_UNPLUG)
}

/// Client → server: interleaved PCM in the format the last start named.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE3` |
/// | 1 | U8 | operation, 2 |
/// | 2 | U16 | padding |
/// | 4 | U32 | length of the PCM |
/// | 8 | U8[] | the PCM |
///
/// PCM over [`MAX_SAMPLE`] is the server's to refuse, and ends the session: a
/// client sends none.
pub fn sample(pcm: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(MICROPHONE_SAMPLE_HEADER_LEN + pcm.len());
    msg.extend_from_slice(&bare(CLIENT_MICROPHONE_SAMPLE));
    msg.extend_from_slice(&(pcm.len() as u32).to_be_bytes());
    msg.extend_from_slice(pcm);
    msg
}

/// A microphone submessage from the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMicrophone {
    /// The server takes a microphone: the answer to `SetEncodings`.
    Available,
    /// An application started recording, and wants PCM in this format.
    Start(MicrophoneFormat),
    /// The last application stopped.
    Stop,
}

/// How many bytes the server message whose first [`MICROPHONE_BARE_LEN`] are
/// `header` is in all. An operation this client does not know is an error: the
/// messages share no length field, so one that cannot be measured leaves the
/// stream at an offset nothing recovers from.
pub fn server_len(header: &[u8; MICROPHONE_BARE_LEN]) -> Result<usize, MicrophoneParseError> {
    debug_assert_eq!(header[0], MSG_MICROPHONE);
    match header[1] {
        SERVER_MICROPHONE_START => Ok(MICROPHONE_START_LEN),
        SERVER_MICROPHONE_AVAILABLE | SERVER_MICROPHONE_STOP => Ok(MICROPHONE_BARE_LEN),
        other => Err(MicrophoneParseError::UnknownOperation(other)),
    }
}

/// Parse the server's microphone message at the front of `buf`, whose first
/// byte is [`MSG_MICROPHONE`]. `Ok(None)` means more bytes are needed.
pub fn parse_server(buf: &[u8]) -> Result<Option<(ServerMicrophone, usize)>, MicrophoneParseError> {
    let Some(header) = buf.first_chunk::<MICROPHONE_BARE_LEN>() else { return Ok(None) };
    let len = server_len(header)?;
    if buf.len() < len {
        return Ok(None);
    }
    let msg = match header[1] {
        SERVER_MICROPHONE_AVAILABLE => ServerMicrophone::Available,
        SERVER_MICROPHONE_START => {
            ServerMicrophone::Start(MicrophoneFormat { channels: crate::u16_at(buf, 4), frequency: crate::u32_at(buf, 8) })
        }
        _ => ServerMicrophone::Stop,
    };
    Ok(Some((msg, len)))
}

#[cfg(test)]
mod tests;
