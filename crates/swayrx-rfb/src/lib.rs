//! The RFB wire swayrx speaks.
//!
//! Everything that decides bytes on the socket lives here and nowhere else: the
//! handshake pieces, the client messages and how they parse, the server messages
//! and how they are built, classic VNC authentication, the ZRLE encoder and the
//! density extension. The daemon crate turns compositor events into calls on this
//! crate and copies the results to sockets; it never writes a protocol byte of
//! its own.
//!
//! The crate is platform-independent so that all of it is unit-tested on any
//! machine, the daemon being buildable only where libwayland and libxkbcommon
//! are. Tests here use an independent decoder for every encoder, so the two
//! halves cannot share a misunderstanding.
//!
//! ## What is on the wire, and what is not
//!
//! RFB 3.8 as RFC 6143 has it, with these choices:
//!
//! - **One pixel encoding, ZRLE.** It is the standard's best lossless encoding,
//!   every client worth naming decodes it, and the remotex gateway asks for it
//!   first. Raw is produced only before the client's first `SetEncodings`, where
//!   the RFC requires it, and for a client whose list never names ZRLE. Tight,
//!   Hextile, RRE, CopyRect and every lossy encoding are absent.
//! - **32-bit true colour only.** The server's native format is the compositor's
//!   XRGB8888, which is the `B, G, R, X` byte order the gateway forces. A client may
//!   ask for any 32-bit true-colour format with 8-bit channels and gets it by a
//!   swizzle; 8- and 16-bit formats and colour maps are refused.
//! - **ContinuousUpdates and Fence**, so a client that supports them gets frames
//!   as the screen changes with one update in flight, instead of polling.
//! - **DesktopSize and ExtendedDesktopSize**, so a client may resize the desktop.
//! - **The density extension**, one pseudo-encoding and one message type, which
//!   is how a client learns the scale the framebuffer is drawn at and asks for
//!   the one it wants ([`density`]).
//! - **None, VncAuth, and RSA-AES** for security. The first two are RFC 6143's;
//!   the third is RealVNC's, and the one way a client can name an account and
//!   have the session encrypted ([`rsa_aes`]). Which is offered is the daemon's
//!   configuration; the crate speaks all three.

pub mod auth;
pub mod density;
pub mod msg;
pub mod pixel;
pub mod rsa_aes;
pub mod zrle;

/// Raw: pixels as they are, in the client's format. RFC 6143 §7.7.1 requires
/// every server to produce it until asked for something else.
pub const ENCODING_RAW: i32 = 0;
/// ZRLE, the one encoding this server chooses when the client lists it.
pub const ENCODING_ZRLE: i32 = 16;
/// DesktopSize pseudo-encoding: a rectangle announcing the framebuffer's new size.
pub const ENCODING_DESKTOP_SIZE: i32 = -223;
/// LastRect pseudo-encoding: a client that lists it accepts an update whose
/// rectangle count is a ceiling rather than a promise.
pub const ENCODING_LAST_RECT: i32 = -224;
/// Cursor pseudo-encoding. Not produced: the pointer is composited into the
/// framebuffer instead, so this is recognised only to be ignored.
pub const ENCODING_CURSOR: i32 = -239;
/// ExtendedDesktopSize pseudo-encoding, and with it the client's SetDesktopSize.
pub const ENCODING_EXTENDED_DESKTOP_SIZE: i32 = -308;
/// Fence pseudo-encoding: the client will echo markers the server sends.
pub const ENCODING_FENCE: i32 = -312;
/// ContinuousUpdates pseudo-encoding: the client understands
/// EndOfContinuousUpdates and may enable continuous updates.
pub const ENCODING_CONTINUOUS_UPDATES: i32 = -313;
/// Extended Clipboard pseudo-encoding. Recognised, not yet spoken.
pub const ENCODING_EXTENDED_CLIPBOARD: i32 = 0xc0a1_e5ce_u32 as i32;
/// The density extension's pseudo-encoding, the ASCII bytes `SWRX`.
pub const ENCODING_DENSITY: i32 = 0x5357_5258;
