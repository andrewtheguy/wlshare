//! The audio extension: how the desktop's sound reaches a client over the RFB
//! connection it already has, as FLAC or as Opus, whichever the client asked
//! for.
//!
//! - The client lists the pseudo-encoding [`crate::ENCODING_AUDIO`] (`WLSF`) in
//!   `SetEncodings`, and beside it [`crate::ENCODING_AUDIO_OPUS`] (`WLOP`) for
//!   the sound as Opus in place of FLAC ([`Codec`]). The server announces
//!   support with an empty pseudo-rectangle of the first ([`audio_rect`]) in a
//!   `FramebufferUpdate` — the only way support is ever announced, as with
//!   ExtendedDesktopSize. A server that does not speak it says nothing.
//! - The client then sets the sample format it wants ([`set_format`]) and
//!   enables audio ([`enable`]); it may disable it again ([`disable`]). These
//!   three are the client messages of the QEMU Audio extension `rfbproto`
//!   registers — message type 255, submessage 1 — taken as they are, though
//!   QEMU's pseudo-encoding, -259, is not spoken: what it promises is raw
//!   samples, and none are sent.
//! - A client that asked for Opus names the rate it is coded at
//!   ([`set_bitrate`]), before the stream or while it runs: a private
//!   operation beside QEMU's three, which an Opus stream cannot start without.
//! - The server sends [`audio_begin`] when a stream starts and [`audio_end`]
//!   when it stops, QEMU's messages again, and between them the sound, one
//!   frame of it to a message of the private type [`MSG_AUDIO_FRAME`]
//!   ([`frame_header`]): a FLAC frame, or an Opus packet.
//!
//! Every frame is [`AudioFormat::block_frames`] frames of the format the client
//! set, twenty milliseconds, so nothing else about the stream is sent: a FLAC
//! client builds its `STREAMINFO`, and an Opus one its `OpusHead`, from what it
//! asked for. A FLAC frame decodes on its own; an Opus packet decodes from the
//! packets before it, as any Opus stream does, and nothing on the wire numbers
//! them, so a server sends every packet it coded.
//!
//! FLAC stores only signed samples, of at most 24 bits, so the formats are the
//! four of 8 and 16 bits. An unsigned sample has its top bit flipped before it
//! is coded, which maps its range exactly onto the signed one of the same
//! width; the client flips it back. Samples are little-endian.

use thiserror::Error;

/// The message type the client's messages and the server's begin and end use,
/// shared with every other QEMU extension; [`SUBMESSAGE_AUDIO`] names this one.
pub const MSG_QEMU: u8 = 255;
/// The submessage type under [`MSG_QEMU`] that is audio.
pub const SUBMESSAGE_AUDIO: u8 = 1;

/// The frame message's type, server → client only; outside every registered
/// type. One FLAC frame or one Opus packet follows its header.
pub const MSG_AUDIO_FRAME: u8 = 0xE4;
/// The bytes of a frame message before the frame, type included.
pub const AUDIO_FRAME_HEADER_LEN: usize = 8;
/// The most bytes one frame may be. The largest block there is — twenty
/// milliseconds of 16-bit stereo at [`MAX_FREQUENCY`] — is 7680 bytes of
/// samples, and FLAC adds a few header bytes at worst, while an Opus packet is
/// smaller still, so a length past this is a server that has lost its framing
/// rather than a frame to buffer.
pub const MAX_AUDIO_FRAME: usize = 64 * 1024;

/// Client operation: start sending audio.
pub const CLIENT_AUDIO_ENABLE: u16 = 0;
/// Client operation: stop sending audio.
pub const CLIENT_AUDIO_DISABLE: u16 = 1;
/// Client operation: the sample format the client wants, which follows.
pub const CLIENT_AUDIO_SET_FORMAT: u16 = 2;
/// Client operation: the rate Opus is coded at, which follows. wlshare's own,
/// beside QEMU's three.
pub const CLIENT_AUDIO_SET_BITRATE: u16 = 3;

/// Server operation: the stream stopped.
pub const SERVER_AUDIO_END: u16 = 0;
/// Server operation: a stream started.
pub const SERVER_AUDIO_BEGIN: u16 = 1;

/// The bytes of an enable, a disable, a begin and an end, type and submessage
/// included.
pub const AUDIO_SWITCH_LEN: usize = 4;
/// The bytes of a set-format, type and submessage included.
pub const CLIENT_AUDIO_FORMAT_LEN: usize = 10;
/// The bytes of a set-bitrate, type and submessage included.
pub const CLIENT_AUDIO_BITRATE_LEN: usize = 8;

/// The lowest Opus rate a client may ask for, in bits per second: libopus's
/// own floor.
pub const OPUS_BITRATE_MIN: u32 = 6_000;
/// The highest: libopus's own ceiling.
pub const OPUS_BITRATE_MAX: u32 = 510_000;

/// The lowest sampling frequency a client may ask for.
///
/// A FLAC frame here is twenty milliseconds, and FLAC has no block shorter
/// than 16 frames, which puts the floor at 800 Hz; 8 kHz is the lowest rate
/// real audio uses, so nothing a client could legitimately want is refused.
pub const MIN_FREQUENCY: u32 = 8_000;

/// The highest sampling frequency a client may ask for.
///
/// The field is a `u32`, and a server that took it at its word would overflow
/// the buffer size in frames it asks its capture for. FLAC's own ceiling is the
/// twenty bits its stream header has for the rate; this one is twice what a
/// desktop's audio graph runs at, and more than any client asks for.
pub const MAX_FREQUENCY: u32 = 96_000;

/// A sample's encoding, as QEMU's set-format numbers them. Its 32-bit codes,
/// 4 and 5, are not here: FLAC stores at most 24 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    U8 = 0,
    S8 = 1,
    U16 = 2,
    S16 = 3,
}

impl SampleFormat {
    /// The format a wire byte names, if it is one carried here.
    pub fn from_wire(code: u8) -> Option<Self> {
        Some(match code {
            0 => Self::U8,
            1 => Self::S8,
            2 => Self::U16,
            3 => Self::S16,
            _ => return None,
        })
    }

    /// One sample's width in bytes.
    pub fn bytes(self) -> usize {
        match self {
            Self::U8 | Self::S8 => 1,
            Self::U16 | Self::S16 => 2,
        }
    }

    /// Whether the top bit is flipped on the way into FLAC and back out.
    pub fn unsigned(self) -> bool {
        matches!(self, Self::U8 | Self::U16)
    }
}

/// The format a client asked for: what the samples every FLAC frame decodes to
/// are in, and what the server's capture hands its Opus encoder.
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

    /// The frames in every FLAC frame and Opus packet: twenty milliseconds,
    /// rounded down — 960 at 48 kHz, 882 at 44.1.
    pub fn block_frames(&self) -> usize {
        (self.frequency / 50) as usize
    }

    /// Whether this is a format the extension carries: 1 or 2 channels, at
    /// [`MIN_FREQUENCY`] to [`MAX_FREQUENCY`]. A set-format is parsed only
    /// into one that is.
    pub fn check(&self) -> Result<(), AudioParseError> {
        if !(1..=2).contains(&self.channels) {
            return Err(AudioParseError::BadChannels(self.channels));
        }
        if self.frequency < MIN_FREQUENCY {
            return Err(AudioParseError::FrequencyTooLow(self.frequency));
        }
        if self.frequency > MAX_FREQUENCY {
            return Err(AudioParseError::FrequencyTooHigh(self.frequency));
        }
        Ok(())
    }
}

/// What the sound is coded as, which the client chooses in its `SetEncodings`:
/// Opus where [`crate::ENCODING_AUDIO_OPUS`] is listed beside
/// [`crate::ENCODING_AUDIO`], FLAC where it is not. A pseudo-encoding rather
/// than a message, as the video stream's choices are, so it rides the list that
/// asks for the sound at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Flac,
    Opus,
}

impl Codec {
    /// The codec `encodings` asks for.
    pub fn listed(encodings: &[i32]) -> Self {
        if encodings.contains(&crate::ENCODING_AUDIO_OPUS) { Self::Opus } else { Self::Flac }
    }
}

/// An audio submessage from the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAudio {
    Enable,
    Disable,
    SetFormat(AudioFormat),
    /// The rate Opus is coded at, in bits per second.
    SetBitrate(u32),
}

/// An audio submessage from the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerAudio {
    /// A stream started; its frames follow.
    Begin,
    /// The stream stopped.
    End,
}

/// Why an audio submessage could not be one.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum AudioParseError {
    /// The QEMU submessages share no length field, so one that is not audio
    /// cannot be measured, and the stream is at an offset nothing recovers from.
    #[error("QEMU submessage {0} is not audio, the one QEMU extension spoken here")]
    UnknownSubmessage(u8),
    #[error("audio operation {0} is not one this end takes from the other")]
    UnknownOperation(u16),
    #[error("sample format {0} is not U8, S8, U16 or S16, the ones FLAC carries here")]
    UnknownSampleFormat(u8),
    #[error("{0} channels; the extension allows 1 or 2")]
    BadChannels(u8),
    #[error("a frequency of {0} Hz, under the {MIN_FREQUENCY} Hz the extension carries")]
    FrequencyTooLow(u32),
    #[error("a frequency of {0} Hz, over the {MAX_FREQUENCY} Hz the extension carries")]
    FrequencyTooHigh(u32),
    #[error("an Opus rate of {0} bit/s, outside the {OPUS_BITRATE_MIN} to {OPUS_BITRATE_MAX} Opus is coded at")]
    BadBitrate(u32),
    #[error("an audio frame of {0} bytes is over the {MAX_AUDIO_FRAME}-byte ceiling")]
    FrameTooLong(usize),
}

/// Whether `bitrate`, in bits per second, is one Opus is coded at. A
/// set-bitrate is parsed only into one that is.
pub fn check_bitrate(bitrate: u32) -> Result<(), AudioParseError> {
    if (OPUS_BITRATE_MIN..=OPUS_BITRATE_MAX).contains(&bitrate) { Ok(()) } else { Err(AudioParseError::BadBitrate(bitrate)) }
}

fn switch(operation: u16) -> [u8; AUDIO_SWITCH_LEN] {
    let [a, b] = operation.to_be_bytes();
    [MSG_QEMU, SUBMESSAGE_AUDIO, a, b]
}

/// The submessage's operation, once the header is there and names audio.
fn operation(buf: &[u8]) -> Result<Option<u16>, AudioParseError> {
    if buf.len() < AUDIO_SWITCH_LEN {
        return Ok(None);
    }
    if buf[1] != SUBMESSAGE_AUDIO {
        return Err(AudioParseError::UnknownSubmessage(buf[1]));
    }
    Ok(Some(crate::u16_at(buf, 2)))
}

/// Client → server: start the stream.
pub fn enable() -> [u8; AUDIO_SWITCH_LEN] {
    switch(CLIENT_AUDIO_ENABLE)
}

/// Client → server: stop the stream.
pub fn disable() -> [u8; AUDIO_SWITCH_LEN] {
    switch(CLIENT_AUDIO_DISABLE)
}

/// Client → server: the format every following FLAC frame decodes to, and the
/// one Opus is coded from. A format [`AudioFormat::check`] refuses is the
/// server's to refuse, and ends the session: a client sends none.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | 255 |
/// | 1 | U8 | 1 |
/// | 2 | U16 | operation, 2 |
/// | 4 | U8 | sample format |
/// | 5 | U8 | channels |
/// | 6 | U32 | frequency |
pub fn set_format(format: AudioFormat) -> [u8; CLIENT_AUDIO_FORMAT_LEN] {
    let mut msg = [0u8; CLIENT_AUDIO_FORMAT_LEN];
    msg[..AUDIO_SWITCH_LEN].copy_from_slice(&switch(CLIENT_AUDIO_SET_FORMAT));
    msg[4] = format.sample as u8;
    msg[5] = format.channels;
    msg[6..].copy_from_slice(&format.frequency.to_be_bytes());
    msg
}

/// Client → server: the rate, in bits per second, Opus is coded at from the
/// next packet on, before a stream or during one. A rate outside
/// [`OPUS_BITRATE_MIN`] to [`OPUS_BITRATE_MAX`] is the server's to refuse, and
/// ends the session.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | 255 |
/// | 1 | U8 | 1 |
/// | 2 | U16 | operation, 3 |
/// | 4 | U32 | bits per second |
pub fn set_bitrate(bitrate: u32) -> [u8; CLIENT_AUDIO_BITRATE_LEN] {
    let mut msg = [0u8; CLIENT_AUDIO_BITRATE_LEN];
    msg[..AUDIO_SWITCH_LEN].copy_from_slice(&switch(CLIENT_AUDIO_SET_BITRATE));
    msg[4..].copy_from_slice(&bitrate.to_be_bytes());
    msg
}

/// Parse the client's audio submessage at the front of `buf`, whose first byte
/// is [`MSG_QEMU`]. `Ok(None)` means more bytes are needed.
pub fn parse_client(buf: &[u8]) -> Result<Option<(ClientAudio, usize)>, AudioParseError> {
    debug_assert_eq!(buf.first(), Some(&MSG_QEMU));
    let Some(operation) = operation(buf)? else { return Ok(None) };
    match operation {
        CLIENT_AUDIO_ENABLE => Ok(Some((ClientAudio::Enable, AUDIO_SWITCH_LEN))),
        CLIENT_AUDIO_DISABLE => Ok(Some((ClientAudio::Disable, AUDIO_SWITCH_LEN))),
        CLIENT_AUDIO_SET_FORMAT => {
            if buf.len() < CLIENT_AUDIO_FORMAT_LEN {
                return Ok(None);
            }
            let sample = SampleFormat::from_wire(buf[4]).ok_or(AudioParseError::UnknownSampleFormat(buf[4]))?;
            let format = AudioFormat { sample, channels: buf[5], frequency: crate::u32_at(buf, 6) };
            format.check()?;
            Ok(Some((ClientAudio::SetFormat(format), CLIENT_AUDIO_FORMAT_LEN)))
        }
        CLIENT_AUDIO_SET_BITRATE => {
            if buf.len() < CLIENT_AUDIO_BITRATE_LEN {
                return Ok(None);
            }
            let bitrate = crate::u32_at(buf, 4);
            check_bitrate(bitrate)?;
            Ok(Some((ClientAudio::SetBitrate(bitrate), CLIENT_AUDIO_BITRATE_LEN)))
        }
        other => Err(AudioParseError::UnknownOperation(other)),
    }
}

/// The announcement: an empty pseudo-rectangle of [`crate::ENCODING_AUDIO`],
/// sent inside a `FramebufferUpdate` as its whole content.
pub fn audio_rect() -> [u8; 12] {
    crate::rect_header(0, 0, 0, 0, crate::ENCODING_AUDIO)
}

/// Server → client: a stream is starting; its frames follow.
pub fn audio_begin() -> [u8; AUDIO_SWITCH_LEN] {
    switch(SERVER_AUDIO_BEGIN)
}

/// Server → client: the stream stopped.
pub fn audio_end() -> [u8; AUDIO_SWITCH_LEN] {
    switch(SERVER_AUDIO_END)
}

/// Parse the server's audio submessage at the front of `buf`, whose first byte
/// is [`MSG_QEMU`]. `Ok(None)` means more bytes are needed.
///
/// Anything but begin and end is an error rather than a skip: the QEMU
/// submessages have no common length field, so a message a client cannot
/// measure leaves the stream at an offset nothing can recover from. That
/// includes QEMU's data operation, raw samples, which this extension never
/// sends.
pub fn parse_server(buf: &[u8]) -> Result<Option<(ServerAudio, usize)>, AudioParseError> {
    debug_assert_eq!(buf.first(), Some(&MSG_QEMU));
    let Some(operation) = operation(buf)? else { return Ok(None) };
    match operation {
        SERVER_AUDIO_BEGIN => Ok(Some((ServerAudio::Begin, AUDIO_SWITCH_LEN))),
        SERVER_AUDIO_END => Ok(Some((ServerAudio::End, AUDIO_SWITCH_LEN))),
        other => Err(AudioParseError::UnknownOperation(other)),
    }
}

/// Server → client: the header of a frame message whose frame is `len` bytes,
/// which follow it.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE4` |
/// | 1 | U8\[3\] | padding |
/// | 4 | U32 | length of the frame |
/// | 8 | U8[] | one FLAC frame or one Opus packet |
pub fn frame_header(len: u32) -> [u8; AUDIO_FRAME_HEADER_LEN] {
    let [a, b, c, d] = len.to_be_bytes();
    [MSG_AUDIO_FRAME, 0, 0, 0, a, b, c, d]
}

/// The length of the frame a frame message's header announces: an error past
/// [`MAX_AUDIO_FRAME`], which a client reads past rather than allocates.
pub fn frame_len(header: &[u8; AUDIO_FRAME_HEADER_LEN]) -> Result<usize, AudioParseError> {
    debug_assert_eq!(header[0], MSG_AUDIO_FRAME);
    let len = crate::u32_at(header, 4) as usize;
    if len > MAX_AUDIO_FRAME {
        return Err(AudioParseError::FrameTooLong(len));
    }
    Ok(len)
}

#[cfg(test)]
mod tests;
