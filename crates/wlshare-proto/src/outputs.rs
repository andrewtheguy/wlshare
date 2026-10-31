//! The outputs extension: how a client learns which outputs the compositor has
//! and asks for the one it wants shared.
//!
//! One framebuffer is one output, so the client that wants the other
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
//! - The client may send a `SelectOutput` ([`select_output`]) naming the
//!   output it wants shared. The server switches the capture to it and answers with an `OutputList`
//!   whose `active` says what actually happened: a request it cannot honour —
//!   an id it does not have, an output that went away — is answered with the
//!   list as it is, so the client's menu ends up agreeing with what is on the
//!   canvas rather than with what was clicked. A request is never left without
//!   an answer.
//! - The client may send a `VirtualDisplays` ([`virtual_displays`]) saying
//!   how many virtual displays it wants, as an RDP client's monitor layout and a Mac's display
//!   configuration do. A server whose compositor can make outputs — sway,
//!   through its IPC — adds headless outputs and takes them away until there
//!   are that many, each arrival and departure a list of its own; the request
//!   itself is answered with the list as it is. The list's `virtual displays`
//!   byte says the most the server makes, and 0 that it makes none: the
//!   request is then answered and changes nothing.
//!
//! The two client messages share one eight-byte layout: the type, an operation
//! byte ([`OP_SELECT_OUTPUT`], [`OP_VIRTUAL_DISPLAYS`]), two bytes of padding
//! and one U32, the id or the count.
//!
//! The size and scale of each entry are what a capture of that output would
//! produce, so a client can label the choice before making it. Scales are the
//! density extension's 16.16 fixed point.

use crate::density::to_fixed;

/// The message type, used by both directions; outside every registered type.
pub const MSG_OUTPUTS: u8 = 0xE1;

/// The bytes of a client's message, type included: type, operation, two bytes
/// of padding, and one U32.
pub const CLIENT_OUTPUTS_LEN: usize = 8;

/// The operation byte of a `SelectOutput`: share the output with the id that
/// follows.
pub const OP_SELECT_OUTPUT: u8 = 0;

/// The operation byte of a `VirtualDisplays`: have as many virtual displays as
/// the count that follows.
pub const OP_VIRTUAL_DISPLAYS: u8 = 1;

/// What a client asked in an outputs message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientOutputs {
    /// Share the output with this id.
    Select { id: u32 },
    /// Have this many virtual displays.
    VirtualDisplays { count: u32 },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OutputsParseError {
    #[error("outputs operation {0} is not select or virtual displays")]
    UnknownOperation(u8),
}

/// Parse the client message at the front of `buf`, whose first byte is
/// [`MSG_OUTPUTS`]. `Ok(None)` means more bytes are needed.
pub fn parse_client(buf: &[u8]) -> Result<Option<(ClientOutputs, usize)>, OutputsParseError> {
    if buf.len() < CLIENT_OUTPUTS_LEN {
        return Ok(None);
    }
    let value = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let msg = match buf[1] {
        OP_SELECT_OUTPUT => ClientOutputs::Select { id: value },
        OP_VIRTUAL_DISPLAYS => ClientOutputs::VirtualDisplays { count: value },
        other => return Err(OutputsParseError::UnknownOperation(other)),
    };
    Ok(Some((msg, CLIENT_OUTPUTS_LEN)))
}

/// Client → server `SelectOutput`: share the output with this id.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE1` |
/// | 1 | U8 | [`OP_SELECT_OUTPUT`] |
/// | 2 | U8\[2\] | padding |
/// | 4 | U32 | id |
pub fn select_output(id: u32) -> [u8; CLIENT_OUTPUTS_LEN] {
    client_message(OP_SELECT_OUTPUT, id)
}

/// Client → server `VirtualDisplays`: have this many virtual displays.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE1` |
/// | 1 | U8 | [`OP_VIRTUAL_DISPLAYS`] |
/// | 2 | U8\[2\] | padding |
/// | 4 | U32 | count |
pub fn virtual_displays(count: u32) -> [u8; CLIENT_OUTPUTS_LEN] {
    client_message(OP_VIRTUAL_DISPLAYS, count)
}

fn client_message(op: u8, value: u32) -> [u8; CLIENT_OUTPUTS_LEN] {
    let [a, b, c, d] = value.to_be_bytes();
    [MSG_OUTPUTS, op, 0, 0, a, b, c, d]
}

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

/// Server → client `OutputList`: every output, which one is shared, and the
/// most virtual displays the server makes on request — 0 for one that makes
/// none.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE1` |
/// | 1 | U8 | virtual displays: the most the server makes |
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
pub fn output_list(active: u32, virtual_displays: u8, outputs: &[OutputEntry]) -> Vec<u8> {
    let outputs = &outputs[..outputs.len().min(usize::from(u16::MAX))];
    let mut msg = vec![MSG_OUTPUTS, virtual_displays];
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

/// The bytes of an `OutputList` before its entries, type included.
pub const LIST_HEADER_LEN: usize = 8;

/// The bytes of an entry before its name.
pub const ENTRY_HEADER_LEN: usize = 14;

/// An `OutputList`'s header: the most virtual displays the server makes, how
/// many entries follow, and the shared output's id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListHeader {
    pub virtual_displays: u8,
    pub count: u16,
    pub active: u32,
}

impl ListHeader {
    /// The header of a list, whose first byte is [`MSG_OUTPUTS`].
    pub fn parse(header: &[u8; LIST_HEADER_LEN]) -> Self {
        debug_assert_eq!(header[0], MSG_OUTPUTS);
        Self { virtual_displays: header[1], count: crate::u16_at(header, 2), active: crate::u32_at(header, 4) }
    }
}

/// An entry's fixed part: everything but its name, and how long that is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryHeader {
    pub id: u32,
    pub width: u16,
    pub height: u16,
    /// The scale, 16.16 fixed ([`crate::density::from_fixed`]).
    pub fixed: u32,
    pub headless: bool,
    /// The bytes of the name that follows.
    pub name_len: u8,
}

impl EntryHeader {
    pub fn parse(entry: &[u8; ENTRY_HEADER_LEN]) -> Self {
        Self {
            id: crate::u32_at(entry, 0),
            width: crate::u16_at(entry, 4),
            height: crate::u16_at(entry, 6),
            fixed: crate::u32_at(entry, 8),
            headless: entry[12] & OUTPUT_HEADLESS != 0,
            name_len: entry[13],
        }
    }
}

#[cfg(test)]
mod tests;
