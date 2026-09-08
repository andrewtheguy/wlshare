//! Security types: None, and classic VNC authentication.
//!
//! VncAuth is a 16-byte random challenge that the client encrypts with DES in
//! ECB mode, keyed by the first eight bytes of the password — zero-padded, and
//! with the bits of every key byte reversed, which is RFB's own convention and
//! not DES's. Only those eight bytes of the password count. It is not encryption
//! of anything but the challenge; the session itself is cleartext, which is why
//! the listen address is a configuration decision and not a default.

use des::Des;
use des::cipher::{BlockCipherEncrypt, KeyInit};

/// Security type 1: no authentication.
pub const SECURITY_NONE: u8 = 1;
/// Security type 2: classic VNC authentication.
pub const SECURITY_VNC_AUTH: u8 = 2;

/// Sixteen random bytes for one connection's challenge.
pub fn challenge() -> [u8; 16] {
    let mut c = [0u8; 16];
    getrandom::fill(&mut c).expect("the OS random source is unavailable");
    c
}

/// The response a client holding `password` gives to `challenge`.
pub fn response(password: &str, challenge: &[u8; 16]) -> [u8; 16] {
    let mut key = [0u8; 8];
    for (slot, byte) in key.iter_mut().zip(password.bytes()) {
        *slot = byte.reverse_bits();
    }
    let cipher = Des::new_from_slice(&key).expect("an 8-byte DES key");
    let mut out = *challenge;
    for block in out.as_chunks_mut::<8>().0 {
        cipher.encrypt_block(block.into());
    }
    out
}

/// Whether `given` is the response to `challenge` under `password`, compared
/// without an early exit on the first differing byte.
pub fn verify(password: &str, challenge: &[u8; 16], given: &[u8; 16]) -> bool {
    let expected = response(password, challenge);
    expected.iter().zip(given).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Responses computed outside this crate, so a wrong key derivation cannot
    /// agree with itself and pass: the bits were reversed by hand and the two
    /// ECB blocks encrypted with OpenSSL, not with `des`.
    ///
    /// "test" pads to `74 65 73 74 00 00 00 00`, which reverses byte by byte to
    /// the key `2e a6 ce 2e 00 00 00 00`; "wlshare!" fills all eight bytes and so
    /// leaves no zero half, which is what makes it worth having as well.
    #[test]
    fn responses_match_vectors_computed_elsewhere() {
        for (password, challenge, want) in [
            (
                "test",
                *b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f",
                *b"\x51\xa8\x9f\xa0\x01\x3d\x72\xc6\x55\x01\x95\x13\xaf\x52\xc2\x0c",
            ),
            (
                "wlshare!",
                *b"\x01\x23\x45\x67\x89\xab\xcd\xef\xfe\xdc\xba\x98\x76\x54\x32\x10",
                *b"\xd1\x50\x33\xa6\x42\x59\x4f\xfc\xca\xde\xdf\x79\xb8\xfe\xfe\x00",
            ),
        ] {
            assert_eq!(response(password, &challenge), want, "response to {password}");
            assert!(verify(password, &challenge, &want));
        }
    }

    /// The other half of the vectors above: a response is only good for the
    /// password and the challenge it was made from.
    #[test]
    fn a_response_is_bound_to_its_password_and_challenge() {
        let challenge = [0u8; 16];
        let r = response("test", &challenge);
        // Both blocks are the same plaintext under the same key, so ECB gives
        // the same ciphertext twice -- the one place that property is visible.
        assert_eq!(r[..8], r[8..]);
        assert!(verify("test", &challenge, &r));
        assert!(!verify("tesT", &challenge, &r));
        assert!(!verify("test", &[1u8; 16], &r));
    }

    #[test]
    fn only_the_first_eight_password_bytes_count() {
        let challenge = challenge();
        assert_eq!(response("longpassword", &challenge), response("longpass", &challenge));
        assert_ne!(response("longpass", &challenge), response("longpas", &challenge));
    }
}
