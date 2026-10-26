//! The audio extension's encoders: the desktop's sound as the client asked
//! for it, FLAC or Opus, in the extension's frame messages.
//!
//! The extension itself — what the client lists and sends, the begin, the end
//! and the frame message — is [`wlshare_proto::audio`]'s. What is here is the
//! coding: a frame message for every [`AudioFormat::block_frames`] frames the
//! capture hands over, twenty milliseconds.
//!
//! FLAC is lossless: the client decodes exactly the samples the capture
//! produced, while music takes about half of its PCM rate, speech under a
//! third, and silence a few bytes a frame. The FLAC stream header (`STREAMINFO`)
//! is never sent, because everything in it is already agreed, so a client
//! builds the header itself. Each frame decodes on its own, so one a session
//! dropped costs nothing but its own samples.
//!
//! The tests read every frame back as a client does: `streaminfo` is the
//! header it builds, and `FlacDecoder` turns each frame back into samples in
//! the format it set; an Opus stream's packets are decoded by FFmpeg behind the
//! `OpusHead` a client builds.
//!
//! Opus is the lossy choice, for a client whose own listener takes Opus: the
//! zDesk gateway hands each packet to the browser as it came, where it would
//! otherwise decode the FLAC and code Opus itself. A packet is twenty
//! milliseconds too, at the one frame size the whole stream keeps, and decodes
//! from the packets before it as any Opus stream does, so a server must send
//! every packet it coded: one that has to lose sound loses it before the
//! encoder. The format's frequency must be one Opus codes at
//! ([`sound_opus::RATES`]); its sample format says only what the capture hands
//! the encoder. The coding is `sound-opus`'s, the one place a desktop's sound is
//! made Opus for wlshare and the gateway alike, with libopus linked statically
//! under it.

use sound_flac::{Encoder, Stream};
use thiserror::Error;
use wlshare_proto::audio::{AUDIO_FRAME_HEADER_LEN, AudioFormat, AudioParseError, Codec, check_bitrate, frame_header};
#[cfg(test)]
use wlshare_proto::audio::{MAX_AUDIO_FRAME, MAX_FREQUENCY, MIN_FREQUENCY, SampleFormat};

/// Why a frame could not be made.
#[derive(Debug, Error)]
pub enum AudioEncodeError {
    /// The format or the rate is not one the extension carries.
    #[error(transparent)]
    Unsupported(#[from] AudioParseError),
    /// The format's frequency is not one Opus codes at, or libopus refused
    /// the stream, a rate or a block of it.
    #[error(transparent)]
    Opus(#[from] sound_opus::Error),
    /// The FLAC encoder refused the stream or a block of it.
    #[error(transparent)]
    Flac(#[from] sound_flac::Error),
    /// The client enabled Opus without ever setting its rate: the stream has
    /// no rate of its own to start at, and the client knows what it wants.
    #[error("an Opus stream needs a set-bitrate before its enable")]
    NoBitrate,
}

/// A frame message with nothing in it yet: the header, its length to be
/// written once the frame is behind it ([`finish`]).
fn begin(capacity: usize) -> Vec<u8> {
    let mut msg = Vec::with_capacity(AUDIO_FRAME_HEADER_LEN + capacity);
    msg.extend_from_slice(&frame_header(0));
    msg
}

fn finish(mut msg: Vec<u8>) -> Vec<u8> {
    let length = (msg.len() - AUDIO_FRAME_HEADER_LEN) as u32;
    msg[..AUDIO_FRAME_HEADER_LEN].copy_from_slice(&frame_header(length));
    msg
}

/// Samples held until they fill a block: what the capture gave that did not,
/// kept for its next buffer.
#[derive(Default)]
struct Pending(Vec<u8>);

impl Pending {
    /// Add `samples` and hand every whole block of `block_bytes` to `encode`,
    /// in order, keeping what is left over.
    fn push<E>(&mut self, samples: &[u8], block_bytes: usize, mut encode: impl FnMut(&[u8]) -> Result<Vec<u8>, E>) -> Result<Vec<Vec<u8>>, E> {
        self.0.extend_from_slice(samples);
        let whole = self.0.len() / block_bytes;
        let messages = self.0.chunks_exact(block_bytes).map(&mut encode).collect::<Result<Vec<_>, E>>();
        self.0.drain(..whole * block_bytes);
        messages
    }
}

/// One stream's FLAC encoder: the capture's buffers in, [`wlshare_proto::audio::MSG_AUDIO_FRAME`]
/// messages out, one for every [`AudioFormat::block_frames`] frames.
///
/// The capture's buffers are twenty milliseconds once PipeWire settles, but
/// its first ones are often shorter and a graph running a larger quantum hands
/// over more; a FLAC frame of fixed size is what makes the stream header the
/// client builds true, so the encoder keeps what does not fill a frame for the
/// next buffer. What is left when the stream stops is under twenty
/// milliseconds, and goes with it.
///
/// A frame is made the moment its last sample arrives, by `sound-flac`'s
/// encoder, as a FLAC stream of its own, so every frame is numbered zero.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | 0xE4 |
/// | 1 | U8\[3\] | padding |
/// | 4 | U32 | length of the frame |
/// | 8 | U8[] | one FLAC frame |
pub struct FlacEncoder {
    format: AudioFormat,
    codec: Encoder,
    /// Samples not yet a whole frame's worth, as the capture gave them.
    pending: Pending,
    /// One block as the encoder takes it, a signed integer to a sample, reused.
    samples: Vec<i32>,
}

impl FlacEncoder {
    /// An encoder for `format`, if it is one the extension carries.
    pub fn new(format: AudioFormat) -> Result<Self, AudioEncodeError> {
        format.check()?;
        let codec = Encoder::new(Stream {
            rate: format.frequency,
            channels: format.channels,
            bits: 8 * format.sample.bytes() as u8,
            block: format.block_frames() as u16,
        })?;
        Ok(Self { format, codec, pending: Pending::default(), samples: Vec::new() })
    }

    /// Take `samples`, interleaved little-endian in the client's format and a
    /// whole number of frames, and return a message for every FLAC frame they
    /// completed.
    pub fn push(&mut self, samples: &[u8]) -> Result<Vec<Vec<u8>>, AudioEncodeError> {
        debug_assert!(samples.len().is_multiple_of(self.format.frame_bytes()));
        let block_bytes = self.format.block_frames() * self.format.frame_bytes();
        let mut pending = std::mem::take(&mut self.pending);
        let messages = pending.push(samples, block_bytes, |block| self.encode(block));
        self.pending = pending;
        messages
    }

    fn encode(&mut self, block: &[u8]) -> Result<Vec<u8>, AudioEncodeError> {
        // The top bit of an unsigned sample, flipped on the way in; it is in the
        // sample's last byte, the samples being little-endian.
        let flip = if self.format.sample.unsigned() { 0x80 } else { 0 };
        self.samples.clear();
        match self.format.sample.bytes() {
            1 => self.samples.extend(block.iter().map(|&sample| i32::from((sample ^ flip) as i8))),
            _ => self.samples.extend(block.as_chunks::<2>().0.iter().map(|&[low, high]| i32::from(i16::from_le_bytes([low, high ^ flip])))),
        }
        let mut msg = begin(block.len());
        self.codec.encode(&self.samples, &mut msg)?;
        Ok(finish(msg))
    }
}

/// One stream's Opus encoder: the capture's buffers in, [`wlshare_proto::audio::MSG_AUDIO_FRAME`]
/// messages out, one packet for every [`AudioFormat::block_frames`] frames,
/// twenty milliseconds, as [`FlacEncoder`]'s are and with the same message
/// around it.
///
/// The packets are `sound-opus`'s, which the zDesk gateway codes the sound
/// of a desktop it encodes itself with, so a passed stream and one made there
/// are the same. No header goes with them: a client builds `OpusHead` from the
/// format it set and [`sound_opus::PRE_SKIP`], the samples a decoder discards
/// from the start of the stream.
pub struct OpusEncoder {
    format: AudioFormat,
    codec: sound_opus::Encoder,
    /// Samples not yet a whole packet's worth, as the capture gave them.
    pending: Pending,
    /// One block as the encoder takes it, signed 16-bit, reused.
    samples: Vec<i16>,
}

impl OpusEncoder {
    /// An encoder for `format` at `bitrate` bits per second, if both are ones
    /// the extension carries as Opus.
    pub fn new(format: AudioFormat, bitrate: u32) -> Result<Self, AudioEncodeError> {
        format.check()?;
        check_bitrate(bitrate)?;
        let codec = sound_opus::Encoder::new(sound_opus::Stream { rate: format.frequency, channels: format.channels }, bitrate)?;
        Ok(Self { format, codec, pending: Pending::default(), samples: Vec::new() })
    }

    /// Move the rate to `bitrate` bits per second, from the next packet on.
    /// Nothing else changes, and a decoder needs no telling: a packet carries
    /// its own coding parameters.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), AudioEncodeError> {
        check_bitrate(bitrate)?;
        Ok(self.codec.set_bitrate(bitrate)?)
    }

    /// Take `samples`, interleaved little-endian in the client's format and a
    /// whole number of frames, and return a message for every Opus packet they
    /// completed.
    pub fn push(&mut self, samples: &[u8]) -> Result<Vec<Vec<u8>>, AudioEncodeError> {
        debug_assert!(samples.len().is_multiple_of(self.format.frame_bytes()));
        let block_bytes = self.format.block_frames() * self.format.frame_bytes();
        let mut pending = std::mem::take(&mut self.pending);
        let messages = pending.push(samples, block_bytes, |block| self.encode(block));
        self.pending = pending;
        messages
    }

    fn encode(&mut self, block: &[u8]) -> Result<Vec<u8>, AudioEncodeError> {
        // The encoder takes signed 16-bit: an unsigned sample has its top bit
        // flipped, as for FLAC, and an 8-bit one becomes the high byte.
        let flip = if self.format.sample.unsigned() { 0x80 } else { 0 };
        self.samples.clear();
        match self.format.sample.bytes() {
            1 => self.samples.extend(block.iter().map(|&sample| i16::from((sample ^ flip) as i8) << 8)),
            _ => self.samples.extend(block.as_chunks::<2>().0.iter().map(|&[low, high]| i16::from_le_bytes([low, high ^ flip]))),
        }
        let mut msg = begin(block.len());
        self.codec.encode(&self.samples, &mut msg)?;
        Ok(finish(msg))
    }
}

/// One stream's encoder, in the codec its client asked for.
pub enum AudioEncoder {
    Flac(FlacEncoder),
    Opus(OpusEncoder),
}

impl AudioEncoder {
    /// An encoder of `codec` for `format`; `bitrate`, in bits per second, is
    /// the rate Opus starts at, which it has to have been set, and nothing to
    /// FLAC.
    pub fn new(codec: Codec, format: AudioFormat, bitrate: Option<u32>) -> Result<Self, AudioEncodeError> {
        Ok(match codec {
            Codec::Flac => Self::Flac(FlacEncoder::new(format)?),
            Codec::Opus => Self::Opus(OpusEncoder::new(format, bitrate.ok_or(AudioEncodeError::NoBitrate)?)?),
        })
    }

    /// A message for every frame `samples` completed: see [`FlacEncoder::push`]
    /// and [`OpusEncoder::push`].
    pub fn push(&mut self, samples: &[u8]) -> Result<Vec<Vec<u8>>, AudioEncodeError> {
        match self {
            Self::Flac(encoder) => encoder.push(samples),
            Self::Opus(encoder) => encoder.push(samples),
        }
    }

    /// Move the rate Opus is coded at; FLAC has none.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), AudioEncodeError> {
        match self {
            Self::Flac(_) => Ok(()),
            Self::Opus(encoder) => encoder.set_bitrate(bitrate),
        }
    }
}

/// The FLAC stream header a client builds from the format it set, as the FLAC
/// specification lays out `STREAMINFO`'s 34 bytes: nothing in it comes from the
/// server. The block size is [`AudioFormat::block_frames`] at both ends, and
/// the frame sizes, the total and the MD5 are unknown. A format
/// [`AudioFormat::check`] refuses has no header.
#[cfg(test)]
fn streaminfo(format: AudioFormat) -> Result<[u8; 34], AudioParseError> {
    format.check()?;
    let block = (format.block_frames() as u16).to_be_bytes();
    let mut info = [0u8; 34];
    info[0..2].copy_from_slice(&block);
    info[2..4].copy_from_slice(&block);
    // Rate (20 bits), channels - 1 (3), bits per sample - 1 (5), total samples
    // (36, unknown).
    let bits = 8 * format.sample.bytes() as u64;
    let packed = (u64::from(format.frequency) << 44) | (u64::from(format.channels - 1) << 41) | ((bits - 1) << 36);
    info[10..18].copy_from_slice(&packed.to_be_bytes());
    Ok(info)
}

/// Why a FLAC frame could not be read back.
#[cfg(test)]
#[derive(Debug, Error, PartialEq, Eq)]
enum AudioDecodeError {
    /// The format is not one the extension carries.
    #[error(transparent)]
    Unsupported(#[from] AudioParseError),
    /// symphonia's own error is carried as its message.
    #[error("FLAC cannot carry this format: {0}")]
    Format(String),
    #[error("decoding a FLAC frame: {0}")]
    Decode(String),
    #[error("a FLAC frame of {got} frames of {channels} channels, where {want} of {expected} were agreed")]
    Shape { got: usize, channels: usize, want: usize, expected: usize },
}

/// One stream's decoder, the client's end of [`FlacEncoder`]: the frame a
/// [`wlshare_proto::audio::MSG_AUDIO_FRAME`] message carries in, interleaved little-endian samples in
/// the client's format out. Made at a begin, from the format that was set.
///
/// Every frame decodes on its own, so a frame that fails costs its own twenty
/// milliseconds and the decoder goes on with the next.
#[cfg(test)]
struct FlacDecoder {
    format: AudioFormat,
    decoder: symphonia_bundle_flac::FlacDecoder,
    samples: Vec<i32>,
}

#[cfg(test)]
impl FlacDecoder {
    fn new(format: AudioFormat) -> Result<Self, AudioDecodeError> {
        use symphonia_core::codecs::audio::well_known::CODEC_ID_FLAC;
        use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};

        let info = streaminfo(format)?;
        let mut params = AudioCodecParameters::new();
        params.for_codec(CODEC_ID_FLAC).with_extra_data(Box::new(info));
        let decoder = symphonia_bundle_flac::FlacDecoder::try_new(&params, &AudioDecoderOptions::default())
            .map_err(|e| AudioDecodeError::Format(e.to_string()))?;
        Ok(Self { format, decoder, samples: Vec::new() })
    }

    /// One frame — the bytes after a message's header — as
    /// [`AudioFormat::block_frames`] frames of interleaved little-endian
    /// samples in the format the stream was set to, bit for bit what the
    /// server captured.
    fn decode(&mut self, frame: &[u8]) -> Result<Vec<u8>, AudioDecodeError> {
        use symphonia_core::codecs::audio::AudioDecoder as _;
        use symphonia_core::packet::Packet;
        use symphonia_core::units::{Duration, Timestamp};

        let (block, channels) = (self.format.block_frames(), usize::from(self.format.channels));
        let packet = Packet::new(0, Timestamp::new(0), Duration::new(block as u64), frame.to_vec());
        let decoded = self.decoder.decode(&packet).map_err(|e| AudioDecodeError::Decode(e.to_string()))?;
        let got = (decoded.frames(), decoded.spec().channels().count());
        if got != (block, channels) {
            return Err(AudioDecodeError::Shape { got: got.0, channels: got.1, want: block, expected: channels });
        }
        self.samples.clear();
        decoded.copy_to_vec_interleaved(&mut self.samples);
        let width = self.format.sample.bytes();
        let mut out = Vec::with_capacity(self.samples.len() * width);
        for &sample in &self.samples {
            // The decoder scales to 32 bits; back down to the format's width.
            let mut bytes = (sample >> (32 - 8 * width)).to_le_bytes();
            if self.format.sample.unsigned() {
                bytes[width - 1] ^= 0x80;
            }
            out.extend_from_slice(&bytes[..width]);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rate the tests code Opus at: the one a client names on a stream
    /// with no rate of its own.
    const BITRATE: u32 = sound_opus::BITRATE_DEFAULT;

    /// An Opus stream starts at the rate its client set and at none of its
    /// own; FLAC needs none.
    #[test]
    fn an_opus_stream_needs_a_bitrate_and_flac_does_not() {
        let format = AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 };
        assert!(matches!(AudioEncoder::new(Codec::Opus, format, None), Err(AudioEncodeError::NoBitrate)));
        assert!(AudioEncoder::new(Codec::Opus, format, Some(BITRATE)).is_ok());
        assert!(AudioEncoder::new(Codec::Flac, format, None).is_ok());
    }

    /// Decode a run of frame messages with [`FlacDecoder`] — symphonia's
    /// decoder, which shares nothing with `sound-flac` — checking each message's
    /// framing on the way.
    fn decode(format: AudioFormat, messages: &[Vec<u8>]) -> Vec<u8> {
        let mut decoder = FlacDecoder::new(format).unwrap();
        let mut out = Vec::new();
        for msg in messages {
            assert_eq!(msg[0], 0xE4);
            assert_eq!(&msg[1..4], &[0, 0, 0]);
            let len = u32::from_be_bytes([msg[4], msg[5], msg[6], msg[7]]) as usize;
            assert_eq!(msg.len(), 8 + len);
            assert!(len <= MAX_AUDIO_FRAME);
            out.extend(decoder.decode(&msg[8..]).unwrap());
        }
        out
    }

    /// The header's fields where the FLAC specification puts them, read back
    /// bit by bit rather than with the packing that wrote them.
    #[test]
    fn the_streaminfo_has_the_specified_layout() {
        let info = streaminfo(AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 }).unwrap();
        assert_eq!(&info[0..4], &[0x03, 0xC0, 0x03, 0xC0], "960 at both ends");
        assert_eq!(&info[4..10], &[0; 6], "frame sizes unknown");
        // 48000 = 0x0BB80 in 20 bits, then 001 for two channels, then 01111 for
        // 16 bits, then a total of zero.
        assert_eq!(&info[10..14], &[0x0B, 0xB8, 0x02, 0xF0]);
        assert_eq!(&info[14..34], &[0; 20]);
    }

    /// A format outside the extension's has no header, encoder or decoder:
    /// no channels would underflow the header's field, and a rate past 20 bits
    /// would spill into the ones after it.
    #[test]
    fn a_format_the_extension_does_not_carry_is_refused_everywhere() {
        let good = AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 };
        for (format, error) in [
            (AudioFormat { channels: 0, ..good }, AudioParseError::BadChannels(0)),
            (AudioFormat { channels: 3, ..good }, AudioParseError::BadChannels(3)),
            (AudioFormat { frequency: MIN_FREQUENCY - 1, ..good }, AudioParseError::FrequencyTooLow(MIN_FREQUENCY - 1)),
            (AudioFormat { frequency: 1 << 20, ..good }, AudioParseError::FrequencyTooHigh(1 << 20)),
        ] {
            assert_eq!(streaminfo(format), Err(error.clone()));
            assert!(matches!(FlacEncoder::new(format), Err(AudioEncodeError::Unsupported(e)) if e == error));
            assert_eq!(FlacDecoder::new(format).err(), Some(AudioDecodeError::Unsupported(error)));
        }
    }

    /// A frame that is not one, and a frame of the wrong length for the format
    /// that was set, are errors rather than samples: the client drops that
    /// frame and goes on with the next.
    #[test]
    fn a_frame_that_does_not_decode_to_the_agreed_block_is_refused() {
        let format = AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 };
        let mut decoder = FlacDecoder::new(format).unwrap();
        assert!(matches!(decoder.decode(&[0xFF, 0xF8, 1, 2, 3]), Err(AudioDecodeError::Decode(_))));

        let other = AudioFormat { frequency: 44_100, ..format };
        let messages = FlacEncoder::new(other).unwrap().push(&signal(other, other.block_frames())).unwrap();
        assert!(decoder.decode(&messages[0][8..]).is_err(), "882 frames where 960 were agreed");

        // And the decoder is still good for a frame that is right.
        let pcm = signal(format, format.block_frames());
        let messages = FlacEncoder::new(format).unwrap().push(&pcm).unwrap();
        assert_eq!(decoder.decode(&messages[0][8..]).unwrap(), pcm);
    }

    /// A tone with noise on it, in whatever format: every byte a sample of it,
    /// so the full range of each width is exercised.
    fn signal(format: AudioFormat, frames: usize) -> Vec<u8> {
        let mut seed = 0x2545_f491_u32;
        let mut out = Vec::with_capacity(frames * format.frame_bytes());
        for n in 0..frames {
            for c in 0..format.channels {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let t = n as f64 / f64::from(format.frequency);
                let tone = (t * 440.0 * (1.0 + f64::from(c)) * std::f64::consts::TAU).sin() * 0.7;
                let noise = (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 0.2;
                let full = ((tone + noise).clamp(-1.0, 1.0) * f64::from(i32::MAX)) as i32;
                let width = format.sample.bytes();
                let mut bytes = (full >> (32 - 8 * width)).to_le_bytes();
                if format.sample.unsigned() {
                    bytes[width - 1] ^= 0x80;
                }
                out.extend_from_slice(&bytes[..width]);
            }
        }
        out
    }

    /// Every format, channel count and a spread of rates decodes to exactly
    /// the samples that went in, fed in buffers that do not line up with the
    /// frames. 70001 Hz is a rate no frame header can state, so its frames
    /// leave it to the header the client builds.
    #[test]
    fn every_format_round_trips_bit_for_bit() {
        for sample in [SampleFormat::U8, SampleFormat::S8, SampleFormat::U16, SampleFormat::S16] {
            for channels in [1, 2] {
                for frequency in [MIN_FREQUENCY, 11_025, 44_100, 48_000, 70_001, MAX_FREQUENCY] {
                    let format = AudioFormat { sample, channels, frequency };
                    let blocks = 5;
                    let pcm = signal(format, blocks * format.block_frames());
                    let mut encoder = FlacEncoder::new(format).unwrap();
                    let mut messages = Vec::new();
                    // Uneven buffers of whole frames: short ones first, as
                    // PipeWire's are, then longer than a block.
                    let mut rest = &pcm[..];
                    for frames in [7, 100, format.block_frames() * 2 + 3].iter().cycle() {
                        if rest.is_empty() {
                            break;
                        }
                        let take = (frames * format.frame_bytes()).min(rest.len());
                        messages.extend(encoder.push(&rest[..take]).unwrap());
                        rest = &rest[take..];
                    }
                    assert_eq!(messages.len(), blocks, "{format:?}");
                    assert!(decode(format, &messages) == pcm, "{format:?} did not survive");
                }
            }
        }
    }

    /// What does not fill a frame waits for the samples that do, and nothing is
    /// sent for it until then.
    #[test]
    fn a_partial_frame_waits_for_the_rest() {
        let format = AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 };
        let pcm = signal(format, 960);
        let mut encoder = FlacEncoder::new(format).unwrap();
        assert!(encoder.push(&pcm[..959 * 4]).unwrap().is_empty());
        let messages = encoder.push(&pcm[959 * 4..]).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(decode(format, &messages), pcm);
    }

    /// Silence, the state a desktop is in most of the time, is a few bytes a
    /// frame rather than the 3840 of its samples.
    #[test]
    fn silence_is_a_few_bytes() {
        for sample in [SampleFormat::U8, SampleFormat::S16] {
            let format = AudioFormat { sample, channels: 2, frequency: 48_000 };
            let silence: Vec<u8> = match sample {
                SampleFormat::U8 => vec![0x80; 960 * 2],
                _ => vec![0; 960 * 4],
            };
            let mut encoder = FlacEncoder::new(format).unwrap();
            let messages = encoder.push(&silence).unwrap();
            assert_eq!(messages.len(), 1);
            assert!(messages[0].len() < 32, "{sample:?}: {} bytes", messages[0].len());
            assert_eq!(decode(format, &messages), silence);
        }
    }

    /// The bounds a set-bitrate is parsed within are the encoder's own.
    #[test]
    fn the_bitrate_bounds_are_libopus_bounds() {
        use wlshare_proto::audio::{OPUS_BITRATE_MAX, OPUS_BITRATE_MIN};
        assert_eq!((OPUS_BITRATE_MIN, OPUS_BITRATE_MAX), (sound_opus::BITRATE_MIN, sound_opus::BITRATE_MAX));
        assert!(check_bitrate(BITRATE).is_ok());
    }

    /// One Ogg page holding `packet` whole, as RFC 3533 lays a page out, its
    /// checksum the format's own CRC-32: unreflected, from zero.
    fn ogg_page(kind: u8, granule: u64, sequence: u32, packet: &[u8]) -> Vec<u8> {
        let mut page = Vec::from(*b"OggS");
        page.extend_from_slice(&[0, kind]);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&1u32.to_le_bytes());
        page.extend_from_slice(&sequence.to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        // A packet is a run of 255-byte segments and one shorter, which ends it.
        let full = packet.len() / 255;
        page.push(full as u8 + 1);
        page.extend(std::iter::repeat_n(255, full));
        page.push((packet.len() % 255) as u8);
        page.extend_from_slice(packet);
        let mut crc = 0u32;
        for &byte in &page {
            crc ^= u32::from(byte) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04c1_1db7 } else { crc << 1 };
            }
        }
        page[22..26].copy_from_slice(&crc.to_le_bytes());
        page
    }

    /// Decode a run of Opus frame messages as a client would have them
    /// decoded: behind the `OpusHead` it builds from the format it set, as
    /// RFC 7845 lays one out, by FFmpeg's own Opus decoder — which shares
    /// nothing with libopus, and must be on the path as `ffmpeg` — checking
    /// each message's framing on the way. The samples come back signed 16-bit
    /// at 48 kHz in the format's channels; the head states no pre-skip, so
    /// that every packet's twenty milliseconds are among them.
    fn decode_opus(format: AudioFormat, messages: &[Vec<u8>]) -> Vec<i16> {
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        let mut head = Vec::from(*b"OpusHead");
        head.extend_from_slice(&[1, format.channels, 0, 0]);
        head.extend_from_slice(&format.frequency.to_le_bytes());
        head.extend_from_slice(&[0, 0, 0]);
        let mut file = ogg_page(2, 0, 0, &head);
        file.extend(ogg_page(0, 0, 1, b"OpusTags\x04\0\0\0test\0\0\0\0"));
        for (n, msg) in messages.iter().enumerate() {
            assert_eq!(msg[0], 0xE4);
            assert_eq!(&msg[1..4], &[0, 0, 0]);
            let len = u32::from_be_bytes([msg[4], msg[5], msg[6], msg[7]]) as usize;
            assert_eq!(msg.len(), 8 + len);
            assert!(len > 0 && len <= sound_opus::MAX_PACKET);
            let last = if n + 1 == messages.len() { 4 } else { 0 };
            file.extend(ogg_page(last, (n as u64 + 1) * 960, n as u32 + 2, &msg[8..]));
        }
        let channels = format.channels.to_string();
        let mut ffmpeg = Command::new("ffmpeg")
            // `-c:a opus` before the input names FFmpeg's decoder rather than
            // its wrapper around libopus.
            .args(["-v", "error", "-c:a", "opus", "-i", "pipe:0", "-f", "s16le", "-ar", "48000", "-ac", &channels, "pipe:1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("the Opus tests decode with ffmpeg, which must be installed");
        let mut stdin = ffmpeg.stdin.take().unwrap();
        let writer = std::thread::spawn(move || stdin.write_all(&file));
        let output = ffmpeg.wait_with_output().unwrap();
        writer.join().unwrap().unwrap();
        assert!(output.status.success(), "ffmpeg refused the stream");
        let decoded: Vec<i16> = output.stdout.as_chunks::<2>().0.iter().map(|&pair| i16::from_le_bytes(pair)).collect();
        assert_eq!(decoded.len(), messages.len() * 960 * usize::from(format.channels), "every packet is twenty milliseconds");
        decoded
    }

    /// A tone on each channel, an octave apart, at half scale in whatever
    /// format: Opus is lossy, so what survives is the tone and where it is.
    fn tone(format: AudioFormat, frames: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(frames * format.frame_bytes());
        for n in 0..frames {
            for c in 0..format.channels {
                let t = n as f64 / f64::from(format.frequency);
                let full = ((t * 440.0 * (1.0 + f64::from(c)) * std::f64::consts::TAU).sin() * 0.5 * f64::from(i32::MAX)) as i32;
                let width = format.sample.bytes();
                let mut bytes = (full >> (32 - 8 * width)).to_le_bytes();
                if format.sample.unsigned() {
                    bytes[width - 1] ^= 0x80;
                }
                out.extend_from_slice(&bytes[..width]);
            }
        }
        out
    }

    /// The mean square of every `step`th sample from `from`, past the first
    /// fifth, where the encoder is still settling.
    fn energy(samples: &[i16], from: usize, step: usize) -> f64 {
        let settled = &samples[samples.len() / 5..];
        let picked: Vec<f64> = settled.iter().skip(from).step_by(step).map(|&s| f64::from(s)).collect();
        picked.iter().map(|s| s * s).sum::<f64>() / picked.len() as f64
    }

    /// Every sample format and channel count, at every rate Opus codes at,
    /// comes back from an independent decoder as the half-scale tone that went
    /// in, fed in buffers that do not line up with the packets: a wrong flip
    /// or width would be noise or a different level.
    #[test]
    fn every_format_survives_opus() {
        // A sine at half of full scale.
        let expected = (0.5 * 32_768.0_f64).powi(2) / 2.0;
        for sample in [SampleFormat::U8, SampleFormat::S8, SampleFormat::U16, SampleFormat::S16] {
            for channels in [1, 2] {
                for frequency in sound_opus::RATES {
                    let format = AudioFormat { sample, channels, frequency };
                    let blocks = 25;
                    let pcm = tone(format, blocks * format.block_frames());
                    let mut encoder = OpusEncoder::new(format, BITRATE).unwrap();
                    let mut messages = Vec::new();
                    let mut rest = &pcm[..];
                    for frames in [7, 100, format.block_frames() * 2 + 3].iter().cycle() {
                        if rest.is_empty() {
                            break;
                        }
                        let take = (frames * format.frame_bytes()).min(rest.len());
                        messages.extend(encoder.push(&rest[..take]).unwrap());
                        rest = &rest[take..];
                    }
                    assert_eq!(messages.len(), blocks, "{format:?}");
                    let decoded = decode_opus(format, &messages);
                    for channel in 0..usize::from(channels) {
                        let ratio = energy(&decoded, channel, usize::from(channels)) / expected;
                        assert!((0.6..1.6).contains(&ratio), "{format:?} channel {channel}: {ratio}");
                    }
                }
            }
        }
    }

    /// A sound on the left alone is on the left alone after the trip.
    #[test]
    fn opus_keeps_the_channels_apart() {
        let format = AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 };
        let mut pcm = tone(format, 25 * 960);
        pcm.as_chunks_mut::<4>().0.iter_mut().for_each(|frame| frame[2..].fill(0));
        let messages = OpusEncoder::new(format, BITRATE).unwrap().push(&pcm).unwrap();
        let decoded = decode_opus(format, &messages);
        let (left, right) = (energy(&decoded, 0, 2), energy(&decoded, 1, 2));
        assert!(left > 1_000_000.0 && right * 10.0 < left, "{left} against {right}");
    }

    /// A frequency Opus does not code at, and a rate outside libopus's, have
    /// no Opus encoder; FLAC carries the same format.
    #[test]
    fn opus_refuses_a_frequency_or_a_rate_it_does_not_code_at() {
        let good = AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 };
        for frequency in [44_100, 11_025, MAX_FREQUENCY] {
            let format = AudioFormat { frequency, ..good };
            assert!(matches!(OpusEncoder::new(format, BITRATE), Err(AudioEncodeError::Opus(sound_opus::Error::Unsupported(_)))));
            assert!(matches!(AudioEncoder::new(Codec::Opus, format, Some(BITRATE)), Err(AudioEncodeError::Opus(_))));
            assert!(AudioEncoder::new(Codec::Flac, format, None).is_ok());
        }
        assert!(matches!(OpusEncoder::new(AudioFormat { channels: 3, ..good }, BITRATE), Err(AudioEncodeError::Unsupported(AudioParseError::BadChannels(3)))));
        assert!(matches!(OpusEncoder::new(good, 1), Err(AudioEncodeError::Unsupported(AudioParseError::BadBitrate(1)))));
        let mut encoder = OpusEncoder::new(good, BITRATE).unwrap();
        assert!(matches!(encoder.set_bitrate(u32::MAX), Err(AudioEncodeError::Unsupported(AudioParseError::BadBitrate(u32::MAX)))));
    }

    /// What does not fill a packet waits, as for FLAC, and silence is a few
    /// bytes a packet.
    #[test]
    fn an_opus_packet_waits_for_its_block_and_silence_is_small() {
        let format = AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 };
        let mut encoder = OpusEncoder::new(format, BITRATE).unwrap();
        assert!(encoder.push(&vec![0; 959 * 4]).unwrap().is_empty());
        assert_eq!(encoder.push(&[0; 4]).unwrap().len(), 1);
        let messages = encoder.push(&vec![0; 10 * 960 * 4]).unwrap();
        assert_eq!(messages.len(), 10);
        assert!(messages.iter().skip(2).all(|msg| msg.len() < 8 + 16), "{:?}", messages.iter().map(Vec::len).collect::<Vec<_>>());
    }

    /// The rate moves on a running stream, through the encoder a capture
    /// holds, and the decoder reads across the change; FLAC takes the message
    /// and has nothing to move.
    #[test]
    fn the_opus_rate_moves_on_a_running_stream() {
        let format = AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 };
        let pcm = tone(format, 20 * 960);
        let mut encoder = AudioEncoder::new(Codec::Opus, format, Some(96_000)).unwrap();
        let before = encoder.push(&pcm).unwrap();
        encoder.set_bitrate(16_000).unwrap();
        let after = encoder.push(&pcm).unwrap();
        let average = |messages: &[Vec<u8>]| messages.iter().map(Vec::len).sum::<usize>() / messages.len();
        assert!(average(&after) * 2 < average(&before), "{} against {}", average(&after), average(&before));
        let all: Vec<Vec<u8>> = before.into_iter().chain(after).collect();
        assert!(energy(&decode_opus(format, &all), 0, 2) > 1_000_000.0);

        let mut flac = AudioEncoder::new(Codec::Flac, format, None).unwrap();
        flac.set_bitrate(16_000).unwrap();
        assert_eq!(decode(format, &flac.push(&pcm[..960 * 4]).unwrap()), &pcm[..960 * 4]);
    }

    /// The pre-skip a client's `OpusHead` states is the encoder's own.
    #[test]
    fn the_pre_skip_is_the_encoders_lookahead() {
        assert_eq!(sound_opus::PRE_SKIP, 312);
        for rate in sound_opus::RATES {
            let mut encoder = sound_opus::Encoder::new(sound_opus::Stream { rate, channels: 2 }, BITRATE).unwrap();
            assert_eq!(encoder.pre_skip().unwrap(), sound_opus::PRE_SKIP);
        }
    }
}
