//! RealVNC's RSA-AES security types (RFB 5 and 129), server side: an
//! authenticated, encrypted RFB session over an ordinary 3.8 wire, and the one
//! standard way a client can tell this server *who* is connecting.
//!
//! The types are RealVNC's, documented in the community `rfbproto` and spoken
//! on the open side by TigerVNC, neatvnc and the remotex gateway, whose client
//! this server was written against. VncAuth proves knowledge of a machine's
//! secret and encrypts nothing; RSA-AES carries a username and a password, so
//! they can be checked against the system's accounts, and every byte after the
//! key exchange is inside AES-EAX.
//!
//! ## The exchange
//!
//! ```text
//! server → client   u32 bits || modulus || exponent        the server's RSA key
//! client → server   u32 bits || modulus || exponent        the client's, fresh per session
//! client → server   u16 len  || RSA-PKCS1v15(server key, client random)
//! server → client   u16 len  || RSA-PKCS1v15(client key, server random)
//! ```
//!
//! Both randoms known, each direction gets its own AES key:
//!
//! ```text
//! client → server   H(server random || client random)[..key]
//! server → client   H(client random || server random)[..key]
//! ```
//!
//! `H` is SHA-1 for the 128-bit type and SHA-256 for the 256-bit one, the random
//! is 16 or 32 bytes to match, and from here every byte in both directions is a
//! [frame](Sealer::frame). Inside the frames: the client's hash of the two public
//! keys, the server's hash of them the other way round (which binds the session
//! to the keys that were actually sent — a middle-man's substitution shows up
//! here), a one-byte subtype saying the server wants a username and a password,
//! the credentials, then RFB's own SecurityResult and everything after it.
//!
//! ## What the server's key is worth
//!
//! The server's key is the one thing a client can pin. It is generated once and
//! kept in a file, so its fingerprint — RealVNC's eight dashed SHA-1 bytes,
//! which this module logs at startup and the remotex gateway logs on every
//! connection — is stable and an operator can compare the two. Against an
//! active middle-man the pinning is the client's to do; against a passive one
//! the session is encrypted, which VncAuth's never was.
//!
//! ## The frame
//!
//! ```text
//! wire   u16 len || byte[len] AES-EAX ciphertext || byte[16] tag
//! nonce  a 128-bit little-endian counter, from zero, one per frame per direction
//! aad    the two length bytes
//! ```
//!
//! A frame is a transport unit and not a message: a framebuffer update spans as
//! many frames as it needs, so the read side is an [`AsyncRead`] yielding the
//! concatenation of frame bodies and the write side takes a whole message and
//! cuts it. The counter is the nonce, so a frame is decrypted exactly once and
//! in order; a reader that has failed is finished.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use aes::{Aes128, Aes256};
use eax::Eax;
use eax::aead::{Aead as _, KeyInit as _, Payload};
use rand::Rng as _;
use rsa::pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _};
use rsa::traits::PublicKeyParts as _;
use rsa::{BoxedUint, Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _, ReadBuf};

/// RFB security type `RA2`: RSA key exchange, AES-128-EAX, SHA-1.
pub const SECURITY_RSA_AES_128: u8 = 5;
/// RFB security type `RA2_256`: the same with AES-256-EAX and SHA-256.
pub const SECURITY_RSA_AES_256: u8 = 129;

/// Bits in the key generated for a server without one. TigerVNC's and
/// wayvnc's size; the exchange lets the two ends' sizes differ.
pub const SERVER_KEY_BITS: usize = 2048;
/// Bounds on the client's key, TigerVNC's. Below the lower one the random it
/// carries is not protected; above the upper one a bogus length turns into a
/// very large allocation and a very slow exponentiation.
const MIN_CLIENT_KEY_BITS: u32 = 1024;
const MAX_CLIENT_KEY_BITS: u32 = 8192;
/// The most plaintext put in one outgoing frame: TigerVNC's `MaxMessageSize`,
/// and the receive buffer of every implementation this has been tried against.
const MAX_FRAME_BODY: usize = 8192;
/// Length prefix and tag around a frame's ciphertext.
const HEADER: usize = 2;
const TAG: usize = 16;

/// RSA-AES subtype 1: the server wants a username and a password. The only
/// subtype this server sends; subtype 2, a password alone, is what VncAuth
/// already is.
const SUBTYPE_USER_PASS: u8 = 1;

/// Which of the two types was chosen, deciding the hash, the key length and
/// the random's size together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strength {
    Aes128,
    Aes256,
}

impl Strength {
    /// The strength behind an RFB security type, if it is one of the two.
    pub fn of(security_type: u8) -> Option<Self> {
        match security_type {
            SECURITY_RSA_AES_128 => Some(Self::Aes128),
            SECURITY_RSA_AES_256 => Some(Self::Aes256),
            _ => None,
        }
    }

    /// Bytes of random each side contributes, which is also the AES key length.
    fn random_len(self) -> usize {
        match self {
            Self::Aes128 => 16,
            Self::Aes256 => 32,
        }
    }

    /// The type's hash over the given pieces, in order. SHA-1 yields 20 bytes
    /// of which the 128-bit key takes the first 16; SHA-256 yields exactly the
    /// 256-bit key.
    fn hash(self, parts: &[&[u8]]) -> Vec<u8> {
        match self {
            Self::Aes128 => {
                let mut h = Sha1::new();
                parts.iter().for_each(|p| h.update(p));
                h.finalize().to_vec()
            }
            Self::Aes256 => {
                let mut h = Sha256::new();
                parts.iter().for_each(|p| h.update(p));
                h.finalize().to_vec()
            }
        }
    }

    fn cipher(self, key: &[u8]) -> Cipher {
        match self {
            Self::Aes128 => Cipher::Aes128(Eax::<Aes128>::new_from_slice(key).expect("a 16-byte key")),
            Self::Aes256 => Cipher::Aes256(Eax::<Aes256>::new_from_slice(key).expect("a 32-byte key")),
        }
    }
}

/// An RSA public key as it travels: `u32 bits || modulus || exponent`, the
/// two numbers big-endian and each padded to the modulus' byte length. Kept in
/// wire form because both the key hashes and the fingerprint are over exactly
/// these bytes.
#[derive(Clone, PartialEq, Eq)]
struct WireKey(Vec<u8>);

impl WireKey {
    fn encode(bits: u32, modulus: &[u8], exponent: &[u8]) -> Self {
        let size = (bits as usize).div_ceil(8);
        let mut wire = Vec::with_capacity(4 + 2 * size);
        wire.extend_from_slice(&bits.to_be_bytes());
        wire.extend_from_slice(&left_pad(modulus, size));
        wire.extend_from_slice(&left_pad(exponent, size));
        Self(wire)
    }

    fn of_public(key: &RsaPublicKey) -> Self {
        Self::encode(key.n().bits(), &key.n_bytes(), &key.e_bytes())
    }

    /// The key's byte length, which is what its ciphertexts are long.
    fn size(&self) -> usize {
        (self.0.len() - 4) / 2
    }

    fn modulus(&self) -> &[u8] {
        &self.0[4..4 + self.size()]
    }

    fn exponent(&self) -> &[u8] {
        &self.0[4 + self.size()..]
    }

    /// RealVNC's display of a key: the first eight bytes of its SHA-1, dashed.
    fn fingerprint(&self) -> String {
        let digest = Sha1::digest(&self.0);
        digest[..8].iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join("-")
    }

    fn public_key(&self) -> Result<RsaPublicKey, Error> {
        // Public inputs, so the variable-time decoder is the right one.
        RsaPublicKey::new(
            BoxedUint::from_be_slice_vartime(self.modulus()),
            BoxedUint::from_be_slice_vartime(self.exponent()),
        )
        .map_err(|e| Error::Protocol(format!("the client's RSA key is invalid: {e}")))
    }
}

fn left_pad(bytes: &[u8], size: usize) -> Vec<u8> {
    let bytes = bytes.iter().copied().skip_while(|&b| b == 0).collect::<Vec<_>>();
    assert!(bytes.len() <= size, "a {}-byte number in a {size}-byte field", bytes.len());
    let mut out = vec![0u8; size - bytes.len()];
    out.extend_from_slice(&bytes);
    out
}

/// What can go wrong between the security type being chosen and the
/// credentials arriving.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The client's bytes do not make an RSA-AES exchange.
    #[error("{0}")]
    Protocol(String),
    /// The client's key hash does not cover the keys that were exchanged.
    #[error("the client's RSA-AES key hash does not match the keys exchanged — the connection was tampered with")]
    Tampered,
}

/// The server's long-lived RSA key: the private half for the exchange and the
/// public half in wire form, which is what gets hashed and fingerprinted.
pub struct ServerKey {
    private: RsaPrivateKey,
    wire: WireKey,
}

impl ServerKey {
    /// A fresh [`SERVER_KEY_BITS`]-bit key. Real CPU time — call it off any
    /// latency-sensitive path.
    pub fn generate() -> Result<Self, rsa::Error> {
        Ok(Self::new(RsaPrivateKey::new(&mut rand::rng(), SERVER_KEY_BITS)?))
    }

    fn new(private: RsaPrivateKey) -> Self {
        let wire = WireKey::of_public(private.as_public_key());
        Self { private, wire }
    }

    /// The key as a PKCS#8 PEM document (`BEGIN PRIVATE KEY`), for the file.
    pub fn to_pem(&self) -> Result<String, rsa::pkcs8::Error> {
        self.private.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).map(|pem| pem.to_string())
    }

    /// A key from the PEM document [`Self::to_pem`] wrote.
    pub fn from_pem(pem: &str) -> Result<Self, rsa::pkcs8::Error> {
        RsaPrivateKey::from_pkcs8_pem(pem).map(Self::new)
    }

    pub fn bits(&self) -> usize {
        self.wire.size() * 8
    }

    /// RealVNC's display of the key, which is what a client shows or logs.
    pub fn fingerprint(&self) -> String {
        self.wire.fingerprint()
    }
}

/// What the client sent as its account: `u8 len || username || u8 len ||
/// password`, each UTF-8 as far as it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// Both directions' ciphers after the exchange, each already advanced past the
/// handshake frames it carried. [`Opener`] holds whatever the client sent beyond
/// the credentials, so wrap it around the same reader.
pub struct Session {
    pub sealer: Sealer,
    pub opener: Opener,
}

/// Run the exchange on a freshly chosen RSA-AES type: key exchange in the
/// clear, then the key hashes, the subtype and the credentials inside frames.
///
/// Returns once the credentials are in hand; checking them and answering with
/// SecurityResult is the caller's, through [`Session::sealer`] like everything
/// after.
pub async fn authenticate<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    strength: Strength,
    key: &ServerKey,
) -> Result<(Credentials, Session), Error> {
    writer.write_all(&key.wire.0).await?;

    let client_wire = read_client_key(reader).await?;
    let client_key = client_wire.public_key()?;
    let client_random = read_client_random(reader, key, strength).await?;

    let mut server_random = vec![0u8; strength.random_len()];
    rand::rng().fill_bytes(&mut server_random);
    let sealed_random = client_key
        .encrypt(&mut rand::rng(), Pkcs1v15Encrypt, &server_random)
        .map_err(|e| Error::Protocol(format!("encrypting the server random to the client's key failed: {e}")))?;
    if sealed_random.len() != client_wire.size() {
        return Err(Error::Protocol(format!(
            "the server random encrypted to {} bytes under a {}-byte client key",
            sealed_random.len(),
            client_wire.size()
        )));
    }
    let mut out = (sealed_random.len() as u16).to_be_bytes().to_vec();
    out.extend_from_slice(&sealed_random);
    writer.write_all(&out).await?;

    let send_key = strength.hash(&[&client_random, &server_random]);
    let recv_key = strength.hash(&[&server_random, &client_random]);
    let key_len = strength.random_len();
    let mut sealer = Sealer::new(strength.cipher(&send_key[..key_len]));
    let mut frames = FrameReader::new(reader, Opener::new(strength.cipher(&recv_key[..key_len])));

    let mut client_hash = vec![0u8; strength.hash(&[]).len()];
    frames.read_exact(&mut client_hash).await?;
    if client_hash != strength.hash(&[&client_wire.0, &key.wire.0]) {
        return Err(Error::Tampered);
    }
    let server_hash = strength.hash(&[&key.wire.0, &client_wire.0]);
    let mut out = sealer.frame(&server_hash);
    out.extend(sealer.frame(&[SUBTYPE_USER_PASS]));
    writer.write_all(&out).await?;

    let username = read_field(&mut frames).await?;
    let password = read_field(&mut frames).await?;
    let (_, opener) = frames.into_parts();
    Ok((Credentials { username, password }, Session { sealer, opener }))
}

async fn read_client_key<R: AsyncRead + Unpin>(reader: &mut R) -> Result<WireKey, Error> {
    let bits = reader.read_u32().await?;
    if !(MIN_CLIENT_KEY_BITS..=MAX_CLIENT_KEY_BITS).contains(&bits) {
        return Err(Error::Protocol(format!(
            "the client's RSA key is {bits} bits; this server accepts {MIN_CLIENT_KEY_BITS} to {MAX_CLIENT_KEY_BITS}"
        )));
    }
    let size = (bits as usize).div_ceil(8);
    let mut wire = vec![0u8; 4 + 2 * size];
    wire[..4].copy_from_slice(&bits.to_be_bytes());
    reader.read_exact(&mut wire[4..]).await?;
    Ok(WireKey(wire))
}

async fn read_client_random<R: AsyncRead + Unpin>(reader: &mut R, key: &ServerKey, strength: Strength) -> Result<Vec<u8>, Error> {
    let len = usize::from(reader.read_u16().await?);
    if len != key.wire.size() {
        return Err(Error::Protocol(format!(
            "the client random is {len} bytes, not the {} of the server key",
            key.wire.size()
        )));
    }
    let mut sealed = vec![0u8; len];
    reader.read_exact(&mut sealed).await?;
    let random = key
        .private
        .decrypt(Pkcs1v15Encrypt, &sealed)
        .map_err(|e| Error::Protocol(format!("decrypting the client random failed: {e}")))?;
    if random.len() != strength.random_len() {
        return Err(Error::Protocol(format!(
            "the client random is {} bytes, not {}",
            random.len(),
            strength.random_len()
        )));
    }
    Ok(random)
}

/// One credential: a byte of length, then that many bytes of text.
async fn read_field<R: AsyncRead + Unpin>(frames: &mut R) -> Result<String, Error> {
    let len = usize::from(frames.read_u8().await?);
    let mut bytes = vec![0u8; len];
    frames.read_exact(&mut bytes).await?;
    String::from_utf8(bytes).map_err(|_| Error::Protocol("a credential is not UTF-8".to_owned()))
}

/// One direction's AES-EAX, at whichever width the type chose.
enum Cipher {
    Aes128(Eax<Aes128>),
    Aes256(Eax<Aes256>),
}

impl Cipher {
    fn seal(&self, nonce: &[u8; 16], aad: &[u8], msg: &[u8]) -> Vec<u8> {
        let payload = Payload { msg, aad };
        let sealed = match self {
            Self::Aes128(c) => c.encrypt(nonce.into(), payload),
            Self::Aes256(c) => c.encrypt(nonce.into(), payload),
        };
        sealed.expect("AES-EAX encryption of an in-memory buffer cannot fail")
    }

    fn open(&self, nonce: &[u8; 16], aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        let payload = Payload { msg: sealed, aad };
        match self {
            Self::Aes128(c) => c.decrypt(nonce.into(), payload),
            Self::Aes256(c) => c.decrypt(nonce.into(), payload),
        }
        .ok()
    }
}

/// The 128-bit little-endian frame counter. Never resets; a frame is numbered
/// once.
fn bump(counter: &mut [u8; 16]) {
    for byte in counter.iter_mut() {
        *byte = byte.wrapping_add(1);
        if *byte != 0 {
            break;
        }
    }
}

/// The server → client direction: takes a message, returns the frames that
/// carry it.
pub struct Sealer {
    cipher: Cipher,
    counter: [u8; 16],
}

impl Sealer {
    fn new(cipher: Cipher) -> Self {
        Self { cipher, counter: [0; 16] }
    }

    /// Frame a message, cutting it at [`MAX_FRAME_BODY`]. An empty message is
    /// no frame at all — nothing has ever needed to send one, and a reader that
    /// receives one has only a counter to advance for it.
    pub fn frame(&mut self, msg: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(msg.len() + (msg.len() / MAX_FRAME_BODY + 1) * (HEADER + TAG));
        for body in msg.chunks(MAX_FRAME_BODY) {
            let header = (body.len() as u16).to_be_bytes();
            let sealed = self.cipher.seal(&self.counter, &header, body);
            bump(&mut self.counter);
            out.extend_from_slice(&header);
            out.extend_from_slice(&sealed);
        }
        out
    }
}

enum Phase {
    /// The two length bytes.
    Len,
    /// `len` bytes of ciphertext and the tag behind them.
    Sealed { len: usize },
}

/// The client → server direction: the cipher, the counter, and whatever has
/// been read but not yet handed on. Separate from the reader it feeds so the
/// handshake can run it over a borrowed socket and the session over the owned
/// one, without a byte falling between — a client sends ClientInit on the heels
/// of its credentials, and it may already be here.
pub struct Opener {
    cipher: Cipher,
    counter: [u8; 16],
    /// The frame in flight: the header while it is being read, then the sealed
    /// body and tag.
    staging: Vec<u8>,
    filled: usize,
    phase: Phase,
    /// Opened bytes not yet handed upward.
    body: Vec<u8>,
    body_pos: usize,
    /// A frame failed, which is terminal: the counter that would decrypt the
    /// next one has moved on from the one that was never accepted.
    failed: bool,
}

impl Opener {
    fn new(cipher: Cipher) -> Self {
        Self {
            cipher,
            counter: [0; 16],
            staging: Vec::new(),
            filled: 0,
            phase: Phase::Len,
            body: Vec::new(),
            body_pos: 0,
            failed: false,
        }
    }

    /// Open the complete frame in `staging`, leaving its body ready to hand on.
    fn accept(&mut self, len: usize) -> io::Result<()> {
        let (header, sealed) = self.staging.split_at(HEADER);
        let body = self
            .cipher
            .open(&self.counter, header, &sealed[..len + TAG])
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "an RSA-AES frame failed its authentication tag"))?;
        bump(&mut self.counter);
        self.body = body;
        self.body_pos = 0;
        Ok(())
    }

    fn poll_read<R: AsyncRead + Unpin>(&mut self, inner: &mut R, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.failed {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::InvalidData, "the RSA-AES transport already failed")));
        }
        loop {
            if self.body_pos < self.body.len() {
                let n = buf.remaining().min(self.body.len() - self.body_pos);
                buf.put_slice(&self.body[self.body_pos..self.body_pos + n]);
                self.body_pos += n;
                return Poll::Ready(Ok(()));
            }
            if buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
            match self.phase {
                Phase::Len => {
                    self.staging.resize(HEADER, 0);
                    let n = ready!(poll_fill(inner, cx, &mut self.staging[self.filled..]))?;
                    if n == 0 {
                        // Between frames a hang-up is an ordinary close.
                        return if self.filled == 0 { Poll::Ready(Ok(())) } else { Poll::Ready(Err(truncated())) };
                    }
                    self.filled += n;
                    if self.filled == HEADER {
                        let len = usize::from(u16::from_be_bytes([self.staging[0], self.staging[1]]));
                        self.staging.resize(HEADER + len + TAG, 0);
                        self.phase = Phase::Sealed { len };
                    }
                }
                Phase::Sealed { len } => {
                    let end = HEADER + len + TAG;
                    let n = ready!(poll_fill(inner, cx, &mut self.staging[self.filled..end]))?;
                    if n == 0 {
                        return Poll::Ready(Err(truncated()));
                    }
                    self.filled += n;
                    if self.filled == end {
                        if let Err(e) = self.accept(len) {
                            self.failed = true;
                            return Poll::Ready(Err(e));
                        }
                        self.filled = 0;
                        self.phase = Phase::Len;
                    }
                }
            }
        }
    }
}

fn poll_fill<R: AsyncRead + Unpin>(inner: &mut R, cx: &mut Context<'_>, dst: &mut [u8]) -> Poll<io::Result<usize>> {
    let mut buf = ReadBuf::new(dst);
    ready!(Pin::new(inner).poll_read(cx, &mut buf))?;
    Poll::Ready(Ok(buf.filled().len()))
}

fn truncated() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "the connection ended inside an RSA-AES frame")
}

/// An [`Opener`] over a reader: the plaintext stream, as the RFB layer reads it.
pub struct FrameReader<R> {
    inner: R,
    opener: Opener,
}

impl<R> FrameReader<R> {
    pub fn new(inner: R, opener: Opener) -> Self {
        Self { inner, opener }
    }

    /// Take the transport apart again, with everything read so far still in
    /// the [`Opener`].
    pub fn into_parts(self) -> (R, Opener) {
        (self.inner, self.opener)
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for FrameReader<R> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        me.opener.poll_read(&mut me.inner, cx, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(strength: Strength) -> (Sealer, Opener) {
        let key = vec![0x42u8; strength.random_len()];
        (Sealer::new(strength.cipher(&key)), Opener::new(strength.cipher(&key)))
    }

    /// A small key for the tests: generation dominates their run time, and the
    /// exchange does not care about the size.
    fn test_key() -> ServerKey {
        ServerKey::new(RsaPrivateKey::new(&mut rand::rng(), 1024).unwrap())
    }

    #[tokio::test]
    async fn frames_round_trip_across_both_widths_and_split_reads() {
        for strength in [Strength::Aes128, Strength::Aes256] {
            let (mut sealer, opener) = pair(strength);
            let big: Vec<u8> = (0..20_000u32).map(|i| (i * 7) as u8).collect();
            let mut wire = sealer.frame(b"hello");
            wire.extend(sealer.frame(&big));
            wire.extend(sealer.frame(b"!"));
            // Three frames for the big message and one each for the others.
            assert_eq!(wire.len(), 5 + big.len() + 1 + 5 * (HEADER + TAG));

            // Delivered a byte at a time, read in odd sizes: still one stream.
            let (mut tx, rx) = tokio::io::duplex(1);
            let feeder = tokio::spawn(async move {
                for b in wire {
                    tx.write_all(&[b]).await.unwrap();
                }
            });
            let mut reader = FrameReader::new(rx, opener);
            let mut got = Vec::new();
            let mut chunk = [0u8; 777];
            loop {
                let n = reader.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&chunk[..n]);
            }
            feeder.await.unwrap();
            let mut want = b"hello".to_vec();
            want.extend(&big);
            want.push(b'!');
            assert_eq!(got, want);
        }
    }

    #[tokio::test]
    async fn a_tampered_frame_closes_the_transport() {
        let (mut sealer, opener) = pair(Strength::Aes128);
        let mut wire = sealer.frame(b"first");
        wire.extend(sealer.frame(b"second"));
        wire[HEADER + 1] ^= 1;
        let mut reader = FrameReader::new(std::io::Cursor::new(wire), opener);
        let mut buf = [0u8; 16];
        let err = reader.read(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{err}");
        let err = reader.read(&mut buf).await.unwrap_err();
        assert!(err.to_string().contains("already failed"), "{err}");
    }

    #[tokio::test]
    async fn a_frame_is_numbered_once() {
        let (mut sealer, opener) = pair(Strength::Aes256);
        let first = sealer.frame(b"once");
        let mut wire = first.clone();
        wire.extend(&first);
        let mut reader = FrameReader::new(std::io::Cursor::new(wire), opener);
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"once");
        assert!(reader.read(&mut buf).await.is_err());
    }

    #[test]
    fn the_counter_is_little_endian_with_carry() {
        let mut c = [0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        bump(&mut c);
        assert_eq!(c, [0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn wire_key_pads_and_fingerprints_like_realvnc() {
        let key = WireKey::encode(1024, &[0, 0, 0x01, 0x02], &[0x01, 0x00, 0x01]);
        assert_eq!(key.0.len(), 4 + 256);
        assert_eq!(&key.0[..4], &[0, 0, 4, 0]);
        assert_eq!(key.size(), 128);
        assert_eq!(&key.modulus()[126..], &[0x01, 0x02]);
        assert_eq!(&key.exponent()[125..], &[0x01, 0x00, 0x01]);
        let fp = key.fingerprint();
        assert_eq!(fp.len(), 8 * 2 + 7, "{fp}");
        assert_eq!(fp.matches('-').count(), 7, "{fp}");
    }

    #[test]
    fn a_key_survives_the_pem_round_trip_with_its_fingerprint() {
        let key = test_key();
        let pem = key.to_pem().unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----\n"), "{pem}");
        let back = ServerKey::from_pem(&pem).unwrap();
        assert_eq!(back.fingerprint(), key.fingerprint());
        assert_eq!(back.bits(), 1024);
        assert!(ServerKey::from_pem("-----BEGIN PRIVATE KEY-----\nbm9wZQ==\n-----END PRIVATE KEY-----\n").is_err());
    }

    /// The client side of the exchange, written from the specification rather
    /// than by calling the server's helpers, so a mistake in one cannot be
    /// agreed with by the other. Sends the credentials, then reads the
    /// SecurityResult and a message behind it and echoes a message back.
    async fn scripted_client<S: AsyncRead + AsyncWrite + Unpin>(
        mut sock: S,
        strength: Strength,
        username: &str,
        password: &str,
        lie_in_hash: bool,
    ) -> Result<(u32, Vec<u8>), io::Error> {
        let client_key = RsaPrivateKey::new(&mut rand::rng(), 1024).unwrap();
        let bits = client_key.n().bits();
        let size = (bits as usize).div_ceil(8);
        let mut client_wire = bits.to_be_bytes().to_vec();
        client_wire.extend(left_pad(&client_key.n_bytes(), size));
        client_wire.extend(left_pad(&client_key.e_bytes(), size));

        let server_bits = sock.read_u32().await?;
        let server_size = (server_bits as usize).div_ceil(8);
        let mut server_wire = server_bits.to_be_bytes().to_vec();
        server_wire.resize(4 + 2 * server_size, 0);
        sock.read_exact(&mut server_wire[4..]).await?;
        let server_key = RsaPublicKey::new(
            BoxedUint::from_be_slice_vartime(&server_wire[4..4 + server_size]),
            BoxedUint::from_be_slice_vartime(&server_wire[4 + server_size..]),
        )
        .unwrap();

        let random_len = strength.random_len();
        let mut client_random = vec![0u8; random_len];
        rand::rng().fill_bytes(&mut client_random);
        let sealed = server_key.encrypt(&mut rand::rng(), Pkcs1v15Encrypt, &client_random).unwrap();
        let mut out = client_wire.clone();
        out.extend((sealed.len() as u16).to_be_bytes());
        out.extend(&sealed);
        sock.write_all(&out).await?;

        let len = usize::from(sock.read_u16().await?);
        assert_eq!(len, size);
        let mut sealed = vec![0u8; len];
        sock.read_exact(&mut sealed).await?;
        let server_random = client_key.decrypt(Pkcs1v15Encrypt, &sealed).unwrap();
        assert_eq!(server_random.len(), random_len);

        // The client's send key is H(server || client); its receive key the reverse.
        let send = strength.hash(&[&server_random, &client_random]);
        let recv = strength.hash(&[&client_random, &server_random]);
        let mut sealer = Sealer::new(strength.cipher(&send[..random_len]));
        let mut client_hash = strength.hash(&[&client_wire, &server_wire]);
        if lie_in_hash {
            client_hash[0] ^= 1;
        }
        sock.write_all(&sealer.frame(&client_hash)).await?;

        let mut frames = FrameReader::new(&mut sock, Opener::new(strength.cipher(&recv[..random_len])));
        let mut server_hash = vec![0u8; client_hash.len()];
        frames.read_exact(&mut server_hash).await?;
        assert_eq!(server_hash, strength.hash(&[&server_wire, &client_wire]));
        assert_eq!(frames.read_u8().await?, SUBTYPE_USER_PASS);

        let (sock, opener) = frames.into_parts();
        let mut credentials = vec![username.len() as u8];
        credentials.extend(username.as_bytes());
        credentials.push(password.len() as u8);
        credentials.extend(password.as_bytes());
        sock.write_all(&sealer.frame(&credentials)).await?;

        let mut frames = FrameReader::new(sock, opener);
        let result = frames.read_u32().await?;
        let mut after = [0u8; 5];
        frames.read_exact(&mut after).await?;
        let (sock, _) = frames.into_parts();
        sock.write_all(&sealer.frame(b"echo")).await?;
        Ok((result, after.to_vec()))
    }

    async fn exchange(strength: Strength, username: &str, password: &str, lie_in_hash: bool) -> (Result<Credentials, Error>, Result<(u32, Vec<u8>), io::Error>) {
        let key = test_key();
        let (client_sock, server_sock) = tokio::io::duplex(4096);
        let username = username.to_owned();
        let password = password.to_owned();
        let client = tokio::spawn(async move { scripted_client(client_sock, strength, &username, &password, lie_in_hash).await });

        // The halves live inside the block, so a server that gives up drops them
        // and the client reads a hang-up instead of waiting for a hash forever.
        let server = async {
            let (mut reader, mut writer) = tokio::io::split(server_sock);
            let (credentials, Session { mut sealer, opener }) = authenticate(&mut reader, &mut writer, strength, &key).await?;
            // SecurityResult and a message, both inside one frame, then an echo back.
            let mut after = 0u32.to_be_bytes().to_vec();
            after.extend_from_slice(b"after");
            writer.write_all(&sealer.frame(&after)).await?;
            let mut frames = FrameReader::new(reader, opener);
            let mut echo = [0u8; 4];
            frames.read_exact(&mut echo).await?;
            assert_eq!(&echo, b"echo");
            Ok(credentials)
        }
        .await;
        (server, client.await.unwrap())
    }

    #[tokio::test]
    async fn the_exchange_authenticates_against_a_client_written_from_the_specification() {
        for strength in [Strength::Aes128, Strength::Aes256] {
            let (server, client) = exchange(strength, "andrew", "hunter2", false).await;
            let credentials = server.unwrap();
            assert_eq!(credentials.username, "andrew");
            assert_eq!(credentials.password, "hunter2");
            let (result, after) = client.unwrap();
            assert_eq!(result, 0);
            assert_eq!(after, b"after");
        }
    }

    #[tokio::test]
    async fn a_client_hash_over_other_keys_ends_the_exchange() {
        let (server, client) = exchange(Strength::Aes256, "andrew", "hunter2", true).await;
        assert!(matches!(server, Err(Error::Tampered)), "{server:?}");
        assert!(client.is_err());
    }

    #[tokio::test]
    async fn a_client_key_outside_the_bounds_is_refused_after_the_server_key() {
        let key = test_key();
        let mut offer = 512u32.to_be_bytes().to_vec();
        offer.resize(4 + 128, 1);
        let mut sent = Vec::new();
        let err = authenticate(&mut offer.as_slice(), &mut sent, Strength::Aes128, &key).await.err().expect("a 512-bit key is refused");
        assert!(err.to_string().contains("512 bits"), "{err}");
        // The server's key went out first, as the exchange has it, and nothing else.
        assert_eq!(sent.len(), 4 + 2 * 128);
    }
}
