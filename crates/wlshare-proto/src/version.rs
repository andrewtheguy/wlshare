//! The protocol version: which revision of the private extensions both ends
//! speak, exactly, since none of them keeps an older layout beside a newer one.
//!
//! [`PROTOCOL_VERSION`] is this crate's major version, and the repository tags
//! each one `proto-v<N>` at the release that brings it: the zDesk gateway pins
//! that tag, so a release of the daemon that changes nothing here asks nothing
//! of the gateway. Any change to this crate is a new version, so the bytes a
//! tag holds are the bytes every daemon of that version speaks.
//!
//! On the wire it is a pseudo-encoding and an answer to it:
//!
//! - The client lists `WLV` and the version ([`encoding`]) in every
//!   `SetEncodings` that lists anything private: every encoding whose top two
//!   bytes are `WL` ([`crate::private`]). A list that names any of them
//!   without a version, or with two, is fatal.
//! - The server answers the first list that carries one, before anything else
//!   it sends, with an update of one empty rectangle of its own version's
//!   encoding ([`answer`]), unrequested. Where the client's version is not its
//!   own it closes the connection after it; the client, which reads the
//!   server's version from the answer, says which two disagreed. The client
//!   reads the answer as the first server message after its list: anything
//!   else there is a server that does not version the extensions, which is not
//!   wlshare as this crate knows it.
//!
//! A later list carries the same version and is not answered again.

use thiserror::Error;

/// The revision of the private extensions this crate builds and parses.
pub const PROTOCOL_VERSION: u8 = 2;

/// `WLV` and a zero byte: [`encoding`] puts the version in that byte.
pub const ENCODING_VERSION_BASE: i32 = 0x574c_5600;

/// The bytes of the server's answer, the update's type included.
pub const ANSWER_LEN: usize = 16;

/// The pseudo-encoding that names `version`.
pub const fn encoding(version: u8) -> i32 {
    ENCODING_VERSION_BASE + version as i32
}

/// The version `encoding` names, or `None` where it names none.
pub fn of(encoding: i32) -> Option<u8> {
    u8::try_from(encoding.checked_sub(ENCODING_VERSION_BASE)?).ok()
}

/// The version the client's list carries, or `None` where it lists nothing
/// private ([`crate::private`]).
pub fn listed(encodings: &[i32]) -> Result<Option<u8>, VersionListError> {
    let mut versions = encodings.iter().filter_map(|&e| of(e));
    let Some(version) = versions.next() else {
        return match encodings.iter().any(|&e| crate::private(e)) {
            true => Err(VersionListError::Missing),
            false => Ok(None),
        };
    };
    match versions.next() {
        Some(other) => Err(VersionListError::Several(version, other)),
        None => Ok(Some(version)),
    }
}

/// Server → client: a `FramebufferUpdate` of one empty rectangle whose
/// encoding names the server's version.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0`, FramebufferUpdate |
/// | 1 | U8 | padding |
/// | 2 | U16 | rectangles, `1` |
/// | 4 | U16 | x, 0 |
/// | 6 | U16 | y, 0 |
/// | 8 | U16 | width, 0 |
/// | 10 | U16 | height, 0 |
/// | 12 | S32 | encoding, `0x574c5600` + version |
pub fn answer(version: u8) -> [u8; ANSWER_LEN] {
    let mut msg = [0u8; ANSWER_LEN];
    msg[2..4].copy_from_slice(&1u16.to_be_bytes());
    msg[4..].copy_from_slice(&crate::rect_header(0, 0, 0, 0, encoding(version)));
    msg
}

/// The server's version from its answer, the first [`ANSWER_LEN`] bytes the
/// server sent after the client's list.
pub fn parse_answer(msg: &[u8; ANSWER_LEN]) -> Result<u8, VersionAnswerError> {
    let header = [0u8, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
    let encoding = i32::from_be_bytes([msg[12], msg[13], msg[14], msg[15]]);
    match of(encoding) {
        Some(version) if msg[..12] == header => Ok(version),
        _ => Err(VersionAnswerError::NotAnswer),
    }
}

/// Why a list is not one a server speaks to. Fatal to the connection, as a
/// malformed message is.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VersionListError {
    #[error("the list names wlshare's extensions without a protocol version")]
    Missing,
    #[error("the list names protocol versions {0} and {1}")]
    Several(u8, u8),
}

/// Why what the server sent first is not its version.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VersionAnswerError {
    #[error("the server's first message is not wlshare's protocol version")]
    NotAnswer,
}

#[cfg(test)]
mod tests {
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
}
