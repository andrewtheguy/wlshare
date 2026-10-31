use super::*;

#[test]
fn the_version_is_the_crates_major_version() {
    assert_eq!(env!("CARGO_PKG_VERSION"), format!("{PROTOCOL_VERSION}.0.0"));
}

#[test]
fn a_version_is_wlv_and_its_byte() {
    assert_eq!(ENCODING_VERSION_BASE, i32::from_be_bytes(*b"WLV\0"));
    assert_eq!(encoding(7), i32::from_be_bytes(*b"WLV\x07"));
    assert_eq!(of(encoding(255)), Some(255));
    assert_eq!(of(ENCODING_VERSION_BASE - 1), None);
    assert_eq!(of(ENCODING_VERSION_BASE + 256), None);
}

#[test]
fn a_list_with_anything_private_carries_one_version() {
    assert_eq!(listed(&[16, -239]), Ok(None), "a plain client's list");
    assert_eq!(listed(&[16, crate::ENCODING_DENSITY, encoding(3)]), Ok(Some(3)));
    assert_eq!(listed(&[encoding(3), 16, encoding(3)]), Err(VersionListError::Several(3, 3)), "one version, listed once");
    assert_eq!(listed(&[16, crate::ENCODING_DENSITY]), Err(VersionListError::Missing));
    assert_eq!(listed(&[crate::ENCODING_DENSITY, encoding(3), encoding(4)]), Err(VersionListError::Several(3, 4)));
}

#[test]
fn the_answer_has_the_documented_layout() {
    assert_eq!(answer(2), [0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0x57, 0x4c, 0x56, 2]);
    assert_eq!(parse_answer(&answer(2)), Ok(2));
}

#[test]
fn anything_else_first_is_no_answer() {
    let mut two = answer(2);
    two[3] = 2;
    assert_eq!(parse_answer(&two), Err(VersionAnswerError::NotAnswer));
    let mut sized = answer(2);
    sized[9] = 1;
    assert_eq!(parse_answer(&sized), Err(VersionAnswerError::NotAnswer));
    // wlshare's density answer, as a server before versions sent it first.
    let mut density = [0u8; ANSWER_LEN];
    density[..10].copy_from_slice(&crate::density::output_scale(1280, 800, 1.0));
    assert_eq!(parse_answer(&density), Err(VersionAnswerError::NotAnswer));
}
