use super::*;

#[test]
fn the_list_names_the_streams_chroma() {
    let (video, full) = (crate::ENCODING_VIDEO, crate::ENCODING_VIDEO_444);
    assert_eq!(Chroma::listed(&[16, video]), Some(Chroma::Yuv420));
    assert_eq!(Chroma::listed(&[full, 16, video]), Some(Chroma::Yuv444));
    assert_eq!(Chroma::listed(&[16, full]), None, "no stream, no chroma");
    assert_eq!(Chroma::listed(&[16]), None);
    assert_eq!(Chroma::Yuv420.encodings(), [video]);
    assert_eq!(Chroma::Yuv444.encodings(), [video, full]);
    for chroma in [Chroma::Yuv420, Chroma::Yuv444] {
        assert_eq!(Chroma::listed(&chroma.encodings()), Some(chroma));
    }
}

#[test]
fn a_frame_header_is_the_length() {
    assert_eq!(frame_header(0x0102_0304), [1, 2, 3, 4]);
    assert_eq!(
        parse_frame_header(&frame_header(0x0102_0304)),
        Ok(0x0102_0304)
    );
    let over = crate::MAX_BODY as u32 + 1;
    assert_eq!(
        parse_frame_header(&frame_header(over)),
        Err(VideoFrameError::TooLong(over as usize))
    );
}
