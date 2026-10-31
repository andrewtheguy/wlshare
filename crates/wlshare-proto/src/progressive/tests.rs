use super::*;

/// Every stage holds back no less than the one after it in every band,
/// as a decoder that upgrades from one to the next needs, and the last
/// holds back nothing.
#[test]
fn each_stage_holds_back_no_less_than_the_next() {
    let bands = |stage: &[u8; 16]| -> Vec<u8> { stage[1..].iter().flat_map(|b| [b >> 4, b & 0xF]).collect() };
    for pair in STAGES.windows(2) {
        assert!(pair[0][0] < pair[1][0], "labelled coarsest first");
        assert!(bands(&pair[0]).iter().zip(bands(&pair[1])).all(|(a, b)| *a >= b), "stage {} under {}", pair[0][0], pair[1][0]);
    }
    assert!(bands(&STAGES[STAGES.len() - 1]).iter().all(|b| *b == 0), "the full stage holds back nothing");
    assert!(QUANT.iter().flat_map(|b| [b >> 4, b & 0xF]).all(|q| (6..=15).contains(&q)), "RemoteFX's quantizers are 6 to 15");
}

/// A tile block's tile is read back; anything else is none.
#[test]
fn a_body_names_its_tile() {
    let mut first = vec![0, 1, 2];
    first.extend_from_slice(&7u16.to_le_bytes());
    first.extend_from_slice(&3u16.to_le_bytes());
    first.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let body = block(WBT_TILE_FIRST, &first);
    assert_eq!(tile_of(&body), Some((7, 3)));
    assert_eq!(tile_of(&block(WBT_TILE_UPGRADE, &first)), Some((7, 3)));
    assert_eq!(tile_of(&block(WBT_REGION, &first)), None, "no tile block");
    assert_eq!(tile_of(&body[..body.len() - 1]), None, "its length is the body's");
    assert_eq!(tile_of(&[]), None);
}

/// The stream's blocks, each its documented type and length, the region
/// carrying the tables and the bodies.
#[test]
fn a_stream_wraps_the_bodies_in_one_region() {
    let body = block(WBT_TILE_FIRST, &[0; 20]);
    let s = stream(9, &[(64, 0, 64, 64)], &[&body]);
    let mut at = 0;
    let mut kinds = Vec::new();
    while at < s.len() {
        let kind = u16::from_le_bytes([s[at], s[at + 1]]);
        let len = u32::from_le_bytes(s[at + 2..at + 6].try_into().unwrap()) as usize;
        kinds.push(kind);
        if kind == WBT_REGION {
            let region = &s[at + 6..at + len];
            assert_eq!((region[0], u16::from_le_bytes([region[1], region[2]]), region[3], region[4]), (TILE, 1, 3, 4));
            assert_eq!(&region[region.len() - body.len()..], &body[..], "the body last");
            assert_eq!(region.len(), 12 + 8 + 15 + 4 * 16 + body.len());
        }
        at += len;
    }
    assert_eq!(at, s.len());
    assert_eq!(kinds, [WBT_SYNC, WBT_FRAME_BEGIN, WBT_CONTEXT, WBT_REGION, WBT_FRAME_END]);
}
