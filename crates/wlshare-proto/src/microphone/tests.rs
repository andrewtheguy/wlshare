use super::*;

const MONO_48K: MicrophoneFormat = MicrophoneFormat { channels: 1, frequency: 48_000 };

/// A server message as the documented layouts describe it, decoded without
/// the builders.
#[derive(Debug, PartialEq, Eq)]
enum Decoded {
    Available,
    Start(MicrophoneFormat),
    Stop,
}

fn decode(bytes: &[u8]) -> Decoded {
    assert_eq!(bytes[0], 0xE3);
    assert_eq!(&bytes[2..4], &[0, 0], "the padding is zero");
    let decoded = match bytes[1] {
        0 => Decoded::Available,
        1 => {
            assert_eq!(&bytes[6..8], &[0, 0], "the padding is zero");
            Decoded::Start(MicrophoneFormat {
                channels: u16::from_be_bytes([bytes[4], bytes[5]]),
                frequency: u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            })
        }
        2 => Decoded::Stop,
        other => panic!("operation {other} is not a server message"),
    };
    let len = if matches!(decoded, Decoded::Start(_)) { 12 } else { 4 };
    assert_eq!(bytes.len(), len, "the message is exactly its layout");
    decoded
}

#[test]
fn server_messages_decode_to_what_they_were_built_from() {
    assert_eq!(decode(&microphone_available()), Decoded::Available);
    assert_eq!(decode(&microphone_start(MONO_48K)), Decoded::Start(MONO_48K));
    let stereo = MicrophoneFormat { channels: 2, frequency: 44_100 };
    assert_eq!(decode(&microphone_start(stereo)), Decoded::Start(stereo));
    assert_eq!(decode(&microphone_stop()), Decoded::Stop);
}

#[test]
fn a_start_has_the_documented_layout() {
    assert_eq!(
        microphone_start(MONO_48K),
        [
            0xE3, 1, 0, 0, //
            0x00, 0x01, // one channel
            0x00, 0x00, //
            0x00, 0x00, 0xBB, 0x80, // 48000
        ]
    );
}

#[test]
fn frames_are_sixteen_bits_a_channel() {
    assert_eq!(MONO_48K.frame_bytes(), 2);
    assert_eq!(MicrophoneFormat { channels: 2, frequency: 16_000 }.frame_bytes(), 4);
}

#[test]
fn the_client_operations_parse() {
    assert_eq!(parse_client(&[0xE3, 0, 0, 0, 0xFF]), Ok(Some((ClientMicrophone::Plug, 4))));
    assert_eq!(parse_client(&[0xE3, 1, 0, 0, 0xFF]), Ok(Some((ClientMicrophone::Unplug, 4))));
    let sample = [0xE3, 2, 0, 0, 0, 0, 0, 4, 0x01, 0x00, 0xFF, 0x7F, 0xFF];
    assert_eq!(parse_client(&sample), Ok(Some((ClientMicrophone::Sample(vec![0x01, 0x00, 0xFF, 0x7F]), 12))));
    assert_eq!(parse_client(&[0xE3, 2, 0, 0, 0, 0, 0, 0]), Ok(Some((ClientMicrophone::Sample(Vec::new()), 8))));
}

#[test]
fn a_partial_message_asks_for_more() {
    assert_eq!(parse_client(&[0xE3, 0, 0]), Ok(None));
    assert_eq!(parse_client(&[0xE3, 2, 0, 0, 0, 0, 0]), Ok(None));
    assert_eq!(parse_client(&[0xE3, 2, 0, 0, 0, 0, 0, 4, 0, 0]), Ok(None), "the samples are not all here");
}

#[test]
fn a_bad_operation_or_length_is_fatal() {
    assert_eq!(parse_client(&[0xE3, 3, 0, 0]), Err(MicrophoneParseError::UnknownOperation(3)));
    let over = (MAX_SAMPLE as u32 + 1).to_be_bytes();
    assert_eq!(
        parse_client(&[0xE3, 2, 0, 0, over[0], over[1], over[2], over[3]]),
        Err(MicrophoneParseError::SampleTooLong(MAX_SAMPLE + 1)),
        "refused before the samples are waited for"
    );
}

/// The client's messages, read back by the server's parser.
#[test]
fn the_client_messages_parse_back() {
    assert_eq!(parse_client(&plug()), Ok(Some((ClientMicrophone::Plug, 4))));
    assert_eq!(parse_client(&unplug()), Ok(Some((ClientMicrophone::Unplug, 4))));
    assert_eq!(sample(&[1, 0]), [0xE3, 2, 0, 0, 0, 0, 0, 2, 1, 0]);
    assert_eq!(parse_client(&sample(&[1, 0, 0xFF, 0x7F])), Ok(Some((ClientMicrophone::Sample(vec![1, 0, 0xFF, 0x7F]), 12))));
}

/// The server's messages, read back by the client's parser.
#[test]
fn the_server_messages_parse_back() {
    for (bytes, expected) in [
        (microphone_available().to_vec(), ServerMicrophone::Available),
        (microphone_start(MONO_48K).to_vec(), ServerMicrophone::Start(MONO_48K)),
        (microphone_stop().to_vec(), ServerMicrophone::Stop),
    ] {
        assert_eq!(parse_server(&bytes), Ok(Some((expected, bytes.len()))));
        assert_eq!(server_len(bytes.first_chunk().unwrap()), Ok(bytes.len()));
        assert_eq!(parse_server(&bytes[..bytes.len() - 1]), Ok(None), "a partial message asks for more");
    }
    assert_eq!(parse_server(&[0xE3, 3, 0, 0]), Err(MicrophoneParseError::UnknownOperation(3)));
}
