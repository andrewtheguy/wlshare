use super::*;

/// A decoder written from the documented layout, not from [`output_list`]:
/// the most virtual displays, the shared id, and the entries.
fn decode(bytes: &[u8]) -> (u8, u32, Vec<OutputEntry>) {
    assert_eq!(bytes[0], 0xE1);
    let most = bytes[1];
    let count = u16::from_be_bytes([bytes[2], bytes[3]]);
    let active = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let mut at = 8;
    let mut outputs = Vec::new();
    for _ in 0..count {
        let e = &bytes[at..];
        let name_len = usize::from(e[13]);
        outputs.push(OutputEntry {
            id: u32::from_be_bytes([e[0], e[1], e[2], e[3]]),
            width: u16::from_be_bytes([e[4], e[5]]),
            height: u16::from_be_bytes([e[6], e[7]]),
            scale: f64::from(u32::from_be_bytes([e[8], e[9], e[10], e[11]])) / 65536.0,
            headless: e[12] & OUTPUT_HEADLESS != 0,
            name: String::from_utf8(e[14..14 + name_len].to_vec()).expect("UTF-8"),
        });
        at += 14 + name_len;
    }
    assert_eq!(at, bytes.len(), "the message is exactly its entries");
    (most, active, outputs)
}

fn entry(id: u32, name: &str, width: u16, height: u16, scale: f64, headless: bool) -> OutputEntry {
    OutputEntry { id, name: name.into(), width, height, scale, headless }
}

#[test]
fn a_list_decodes_to_what_it_was_built_from() {
    let outputs = vec![
        entry(3, "DP-2", 3840, 2160, 1.5, false),
        entry(7, "HEADLESS-1", 1728, 1117, 2.0, true),
    ];
    let (most, active, decoded) = decode(&output_list(7, 2, &outputs));
    assert_eq!(most, 2);
    assert_eq!(active, 7);
    assert_eq!(decoded, outputs);
}

#[test]
fn an_empty_list_is_a_header_alone() {
    let bytes = output_list(0, 0, &[]);
    assert_eq!(bytes, [0xE1, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(decode(&bytes), (0, 0, Vec::new()));
}

#[test]
fn a_list_has_the_documented_layout() {
    assert_eq!(
        output_list(1, 2, &[entry(1, "DP-2", 1920, 1080, 1.0, false)]),
        [
            0xE1, 2, 0x00, 0x01, // up to two virtual displays; one entry
            0x00, 0x00, 0x00, 0x01, // active: id 1
            0x00, 0x00, 0x00, 0x01, // id
            0x07, 0x80, // 1920
            0x04, 0x38, // 1080
            0x00, 0x01, 0x00, 0x00, // scale 1.0
            0x00, // flags
            0x04, b'D', b'P', b'-', b'2',
        ]
    );
}

#[test]
fn a_long_name_is_cut_at_a_character_boundary() {
    let outputs = vec![entry(1, &"é".repeat(200), 800, 600, 1.0, false)];
    let (_, _, decoded) = decode(&output_list(1, 0, &outputs));
    // 255 bytes is 127 two-byte characters and half of one, which is not sent.
    assert_eq!(decoded[0].name, "é".repeat(127));
}

/// Both client messages, read back by a decoder written from the layout.
#[test]
fn a_client_message_is_its_operation_and_one_u32() {
    assert_eq!(select_output(7), [0xE1, 0, 0, 0, 0, 0, 0, 7]);
    assert_eq!(virtual_displays(2), [0xE1, 1, 0, 0, 0, 0, 0, 2]);
    assert_eq!(parse_client(&select_output(u32::MAX)), Ok(Some((ClientOutputs::Select { id: u32::MAX }, 8))));
    assert_eq!(parse_client(&virtual_displays(1)), Ok(Some((ClientOutputs::VirtualDisplays { count: 1 }, 8))));
    assert_eq!(parse_client(&[0xE1, 1, 0, 0, 0, 0, 0]), Ok(None), "a short message asks for more");
    assert_eq!(parse_client(&[0xE1, 2, 0, 0, 0, 0, 0, 1]), Err(OutputsParseError::UnknownOperation(2)));
}

/// The client's reading of a list, against the builder.
#[test]
fn the_headers_read_back_what_the_list_was_built_from() {
    let bytes = output_list(7, 2, &[entry(7, "HEADLESS-1", 1728, 1117, 1.5, true)]);
    let header = ListHeader::parse(bytes[..LIST_HEADER_LEN].try_into().unwrap());
    assert_eq!(header, ListHeader { virtual_displays: 2, count: 1, active: 7 });
    let entry = EntryHeader::parse(bytes[LIST_HEADER_LEN..LIST_HEADER_LEN + ENTRY_HEADER_LEN].try_into().unwrap());
    assert_eq!(entry, EntryHeader { id: 7, width: 1728, height: 1117, fixed: 0x0001_8000, headless: true, name_len: 10 });
    assert_eq!(&bytes[LIST_HEADER_LEN + ENTRY_HEADER_LEN..], b"HEADLESS-1");
}
