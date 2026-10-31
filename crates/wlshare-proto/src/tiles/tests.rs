use super::*;

#[test]
fn the_list_names_one_picture_and_whether_it_walks() {
    let listed = TilesPicture::listed;
    let lossless = crate::ENCODING_LOSSLESS;
    assert_eq!(listed(&[crate::ENCODING_TILES, 16]), Ok(Some(TilesPicture::Progressive)));
    assert_eq!(listed(&[crate::ENCODING_TILES, ENCODING_HELD]), Ok(Some(TilesPicture::Held)));
    assert_eq!(listed(&[16, lossless, crate::ENCODING_TILES]), Ok(Some(TilesPicture::Lossless)));
    assert_eq!(listed(&[crate::ENCODING_TILES, crate::ENCODING_VIDEO]), Err(TilesListError::Both));
    assert_eq!(listed(&[crate::ENCODING_TILES, ENCODING_HELD, lossless]), Err(TilesListError::HeldAndLossless));
    assert_eq!(listed(&[16, ENCODING_HELD]), Ok(None));
    assert_eq!(listed(&[16, lossless]), Ok(None));
    assert_eq!(TilesPicture::Progressive.encodings(), [crate::ENCODING_TILES]);
    assert_eq!(TilesPicture::Held.encodings(), [crate::ENCODING_TILES, ENCODING_HELD]);
    assert_eq!(TilesPicture::Lossless.encodings(), [crate::ENCODING_TILES, lossless]);
    for picture in [TilesPicture::Progressive, TilesPicture::Held, TilesPicture::Lossless] {
        assert_eq!(listed(&picture.encodings()), Ok(Some(picture)));
    }
}

#[test]
fn a_cache_empty_is_its_type_alone() {
    assert_eq!(cache_empty(), [0xE6]);
    assert_eq!(parse_cache_empty(&[0xE6, 0xFF]), 1);
}

/// Each kind against the documented layout, and read back.
#[test]
fn each_header_has_the_documented_layout() {
    for (header, bytes) in [
        (TileHeader::Png { len: 0x0102 }, vec![0, 0, 0, 1, 2]),
        (TileHeader::Start { stream: 3, len: 7 }, vec![1, 3, 0, 0, 0, 7]),
        (TileHeader::Frame { stream: 3, len: 9 }, vec![2, 3, 0, 0, 0, 9]),
        (TileHeader::End { stream: 3 }, vec![3, 3]),
        (TileHeader::Copy { x: 0x0102, y: 0x0304 }, vec![4, 1, 2, 3, 4]),
        (TileHeader::Cached { slot: 0x0102 }, vec![5, 1, 2]),
        (TileHeader::Store { slot: 0x0FFF }, vec![6, 0x0F, 0xFF]),
        (TileHeader::Progressive { len: 0x0102 }, vec![7, 0, 0, 1, 2]),
        (TileHeader::Planar { len: 0x01_0203 }, vec![8, 0, 1, 2, 3]),
        (TileHeader::Still { len: 0x01_0203 }, vec![9, 0, 1, 2, 3]),
    ] {
        assert_eq!(header.to_bytes(), bytes);
        assert_eq!(TileHeader::len_of(bytes[0]), Ok(bytes.len()));
        assert_eq!(TileHeader::parse(&[&bytes[..], &[0xFF]].concat()), Ok(Some((header, bytes.len()))));
        assert_eq!(TileHeader::parse(&bytes[..bytes.len() - 1]), Ok(None), "a partial header asks for more");
    }
}

#[test]
fn a_length_filled_in_keeps_the_header_its_size() {
    for header in [TileHeader::Png { len: 0 }, TileHeader::Start { stream: 2, len: 0 }, TileHeader::Frame { stream: 2, len: 0 }, TileHeader::End { stream: 2 }, TileHeader::Copy { x: 1, y: 2 }, TileHeader::Cached { slot: 3 }, TileHeader::Store { slot: 4 }, TileHeader::Progressive { len: 0 }, TileHeader::Planar { len: 0 }, TileHeader::Still { len: 0 }] {
        let filled = header.with_len(9);
        assert_eq!(filled.to_bytes().len(), header.to_bytes().len());
        assert_eq!(filled.to_bytes()[0], header.to_bytes()[0], "the kind is kept");
    }
    assert_eq!(TileHeader::Frame { stream: 2, len: 0 }.with_len(9), TileHeader::Frame { stream: 2, len: 9 });
    assert_eq!(TileHeader::End { stream: 2 }.with_len(9), TileHeader::End { stream: 2 });
    assert_eq!(TileHeader::Copy { x: 1, y: 2 }.with_len(9), TileHeader::Copy { x: 1, y: 2 }, "a copy's source is no length");
    assert_eq!(TileHeader::Store { slot: 4 }.with_len(9), TileHeader::Store { slot: 4 }, "a slot is no length");
}

#[test]
fn a_bad_kind_length_or_slot_is_fatal() {
    assert_eq!(TileHeader::parse(&[10]), Err(TilesParseError::UnknownKind(10)));
    assert_eq!(TileHeader::parse(&[7, 0, 0, 0, 0]), Err(TilesParseError::Length(0)));
    assert_eq!(TileHeader::parse(&[8, 0, 0, 0, 0]), Err(TilesParseError::Length(0)));
    assert_eq!(TileHeader::parse(&[5, 0x10, 0]), Err(TilesParseError::Slot(4096)));
    assert_eq!(TileHeader::parse(&[6, 0xFF, 0xFF]), Err(TilesParseError::Slot(65535)));
    assert_eq!(TileHeader::parse(&[5, 0x0F, 0xFF, 9]), Ok(Some((TileHeader::Cached { slot: 4095 }, 3))), "the last slot");
    assert_eq!(TileHeader::parse(&[0, 0, 0, 0, 0]), Err(TilesParseError::Length(0)));

    let over = (crate::MAX_BODY as u32 + 1).to_be_bytes();
    assert_eq!(TileHeader::parse(&[2, 1, over[0], over[1], over[2], over[3]]), Err(TilesParseError::Length(crate::MAX_BODY + 1)));
}
