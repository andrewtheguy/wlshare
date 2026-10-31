use super::*;

#[test]
fn a_frame_header_is_the_length() {
    assert_eq!(frame_header(0x0102_0304), [1, 2, 3, 4]);
    assert_eq!(parse_frame_header(&frame_header(0x0102_0304)), Ok(0x0102_0304));
    let over = crate::MAX_BODY as u32 + 1;
    assert_eq!(parse_frame_header(&frame_header(over)), Err(VideoFrameError::TooLong(over as usize)));
}
