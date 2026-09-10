//! The outputs extension: how a client learns which outputs the compositor has
//! and asks for the one it wants shared.
//!
//! One framebuffer is one output ([`crate`]), so the client that wants the other
//! monitor has to say so. Standard RFB has no word for it: `ExtendedDesktopSize`
//! describes screens *inside* one framebuffer, which is not what these are. This
//! private extension adds one pseudo-encoding and one message type, in both
//! directions, the same shape the density extension has ([`crate::density`]):
//!
//! - The client lists [`crate::ENCODING_OUTPUTS`] in `SetEncodings`. A server
//!   that does not know it ignores it, as RFB requires.
//! - The server answers **every** such `SetEncodings` with an [`output_list`] —
//!   that answer is the only way support is ever announced — and sends another
//!   whenever the list, an entry, or the shared one changes.
//! - The client may send a `SelectOutput`
//!   ([`crate::msg::ClientMsg::SelectOutput`]) naming the output it wants shared.
//!   The server switches the capture to it and answers with an `OutputList`
//!   whose `active` says what actually happened: a request it cannot honour —
//!   an id it does not have, an output that went away — is answered with the
//!   list as it is, so the client's menu ends up agreeing with what is on the
//!   canvas rather than with what was clicked. A request is never left without
//!   an answer.
//!
//! The size and scale of each entry are what a capture of that output would
//! produce, so a client can label the choice before making it. Scales are the
//! density extension's 16.16 fixed point.

use crate::density::to_fixed;

/// The message type, used by both directions; outside every registered type.
pub const MSG_OUTPUTS: u8 = 0xE1;

/// The bytes of a `SelectOutput`, type included.
pub const SELECT_OUTPUT_LEN: usize = 8;

/// An entry's flags, bit 0: the output is one the compositor made rather than a
/// monitor somebody is sitting at. Only these are ever resized or rescaled, so a
/// client can say which of its choices will follow it.
pub const OUTPUT_HEADLESS: u8 = 1;

/// The longest name an entry carries, its length being one byte.
pub const MAX_NAME: usize = 255;

/// One output as the server lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputEntry {
    /// What the client names in a `SelectOutput`. Opaque to it, and unique for
    /// as long as the output exists.
    pub id: u32,
    /// The compositor's name for it, `HEADLESS-1` or `DP-2`.
    pub name: String,
    /// The framebuffer size a capture of it has, in pixels.
    pub width: u16,
    pub height: u16,
    /// The scale it is drawn at, fractional included.
    pub scale: f64,
    pub headless: bool,
}

/// Server → client `OutputList`: every output, and which one is shared.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE1` |
/// | 1 | U8 | padding |
/// | 2 | U16 | count |
/// | 4 | U32 | the shared output's id |
///
/// then `count` entries of fourteen bytes and a name:
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U32 | id |
/// | 4 | U16 | width, pixels |
/// | 6 | U16 | height, pixels |
/// | 8 | U32 | scale, 16.16 fixed |
/// | 12 | U8 | flags ([`OUTPUT_HEADLESS`]) |
/// | 13 | U8 | name length |
/// | 14 | U8[] | name, UTF-8 |
///
/// A name over [`MAX_NAME`] bytes is cut at a character boundary, and a list
/// longer than `u16::MAX` is cut to it; neither is reachable from a compositor.
pub fn output_list(active: u32, outputs: &[OutputEntry]) -> Vec<u8> {
    let outputs = &outputs[..outputs.len().min(usize::from(u16::MAX))];
    let mut msg = vec![MSG_OUTPUTS, 0];
    msg.extend_from_slice(&(outputs.len() as u16).to_be_bytes());
    msg.extend_from_slice(&active.to_be_bytes());
    for output in outputs {
        let name = clipped(&output.name);
        msg.extend_from_slice(&output.id.to_be_bytes());
        msg.extend_from_slice(&output.width.to_be_bytes());
        msg.extend_from_slice(&output.height.to_be_bytes());
        msg.extend_from_slice(&to_fixed(output.scale).to_be_bytes());
        msg.push(if output.headless { OUTPUT_HEADLESS } else { 0 });
        msg.push(name.len() as u8);
        msg.extend_from_slice(name.as_bytes());
    }
    msg
}

/// The name as it goes on the wire: at most [`MAX_NAME`] bytes, cut at a
/// character boundary so what arrives is still UTF-8.
fn clipped(name: &str) -> &str {
    if name.len() <= MAX_NAME {
        return name;
    }
    let mut end = MAX_NAME;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decoder written from the documented layout, not from [`output_list`].
    fn decode(bytes: &[u8]) -> (u32, Vec<OutputEntry>) {
        assert_eq!(bytes[0], 0xE1);
        assert_eq!(bytes[1], 0, "the padding byte is zero");
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
        (active, outputs)
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
        let (active, decoded) = decode(&output_list(7, &outputs));
        assert_eq!(active, 7);
        assert_eq!(decoded, outputs);
    }

    #[test]
    fn an_empty_list_is_a_header_alone() {
        let bytes = output_list(0, &[]);
        assert_eq!(bytes, [0xE1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(decode(&bytes), (0, Vec::new()));
    }

    #[test]
    fn a_list_has_the_documented_layout() {
        assert_eq!(
            output_list(1, &[entry(1, "DP-2", 1920, 1080, 1.0, false)]),
            [
                0xE1, 0, 0x00, 0x01, // one entry
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
        let (_, decoded) = decode(&output_list(1, &outputs));
        // 255 bytes is 127 two-byte characters and half of one, which is not sent.
        assert_eq!(decoded[0].name, "é".repeat(127));
    }
}
