//! The link between a daemon and an encoding agent: its bytes, which both ends
//! ([`crate::agent::client`], [`crate::agent::serve`]) build and parse here, and the
//! pre-shared key in front of them. Experimental.
//!
//! A host with no GPU codes its streams by libx264, which is most of what a
//! desktop in motion costs its processor. With an agent configured, a stream
//! is coded by the agent's GPU instead: the daemon converts the picture to
//! NV12 as it does for any encoder, sends the agent what changed of it, and
//! is sent the access unit back. The agent holds a copy of the picture and an
//! encoder over it ([`crate::h264::vaapi`]), so the stream is the one a local GPU
//! would have coded, quantizer by quantizer.
//!
//! **TCP, a connection a stream.** A picture is megabytes and every byte of it
//! matters: a hole in a picture is coded as garbage, and a lost access unit
//! breaks every frame predicted from it. Over UDP that is fragmentation,
//! reassembly, retransmission and pacing to write, which is TCP written again;
//! what UDP would buy, a late packet not holding up the next, is nothing on
//! the one link this is for, a LAN that loses none. So each stream is one TCP
//! connection with Nagle's algorithm off, a frame one request and one reply.
//! A link slow or lossy enough to want more is the wrong place for raw
//! pictures at all.
//!
//! **The key.** Both ends hold one pre-shared key, a file's bytes. Each sends
//! a random, and proves the key to the other by an HMAC-SHA256 over both
//! randoms and its own role, the agent first, so neither end's proof can be
//! replayed to a later connection or reflected back at its sender. That is
//! all the key does: nothing after it is encrypted or authenticated, so the
//! pictures cross the link in the clear, and whoever can alter the
//! connection's packets can alter the pictures. It keeps a stranger on the
//! LAN from using the encoder or posing as it; a link that is not a trusted
//! LAN goes inside a tunnel (WireGuard, SSH).
//!
//! **The bytes**, big-endian. Each end opens with its hello, `WLEA`, the
//! link's version and 32 random bytes, and reads the other's; a version that
//! is not this one ends the connection, since both ends are wlshare and are
//! to be the same release. The agent sends its proof, 32 bytes; the daemon its
//! own and the coded picture's width and height, each `u16`; the agent
//! answers with a status byte, [`OK`] or [`FAILED`] and why (a `u16` length
//! and that many bytes of UTF-8).
//!
//! Then a frame at a time. The daemon: [`FRAME`], a flags byte ([`KEYFRAME`],
//! [`REFINE`]), the quantizer, a `u32` count of rectangles, and each
//! rectangle as its `x`, `y`, width and height, each `u16` and even, its luma
//! rows and then its chroma rows, half as many, each row the rectangle's
//! width in bytes. The agent: [`OK`], a `u32` length and the access unit;
//! [`NONE`] where its encoder made no frame; or [`FAILED`] and why, after
//! which the connection is closed. The daemon closes the connection to end
//! the stream.
//!
//! **What is sent.** The macroblocks a read wrote into since the last frame
//! ([`crate::h264::Planes::still`]), as rectangles of whole macroblocks merged across and
//! down: a stream's first frame is the whole picture, a typed letter a few
//! kilobytes, and a frame that only refines no pixels at all. A video filling
//! the screen is the whole picture every frame, 3 MB at 1920×1080: 93 MB a
//! second at 30 frames, three quarters of a gigabit link.
//!
//! **No fallback.** An agent that is away, refuses the key or fails a frame
//! fails the stream as any coder's failure does, and the session with it: a
//! daemon set to an agent has no other H.264 encoder.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use hmac::{Hmac, KeyInit as _};
use rand::Rng as _;
use sha2::Sha256;

pub use hmac::Mac;

/// What every hello starts with.
const MAGIC: [u8; 4] = *b"WLEA";

/// The link's version, which both ends must share: any change to its bytes
/// is another.
pub const VERSION: u16 = 1;

/// A hello's random.
pub const NONCE: usize = 32;

/// A proof: HMAC-SHA256's output, checked and made through [`Mac`].
pub const PROOF: usize = 32;

/// A hello: the magic, the version and the random.
pub const HELLO: usize = MAGIC.len() + 2 + NONCE;

/// The fewest bytes a key file holds, once trimmed: what 16 random bytes in
/// hexadecimal are.
pub const MIN_KEY: usize = 32;

/// The longest side of a picture an agent takes, past any H.264 level's.
pub const MAX_SIDE: usize = 8192;

/// How long either end waits on the other within the handshake or a frame.
pub const IO: Duration = Duration::from_secs(10);

/// The daemon's one message after the handshake: a frame to code.
pub const FRAME: u8 = 1;

/// A frame's flag: an IDR is asked for.
pub const KEYFRAME: u8 = 1;
/// A frame's flag: the encoder is to look at every macroblock again
/// ([`crate::h264::Planes::refine`]).
pub const REFINE: u8 = 2;

/// The agent's status: done, and for a frame, the access unit follows.
pub const OK: u8 = 0;
/// The agent's status: it could not, and why follows.
pub const FAILED: u8 = 1;
/// The agent's status for a frame: its encoder made none.
pub const NONE: u8 = 2;

/// Which end a proof is.
#[derive(Clone, Copy)]
pub enum Role {
    Daemon,
    Agent,
}

/// The hello an end opens with, around its random.
pub fn hello(nonce: &[u8; NONCE]) -> [u8; HELLO] {
    let mut hello = [0; HELLO];
    hello[..4].copy_from_slice(&MAGIC);
    hello[4..6].copy_from_slice(&VERSION.to_be_bytes());
    hello[6..].copy_from_slice(nonce);
    hello
}

/// Read the other end's hello and return its random: an error for anything
/// that is not this link at this version.
pub fn read_hello(from: &mut impl Read) -> anyhow::Result<[u8; NONCE]> {
    let mut hello = [0; HELLO];
    from.read_exact(&mut hello).context("reading its hello")?;
    anyhow::ensure!(hello[..4] == MAGIC, "it is not an encoding agent's link");
    let version = u16::from_be_bytes([hello[4], hello[5]]);
    anyhow::ensure!(
        version == VERSION,
        "it speaks version {version} of the link and this end version {VERSION}: run the same wlshare at both ends"
    );
    Ok(hello[6..].try_into().expect("a hello's random"))
}

/// The proof of `key` that `role` gives over a connection's two randoms.
pub fn proof(
    key: &[u8],
    role: Role,
    daemon: &[u8; NONCE],
    agent: &[u8; NONCE],
) -> anyhow::Result<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| anyhow::anyhow!("the key is no HMAC key"))?;
    mac.update(match role {
        Role::Daemon => &b"wlshare encoding agent link: the daemon's proof"[..],
        Role::Agent => b"wlshare encoding agent link: the agent's proof",
    });
    mac.update(daemon);
    mac.update(agent);
    Ok(mac)
}

/// A new random for a hello.
pub fn nonce() -> [u8; NONCE] {
    let mut nonce = [0; NONCE];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

/// The pre-shared key a file holds: its bytes without the whitespace around
/// them, [`MIN_KEY`] at least.
pub fn read_key(path: &Path) -> anyhow::Result<Vec<u8>> {
    let key = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let key = key.trim_ascii();
    anyhow::ensure!(
        key.len() >= MIN_KEY,
        "{} holds a key of {} bytes, and one is at least {MIN_KEY}",
        path.display(),
        key.len()
    );
    Ok(key.to_vec())
}

/// A status byte's failure, as the agent writes it: [`FAILED`] and why.
pub fn failed(why: &str) -> Vec<u8> {
    let why = &why.as_bytes()[..why.len().min(usize::from(u16::MAX))];
    let mut message = vec![FAILED];
    message.extend_from_slice(&(why.len() as u16).to_be_bytes());
    message.extend_from_slice(why);
    message
}

/// A rectangle of the coded picture, in luma samples, every number even.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Span {
    /// The rectangle's header: its four numbers.
    pub fn header(&self) -> [u8; 8] {
        let [x, y, w, h] = [self.x, self.y, self.w, self.h].map(|n| (n as u16).to_be_bytes());
        [x[0], x[1], y[0], y[1], w[0], w[1], h[0], h[1]]
    }

    /// A rectangle read from its header, held to a `width`×`height` picture:
    /// even, with pixels, and inside it.
    pub fn read(from: &mut impl Read, (width, height): (usize, usize)) -> anyhow::Result<Self> {
        let mut header = [0; 8];
        from.read_exact(&mut header)
            .context("reading a rectangle")?;
        let [x, y, w, h] =
            [0, 2, 4, 6].map(|at| usize::from(u16::from_be_bytes([header[at], header[at + 1]])));
        anyhow::ensure!(
            [x, y, w, h].iter().all(|n| n % 2 == 0)
                && w > 0
                && h > 0
                && x + w <= width
                && y + h <= height,
            "a {w}x{h} rectangle at {x},{y} is no part of a {width}x{height} picture"
        );
        Ok(Self { x, y, w, h })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rectangle_outside_its_picture_is_refused() {
        let read = |span: Span| Span::read(&mut &span.header()[..], (100, 70));
        let inside = Span {
            x: 64,
            y: 64,
            w: 36,
            h: 6,
        };
        assert_eq!(read(inside).unwrap(), inside);
        for outside in [
            Span { w: 38, ..inside },
            Span { h: 8, ..inside },
            Span { w: 0, ..inside },
            Span { x: 65, ..inside },
            Span { h: 5, ..inside },
        ] {
            assert!(read(outside).is_err(), "{outside:?}");
        }
    }

    /// A proof is its key's, its role's and its connection's: none passes
    /// for another.
    #[test]
    fn a_proof_is_of_one_key_one_role_and_one_connection() {
        let key = [7u8; MIN_KEY];
        let (daemon, agent) = ([1u8; NONCE], [2u8; NONCE]);
        let made = proof(&key, Role::Agent, &daemon, &agent)
            .unwrap()
            .finalize()
            .into_bytes();
        let passes = |key: &[u8], role, daemon: &[u8; NONCE], agent: &[u8; NONCE]| {
            proof(key, role, daemon, agent)
                .unwrap()
                .verify_slice(&made)
                .is_ok()
        };
        assert!(passes(&key, Role::Agent, &daemon, &agent));
        assert!(!passes(&[8u8; MIN_KEY], Role::Agent, &daemon, &agent));
        assert!(!passes(&key, Role::Daemon, &daemon, &agent));
        assert!(!passes(&key, Role::Agent, &agent, &daemon));
        assert!(!passes(&key, Role::Agent, &[3u8; NONCE], &agent));
    }

    #[test]
    fn a_hello_of_another_version_or_another_protocol_is_refused() {
        let ours = hello(&[9; NONCE]);
        assert_eq!(read_hello(&mut &ours[..]).unwrap(), [9; NONCE]);
        let mut newer = ours;
        newer[4..6].copy_from_slice(&(VERSION + 1).to_be_bytes());
        let why = read_hello(&mut &newer[..]).unwrap_err().to_string();
        assert!(why.contains("the same wlshare"), "{why}");
        let mut other = ours;
        other[..4].copy_from_slice(b"RFB ");
        assert!(read_hello(&mut &other[..]).is_err());
    }
}
