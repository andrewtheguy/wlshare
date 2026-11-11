use super::*;

#[test]
fn the_encodings_are_their_ascii_names() {
    assert_eq!(ENCODING_VIDEO, i32::from_be_bytes(*b"WLSV"));
    assert_eq!(ENCODING_HELD, i32::from_be_bytes(*b"WLSD"));
    assert_eq!(ENCODING_LOSSLESS, i32::from_be_bytes(*b"WLSL"));
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
    for e in [
        ENCODING_VIDEO,
        ENCODING_TILES,
        ENCODING_DENSITY,
        ENCODING_OUTPUTS,
        ENCODING_CAMERA,
        ENCODING_MICROPHONE,
        ENCODING_AUDIO,
        ENCODING_AUDIO_OPUS,
        ENCODING_HELD,
        ENCODING_LOSSLESS,
        version::encoding(1),
    ] {
        assert!(private(e), "{e:#x}");
    }
    // RFB's own: ZRLE, Cursor, and the QEMU Audio pseudo-encoding.
    for e in [16, -239, -259] {
        assert!(!private(e), "{e}");
    }
}
