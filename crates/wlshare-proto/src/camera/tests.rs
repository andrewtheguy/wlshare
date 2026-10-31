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

/// The client's messages, read back by the server's parser, whose layouts
/// the tests above hold to the documented ones.
#[test]
fn the_client_messages_parse_back() {
    assert_eq!(plug(VGA), [0xE2, 0, 0, 0, 0x02, 0x80, 0x01, 0xE0, 0, 0, 0x75, 0x30, 0, 0, 0x03, 0xE9]);
    assert_eq!(parse_client(&plug(VGA)), Ok(Some((ClientCamera::Plug(VGA), 16))));
    assert_eq!(parse_client(&unplug()), Ok(Some((ClientCamera::Unplug, 4))));
    assert_eq!(sample(true, &[0, 0, 1]), [0xE2, 2, SAMPLE_KEYFRAME, 0, 0, 0, 0, 3, 0, 0, 1]);
    assert_eq!(parse_client(&sample(false, &[9])), Ok(Some((ClientCamera::Sample { keyframe: false, data: vec![9] }, 9))));
}

/// The server's messages, read back by the client's parser, against the
/// decoder written from the layout.
#[test]
fn the_server_messages_parse_back() {
    for (bytes, expected) in [
        (camera_available().to_vec(), ServerCamera::Available),
        (camera_start(VGA).to_vec(), ServerCamera::Start(VGA)),
        (camera_stop().to_vec(), ServerCamera::Stop),
        (camera_keyframe().to_vec(), ServerCamera::Keyframe),
    ] {
        assert_eq!(parse_server(&bytes), Ok(Some((expected, bytes.len()))));
        assert_eq!(server_len(bytes.first_chunk().unwrap()), Ok(bytes.len()));
        assert_eq!(parse_server(&bytes[..bytes.len() - 1]), Ok(None), "a partial message asks for more");
    }
    assert_eq!(parse_server(&[0xE2, 4, 0, 0]), Err(CameraParseError::UnknownOperation(4)));
}
