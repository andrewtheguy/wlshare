//! The density extension: how a client learns what scale the framebuffer is
//! drawn at, and asks for the one it wants.
//!
//! Standard RFB carries pixels and nothing else, so a compositor output at
//! `scale 2` is indistinguishable from one twice the size at `scale 1`. This
//! private extension adds one pseudo-encoding and one message type, in both
//! directions:
//!
//! - The client lists [`crate::ENCODING_DENSITY`] in `SetEncodings`. A server that
//!   does not know it ignores it, as RFB requires.
//! - The server answers **every** such `SetEncodings` with an `OutputScale`
//!   ([`output_scale`]) — that answer is the only way support is ever
//!   announced — and sends another whenever the captured output's scale or size
//!   changes, *before* the resize rectangle that carries the new framebuffer.
//! - The client may send a `ClientDensity` ([`client_density`]) naming the
//!   density it would like the output drawn at and the size in pixels it wants
//!   at that density — both, every time, so a change of density is one
//!   reconfiguration of the output rather than a scale and then a mode. The
//!   server sets the output's mode and scale to them together where it may, and
//!   answers every declaration with an `OutputScale`: after the change, or at
//!   once with the output as it is when nothing is to be changed or nothing can
//!   be. A declaration is never left without an answer. A resize at an unchanged
//!   density is standard RFB's `SetDesktopSize`.
//!
//! The two messages have one layout ([`Density`]). Scales are 16.16 unsigned
//! fixed point: `0x0002_0000` is 2.0, `0x0001_8000` is 1.5. The server sends
//! the compositor's exact value, fractional included.

/// The message type, used by both directions; outside every registered type.
pub const MSG_DENSITY: u8 = 0xE0;

/// The bytes of an `OutputScale` and of a `ClientDensity`, type included.
pub const DENSITY_LEN: usize = 10;

/// A scale as 16.16 fixed point.
pub fn to_fixed(scale: f64) -> u32 {
    (scale * 65536.0).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// A 16.16 fixed-point scale as a number.
pub fn from_fixed(fixed: u32) -> f64 {
    f64::from(fixed) / 65536.0
}

/// Either message: a framebuffer's size in pixels and the scale it is drawn at.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE0` |
/// | 1 | U8 | padding |
/// | 2 | U16 | width, pixels |
/// | 4 | U16 | height, pixels |
/// | 6 | U32 | scale, 16.16 fixed |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Density {
    pub width: u16,
    pub height: u16,
    /// The scale, 16.16 fixed ([`from_fixed`]).
    pub fixed: u32,
}

impl Density {
    /// The message's bytes.
    pub fn to_bytes(self) -> [u8; DENSITY_LEN] {
        let mut msg = [0u8; DENSITY_LEN];
        msg[0] = MSG_DENSITY;
        msg[2..4].copy_from_slice(&self.width.to_be_bytes());
        msg[4..6].copy_from_slice(&self.height.to_be_bytes());
        msg[6..10].copy_from_slice(&self.fixed.to_be_bytes());
        msg
    }

    /// The message at the front of `buf`, whose first byte is [`MSG_DENSITY`],
    /// and its length, or `None` while it is not all there.
    pub fn parse(buf: &[u8]) -> Option<(Self, usize)> {
        debug_assert_eq!(buf.first(), Some(&MSG_DENSITY));
        if buf.len() < DENSITY_LEN {
            return None;
        }
        Some((Self { width: crate::u16_at(buf, 2), height: crate::u16_at(buf, 4), fixed: crate::u32_at(buf, 6) }, DENSITY_LEN))
    }
}

/// Server → client `OutputScale`: the framebuffer's size in pixels and the scale
/// it is drawn at.
pub fn output_scale(width: u16, height: u16, scale: f64) -> [u8; DENSITY_LEN] {
    Density { width, height, fixed: to_fixed(scale) }.to_bytes()
}

/// Client → server `ClientDensity`: the size in pixels the client wants, and the
/// scale it wants them drawn at.
pub fn client_density(width: u16, height: u16, scale: f64) -> [u8; DENSITY_LEN] {
    Density { width, height, fixed: to_fixed(scale) }.to_bytes()
}

#[cfg(test)]
mod tests;
