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
//!
//! **How long.** A handshake is done within [`HANDSHAKE_WITHIN`] and a frame
//! within [`FRAME_WITHIN`], first byte to last, however the other end spaces
//! its bytes ([`Timed`]); and an end that is no longer there is found out
//! within [`KEEPALIVE`], whether the connection stands idle or holds bytes
//! never acknowledged ([`keepalive`]).

use std::io::{IoSlice, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

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

/// How long a handshake may take, its first byte to its last: at the agent
/// until the key is proved, and at the daemon until the agent has opened its
/// encoder and said so.
pub const HANDSHAKE_WITHIN: Duration = Duration::from_secs(5);

/// How long a frame may take: at the daemon the picture sent, coded and the
/// access unit read back, which is how long a session stands still on an
/// agent that has stopped answering; at the agent, a request read from its
/// first byte to its last. A 3840×2160 picture is a second of a 100 Mbit
/// link, and a link slower than that is not one for raw pictures.
pub const FRAME_WITHIN: Duration = Duration::from_secs(5);

/// How long an end that has gone without a word is waited on before its
/// connection is closed: a stream is idle for as long as the desktop is
/// still, so only the keepalive tells. Half of it before the first probe,
/// and the rest three probes unanswered, the count said and not left
/// to the system's, which on Linux is nine. And as long for what was sent
/// to be acknowledged, since a connection with a frame in flight is not
/// idle and is not probed: there it is the retransmissions that tell, which
/// left to the system go on for a quarter of an hour.
pub const KEEPALIVE: Duration = Duration::from_secs(30);

/// The keepalive probes that go unanswered before an end is taken for gone.
const PROBES: u32 = 3;

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

/// Have an end that is no longer there found out within [`KEEPALIVE`],
/// whether the connection stands idle or holds bytes it never acknowledged.
pub fn keepalive(stream: &TcpStream) -> std::io::Result<()> {
    let socket = socket2::SockRef::from(stream);
    socket.set_tcp_keepalive(
        &socket2::TcpKeepalive::new()
            .with_time(KEEPALIVE / 2)
            .with_interval(KEEPALIVE / 2 / PROBES)
            .with_retries(PROBES),
    )?;
    socket.set_tcp_user_timeout(Some(KEEPALIVE))
}

/// A connection read and written until a moment and no later, however the
/// other end spaces its bytes: each read and each write is given what is
/// left of the time, where a socket's own timeout is one call's and starts
/// again with every byte that arrives.
pub struct Timed<'a> {
    stream: &'a TcpStream,
    /// When what is read and written now is to be done, and the time it
    /// was given; `None` waits for as long as the other end takes.
    until: Option<(Instant, Duration)>,
}

impl<'a> Timed<'a> {
    /// The connection, waited on for as long as it takes until
    /// [`Self::within`] says otherwise.
    pub fn new(stream: &'a TcpStream) -> Self {
        Self {
            stream,
            until: None,
        }
    }

    /// What is read and written from now is done within `limit`.
    pub fn within(&mut self, limit: Duration) {
        self.until = Some((Instant::now() + limit, limit));
    }

    /// What is read and written from now takes as long as it takes.
    pub fn at_leisure(&mut self) {
        self.until = None;
    }

    /// The time left, `None` where there is no limit; an error with none.
    fn left(&self) -> std::io::Result<Option<Duration>> {
        let Some((until, limit)) = self.until else {
            return Ok(None);
        };
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(late(limit));
        }
        Ok(Some(left))
    }

    /// A call's error, said as the limit it ran into where it is the
    /// socket's timeout: the system's word for that is "resource temporarily
    /// unavailable", which tells nobody what happened.
    fn said(&self, error: std::io::Error) -> std::io::Error {
        use std::io::ErrorKind::{TimedOut, WouldBlock};
        match self.until {
            Some((_, limit)) if matches!(error.kind(), TimedOut | WouldBlock) => late(limit),
            _ => error,
        }
    }
}

/// The error of what was not done within `limit`.
fn late(limit: Duration) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("not done within {limit:?}"),
    )
}

impl Read for Timed<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(self.left()?)?;
        let mut stream = self.stream;
        stream.read(buf).map_err(|e| self.said(e))
    }
}

impl Write for Timed<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(self.left()?)?;
        let mut stream = self.stream;
        stream.write(buf).map_err(|e| self.said(e))
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(self.left()?)?;
        let mut stream = self.stream;
        stream.write_vectored(bufs).map_err(|e| self.said(e))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Write every byte of `slices`, in order, from where they lie: a picture's
/// rows go to the connection without being gathered into one buffer first.
pub fn write_all_vectored(
    to: &mut impl Write,
    mut slices: &mut [IoSlice<'_>],
) -> std::io::Result<()> {
    loop {
        // Past the slices with nothing in them, which no write would take.
        IoSlice::advance_slices(&mut slices, 0);
        if slices.is_empty() {
            return Ok(());
        }
        match to.write_vectored(slices) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(written) => IoSlice::advance_slices(&mut slices, written),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
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

    /// A loopback connection's two ends.
    fn pair() -> (TcpStream, TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let near = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        (near, listener.accept().unwrap().0)
    }

    /// An end gone without a word is found out within [`KEEPALIVE`], as the
    /// socket itself says it is set: the wait before the first probe and
    /// every probe after it, and the wait for what was sent to be
    /// acknowledged.
    #[test]
    fn an_end_gone_is_found_out_within_the_keepalive() {
        let (stream, _far) = pair();
        keepalive(&stream).unwrap();
        let socket = socket2::SockRef::from(&stream);
        assert!(socket.keepalive().unwrap());
        assert_eq!(
            socket.tcp_keepalive_time().unwrap()
                + socket.tcp_keepalive_interval().unwrap()
                    * socket.tcp_keepalive_retries().unwrap(),
            KEEPALIVE
        );
        assert_eq!(socket.tcp_user_timeout().unwrap(), Some(KEEPALIVE));
    }

    /// What is to be done within a time is not done later for arriving a
    /// byte at a time, each inside the socket's own timeout; the error says
    /// the time, and with no limit the bytes are waited for.
    #[test]
    fn a_limit_is_on_the_whole_and_not_on_each_byte() {
        let (near, mut far) = pair();
        let dripping = std::thread::spawn(move || {
            for byte in 0..20u8 {
                if far.write_all(&[byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let mut timed = Timed::new(&near);
        let mut some = [0; 4];
        timed.read_exact(&mut some).unwrap();
        assert_eq!(some, [0, 1, 2, 3]);

        timed.within(Duration::from_millis(300));
        let began = Instant::now();
        let why = timed.read_exact(&mut [0; 64]).unwrap_err();
        assert_eq!(why.kind(), std::io::ErrorKind::TimedOut);
        assert!(why.to_string().contains("300ms"), "{why}");
        assert!(
            began.elapsed() < Duration::from_millis(900),
            "{:?}",
            began.elapsed()
        );
        drop(near);
        dripping.join().unwrap();
    }

    /// Slices are written whole and in order, through writes that take part
    /// of one and writes that take several.
    #[test]
    fn slices_are_written_whole_and_in_order() {
        /// Takes at most so many bytes a write.
        struct Sips(Vec<u8>, usize);
        impl Write for Sips {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let taken = buf.len().min(self.1);
                self.0.extend_from_slice(&buf[..taken]);
                Ok(taken)
            }
            fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> std::io::Result<usize> {
                let mut left = self.1;
                for buf in bufs {
                    let taken = buf.len().min(left);
                    self.0.extend_from_slice(&buf[..taken]);
                    left -= taken;
                }
                Ok(self.1 - left)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let parts: [&[u8]; 7] = [
            b"",
            b"a pic",
            b"",
            b"ture's rows",
            b", ",
            b"one after another",
            b"",
        ];
        for sip in [1, 3, 7, 100] {
            let mut slices: Vec<IoSlice<'_>> = parts.iter().map(|p| IoSlice::new(p)).collect();
            let mut to = Sips(Vec::new(), sip);
            write_all_vectored(&mut to, &mut slices).unwrap();
            assert_eq!(to.0, parts.concat(), "{sip} bytes a write");
        }
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
