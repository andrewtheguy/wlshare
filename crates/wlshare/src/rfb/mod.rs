//! The RFB wire wlshare speaks.
//!
//! Everything that decides bytes on the socket lives here, or for a private
//! extension in [`wlshare_proto`], and nowhere else: the handshake pieces, the
//! client messages and how they parse, the server messages and how they are
//! built, RSA-AES, and the encoders. The rest of the daemon turns compositor
//! events into calls on this module and copies the results to sockets; it
//! writes no protocol byte of its own.
//!
//! Nothing here touches an OS API. The audio extension's FLAC encoder is
//! `sound-flac`'s own, in plain Rust, and its Opus encoder libopus from a
//! prebuilt static archive, as libvpx is under `wlshare-vp9`. Tests here use an
//! independent decoder for every encoder, so the two halves cannot share a
//! misunderstanding.
//!
//! ## What is on the wire, and what is not
//!
//! RFB 3.8 as RFC 6143 has it, with these choices:
//!
//! - **Raw or ZRLE, and VP9 for a client that lists it.** ZRLE is the standard's
//!   best lossless encoding, and the zDesk gateway lists it as its first
//!   standard pixel encoding. **The VP9 encoding** is a private one for the
//!   gateway: the whole framebuffer as one VP9 stream, 4:4:4 or 4:2:0, at a
//!   quality its owner sets ([`vp9`]), for a desktop that moves; **the tiles
//!   encoding**, another for the gateway, the desktop as exact PNG tiles where
//!   it is still and a video stream for each region that moves, for a link
//!   with room to spare ([`tiles`]). Raw is produced for a client whose list
//!   names neither. Tight, Hextile, RRE, CopyRect and every other lossy
//!   encoding are absent.
//! - **32-bit true colour only.** The server's native format is the compositor's
//!   XRGB8888, which is the `B, G, R, X` byte order the gateway forces. A client may
//!   ask for any 32-bit true-colour format with 8-bit channels and gets it by a
//!   swizzle; 8- and 16-bit formats and colour maps are refused.
//! - **ContinuousUpdates and Fence**, so a client that supports them gets frames
//!   as the screen changes with one pixel update in flight, instead of polling.
//! - **Cursor**, required of every client, so the compositor pointer stays out
//!   of captured pixels and its shape moves immediately at the client; and
//!   **Cursor With Alpha**, for a client that lists it, so the shape keeps its
//!   shadow and edges ([`cursor`]).
//! - **DesktopSize and ExtendedDesktopSize**, so a client may resize the desktop.
//! - **wlshare's private extensions**: the density, outputs, audio, camera
//!   and microphone extensions and the scroll message, whose bytes are
//!   [`wlshare_proto`]'s, the crate the zDesk gateway reads and writes them
//!   with too. What is here is what the server does with them: the audio
//!   extension's encoders ([`audio`]), and the client messages parsed among
//!   the standard ones ([`msg`]).
//! - **Extended Clipboard**, the registered one and the only clipboard spoken:
//!   text as UTF-8, notified and then asked for ([`clipboard`]). Latin-1 cut
//!   text is framed so that it can be skipped, and otherwise ignored.
//! - **None and RSA-AES** for security. The first is RFC 6143's; the second is
//!   RealVNC's, and the one way a client can name an account and have the
//!   session encrypted ([`rsa_aes`]). Classic VncAuth is deliberately absent: it
//!   names nobody and encrypts nothing after the login. Which is offered is the
//!   daemon's configuration; this module speaks both.

pub mod audio;
// The client's half, which the tests read the server's bytes with and the
// examples connect with; nothing in the daemon calls it.
#[allow(dead_code)]
pub mod client;
pub mod clipboard;
pub mod cursor;
pub mod msg;
pub mod pixel;
pub mod rsa_aes;
pub mod tiles;
pub mod vp9;
pub mod zrle;

/// Raw: pixels as they are, in the client's format. RFC 6143 §7.7.1 requires
/// every server to produce it until asked for something else.
pub const ENCODING_RAW: i32 = 0;
/// ZRLE, the encoding this server chooses when the client lists it and neither
/// [`wlshare_proto::ENCODING_VP9`] nor [`wlshare_proto::ENCODING_TILES`].
pub const ENCODING_ZRLE: i32 = 16;
/// DesktopSize pseudo-encoding: a rectangle announcing the framebuffer's new size.
pub const ENCODING_DESKTOP_SIZE: i32 = -223;
/// Cursor pseudo-encoding. Every client must list it, because captured frames
/// contain no pointer: the compositor's cursor image goes out as [`cursor`]
/// has it.
pub const ENCODING_CURSOR: i32 = -239;
/// Cursor With Alpha pseudo-encoding: a client that lists it is sent the cursor
/// image with its alpha, instead of cut to a mask.
pub const ENCODING_CURSOR_WITH_ALPHA: i32 = -314;
/// ExtendedDesktopSize pseudo-encoding, and with it the client's SetDesktopSize.
pub const ENCODING_EXTENDED_DESKTOP_SIZE: i32 = -308;
/// Fence pseudo-encoding: the client will echo markers the server sends.
pub const ENCODING_FENCE: i32 = -312;
/// ContinuousUpdates pseudo-encoding: the client understands
/// EndOfContinuousUpdates and may enable continuous updates.
pub const ENCODING_CONTINUOUS_UPDATES: i32 = -313;
/// Extended Clipboard pseudo-encoding: the clipboard as UTF-8 ([`clipboard`]).
pub const ENCODING_EXTENDED_CLIPBOARD: i32 = 0xc0a1_e5ce_u32 as i32;
