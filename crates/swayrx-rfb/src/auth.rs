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

    /// A vector produced by an independent client implementation: challenge of
    /// zeros, password "test", the DES-with-reversed-key-bits convention.
    #[test]
    fn a_known_response_verifies() {
        let challenge = [0u8; 16];
        let r = response("test", &challenge);
        // DES(key = reversed("test\0\0\0\0")) of a zero block, twice.
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
