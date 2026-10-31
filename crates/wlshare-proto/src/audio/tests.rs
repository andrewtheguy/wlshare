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

/// The client's builders, against the layouts the parser was tested on.
#[test]
fn the_client_messages_have_the_documented_layouts() {
    assert_eq!(enable(), [255, 1, 0, 0]);
    assert_eq!(disable(), [255, 1, 0, 1]);
    let format = AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 };
    assert_eq!(set_format(format), [255, 1, 0, 2, 3, 2, 0, 0, 0xBB, 0x80]);
    assert_eq!(set_bitrate(96_000), [255, 1, 0, 3, 0, 1, 0x77, 0]);
    assert_eq!(parse_client(&set_bitrate(96_000)), Ok(Some((ClientAudio::SetBitrate(96_000), 8))));
}

#[test]
fn a_bad_submessage_operation_or_format_is_fatal() {
    assert_eq!(parse_client(&[255, 2, 0, 0]), Err(AudioParseError::UnknownSubmessage(2)));
    assert_eq!(parse_client(&[255, 1, 0, 4]), Err(AudioParseError::UnknownOperation(4)));
    assert_eq!(parse_client(&[255, 1, 0, 2, 6, 2, 0, 0, 0xBB, 0x80]), Err(AudioParseError::UnknownSampleFormat(6)));
    assert_eq!(parse_client(&[255, 1, 0, 2, 3, 3, 0, 0, 0xBB, 0x80]), Err(AudioParseError::BadChannels(3)));
    assert_eq!(parse_client(&[255, 1, 0, 2, 4, 2, 0, 0, 0xBB, 0x80]), Err(AudioParseError::UnknownSampleFormat(4)));
    assert_eq!(parse_client(&[255, 1, 0, 2, 3, 1, 0, 0, 0, 0]), Err(AudioParseError::FrequencyTooLow(0)));
}

/// The frequency is bounded at both ends. A `u32::MAX` accepted here would
/// overflow the frame count a server's capture is asked for, and a rate
/// under the floor makes a frame shorter than FLAC's encoder takes, so both
/// bounds are parse rules rather than checks at the point of use.
#[test]
fn a_frequency_outside_the_bounds_is_fatal() {
    let set_at = |frequency: u32| {
        let f = frequency.to_be_bytes();
        parse_client(&[255, 1, 0, 2, 3, 2, f[0], f[1], f[2], f[3]])
    };
    assert!(matches!(set_at(MAX_FREQUENCY), Ok(Some((ClientAudio::SetFormat(_), _)))));
    assert_eq!(set_at(MAX_FREQUENCY + 1), Err(AudioParseError::FrequencyTooHigh(MAX_FREQUENCY + 1)));
    assert_eq!(set_at(u32::MAX), Err(AudioParseError::FrequencyTooHigh(u32::MAX)));
    assert!(matches!(set_at(MIN_FREQUENCY), Ok(Some((ClientAudio::SetFormat(_), _)))));
    assert_eq!(set_at(MIN_FREQUENCY - 1), Err(AudioParseError::FrequencyTooLow(MIN_FREQUENCY - 1)));
    // Every rate that survives leaves a frame count of twenty
    // milliseconds well inside a u32.
    assert!(MAX_FREQUENCY.checked_mul(20).is_some());
    for rate in [8_000, 44_100, 48_000] {
        assert!(matches!(set_at(rate), Ok(Some((ClientAudio::SetFormat(_), _)))));
    }
}

#[test]
fn the_bitrate_parses_within_libopus_bounds() {
    let set_at = |bitrate: u32| {
        let b = bitrate.to_be_bytes();
        parse_client(&[255, 1, 0, 3, b[0], b[1], b[2], b[3]])
    };
    assert_eq!(set_at(96_000), Ok(Some((ClientAudio::SetBitrate(96_000), 8))));
    assert_eq!(parse_client(&[255, 1, 0, 3, 0, 1, 0x77]), Ok(None));
    assert!(set_at(OPUS_BITRATE_MIN).is_ok() && set_at(OPUS_BITRATE_MAX).is_ok());
    for bitrate in [0, OPUS_BITRATE_MIN - 1, OPUS_BITRATE_MAX + 1, u32::MAX] {
        assert_eq!(set_at(bitrate), Err(AudioParseError::BadBitrate(bitrate)));
    }
}

#[test]
fn sample_widths_frame_sizes_and_blocks() {
    assert_eq!(SampleFormat::from_wire(0), Some(SampleFormat::U8));
    assert_eq!(SampleFormat::from_wire(3), Some(SampleFormat::S16));
    assert_eq!(SampleFormat::from_wire(4), None, "32-bit is wider than FLAC stores");
    assert_eq!(SampleFormat::from_wire(5), None);
    assert_eq!(AudioFormat::DEFAULT.frame_bytes(), 4);
    assert_eq!(AudioFormat::DEFAULT.block_frames(), 882);
    assert_eq!(AudioFormat { sample: SampleFormat::U8, channels: 1, frequency: 8000 }.frame_bytes(), 1);
    assert_eq!(AudioFormat { sample: SampleFormat::U8, channels: 1, frequency: MIN_FREQUENCY }.block_frames(), 160);
    assert_eq!(AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 }.block_frames(), 960);
}

/// The server messages, against a decoder written from the documented
/// layouts rather than from the builders, and read back by the client's
/// parsers.
#[test]
fn server_messages_have_the_documented_layouts() {
    assert_eq!(audio_rect(), [0, 0, 0, 0, 0, 0, 0, 0, b'W', b'L', b'S', b'F']);
    assert_eq!(audio_begin(), [255, 1, 0, 1]);
    assert_eq!(audio_end(), [255, 1, 0, 0]);
    assert_eq!(parse_server(&audio_begin()), Ok(Some((ServerAudio::Begin, 4))));
    assert_eq!(parse_server(&audio_end()), Ok(Some((ServerAudio::End, 4))));
    assert_eq!(parse_server(&[255, 1, 0]), Ok(None));
    assert_eq!(parse_server(&[255, 1, 0, 2]), Err(AudioParseError::UnknownOperation(2)), "QEMU's raw samples are never sent");
    assert_eq!(parse_server(&[255, 0, 0, 1]), Err(AudioParseError::UnknownSubmessage(0)));
    assert_eq!(frame_header(0x0102), [0xE4, 0, 0, 0, 0, 0, 1, 2]);
    assert_eq!(frame_len(&frame_header(0x0102)), Ok(0x0102));
    assert_eq!(frame_len(&frame_header(MAX_AUDIO_FRAME as u32 + 1)), Err(AudioParseError::FrameTooLong(MAX_AUDIO_FRAME + 1)));
}

/// The codec is FLAC unless the list that asks for the sound says Opus.
#[test]
fn the_codec_is_the_one_the_encodings_list() {
    assert_eq!(Codec::listed(&[16, crate::ENCODING_AUDIO]), Codec::Flac);
    assert_eq!(Codec::listed(&[crate::ENCODING_AUDIO, crate::ENCODING_AUDIO_OPUS]), Codec::Opus);
    assert_eq!(Codec::listed(&[]), Codec::Flac);
}
