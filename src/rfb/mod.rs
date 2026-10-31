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
//! prebuilt static archive. The H.264 the video and tiles encodings code with
//! is the daemon's, handed in as an [`H264Coder`]. Tests here use an
//! independent decoder for every encoder, so the two halves cannot share a
//! misunderstanding.
//!
//! ## What is on the wire, and what is not
//!
//! RFB 3.8 as RFC 6143 has it, with these choices:
//!
//! - **Raw or ZRLE, and the video encoding for a client that lists it.** ZRLE
//!   is the standard's best lossless encoding, and the zDesk gateway lists it
//!   as its first standard pixel encoding. **The video encoding** is a private
//!   one for the gateway: the whole framebuffer as one H.264 stream, at a
//!   quality its owner sets ([`video`]), for a desktop that moves; **the tiles
//!   encoding**, another for the gateway, the desktop as lossless PNG tiles where
//!   it is still and a video stream for each region that moves, for a link
//!   with room to spare ([`tiles`]), or every tile RemoteFX Progressive, still
//!   or moving ([`progressive`]). Raw is produced for a client whose list
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
pub mod planar;
pub mod progressive;
mod rect;
pub mod rsa_aes;
pub mod tiles;
pub mod video;
pub mod zrle;

pub use rect::Rect;

/// Raw: pixels as they are, in the client's format. RFC 6143 §7.7.1 requires
/// every server to produce it until asked for something else.
pub const ENCODING_RAW: i32 = 0;
/// ZRLE, the encoding this server chooses when the client lists it and neither
/// [`wlshare_proto::ENCODING_VIDEO`] nor [`wlshare_proto::ENCODING_TILES`].
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

/// The coarsest end of the 1–100 quality dial the encoders take, finest last.
pub const QUALITY_MIN: u8 = 1;
/// The finest end of the dial.
pub const QUALITY_MAX: u8 = 100;

/// The dial wlshare codes at: the ceiling the video stream and a walked tiles
/// picture's first passes are walked down from, and a held tiles picture's
/// streams' quality. The server's alone — no client names one — so there is one
/// way to code each picture. 90 is QP 18 in H.264, where a desktop's text
/// reads sharp in motion.
pub const QUALITY: u8 = 90;

/// Why an H.264 coder could not open or code a stream: its encoder's reason,
/// in its own words.
pub type H264Error = Box<dyn std::error::Error + Send + Sync>;

/// The H.264 coder the daemon hands the video encoding's encoder and the
/// tiles encoding's ([`video::VideoEncoder`], [`tiles::TilesEncoder`]): its
/// host's encoder, the hardware's where the host has one and libx264
/// otherwise, each frame one Annex B access unit, the parameter sets ahead of
/// each keyframe's slices, 8-bit 4:2:0 BT.601 at studio swing, the picture
/// the stream's size rounded up to even sides. The video encoding opens a
/// stream of it over the whole desktop, the tiles one for each region that
/// moves, and a failure to open or to code is its encode's.
pub trait H264Coder: Send {
    /// A stream over a `width`×`height` picture, which codes nothing until
    /// read into.
    fn open(&mut self, width: u16, height: u16) -> Result<Box<dyn H264Stream>, H264Error>;
}

/// One stream as an [`H264Coder`] codes it: read, then encode.
pub trait H264Stream: Send {
    /// Read `pixels` — `B, G, R, X`, the stream's size, rows `stride` bytes
    /// apart — as the picture the next frame carries, where `changed` says
    /// it differs from the read before, or anywhere for `None`; the first
    /// read is the whole picture whatever `changed` says. Only the rectangles
    /// are read, widened to the 2×2 blocks they touch, so the rest of
    /// `pixels` need not be the picture.
    fn read_bgrx(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> Result<(), H264Error>;
    /// Encode the picture as last read at `quality`, the 1–100 dial, and
    /// append the frame to `out`, a keyframe where `keyframe` asks for one and
    /// for the stream's first either way: `true` for a frame appended, and
    /// `false` where the coder made none of the picture, nothing appended, and
    /// the whole picture owed to the next frame. The quality is each frame's
    /// own, so a stream follows the walk with no keyframe.
    fn encode(&mut self, keyframe: bool, quality: u8, out: &mut Vec<u8>) -> Result<bool, H264Error>;
}
