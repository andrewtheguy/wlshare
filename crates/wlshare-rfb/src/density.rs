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
//! - The server answers **every** such `SetEncodings` with an [`output_scale`]
//!   message — that answer is the only way support is ever announced — and sends
//!   another whenever the captured output's scale or size changes, *before* the
//!   resize rectangle that carries the new framebuffer.
//! - The client may send a `ClientDensity` ([`crate::msg::ClientMsg::ClientDensity`])
//!   naming the density it would like the output drawn at. The server sets the
//!   output's scale to it where it may, and answers every declaration with an
//!   `OutputScale`: after the change, or at once with the scale as it is when
//!   nothing is to be changed or nothing can be. A declaration is never left
//!   without an answer.
//!
//! Scales are 16.16 unsigned fixed point: `0x0002_0000` is 2.0, `0x0001_8000`
//! is 1.5. The server sends the compositor's exact value, fractional included.

/// The message type, used by both directions; outside every registered type.
pub const MSG_DENSITY: u8 = 0xE0;

/// The bytes of a `ClientDensity`, type included.
pub const CLIENT_DENSITY_LEN: usize = 8;

/// A scale as 16.16 fixed point.
pub fn to_fixed(scale: f64) -> u32 {
    (scale * 65536.0).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// A 16.16 fixed-point scale as a number.
pub fn from_fixed(fixed: u32) -> f64 {
    f64::from(fixed) / 65536.0
}

/// Server → client `OutputScale`: the framebuffer's size in pixels and the scale
/// it is drawn at.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE0` |
/// | 1 | U8 | padding |
/// | 2 | U16 | width, pixels |
/// | 4 | U16 | height, pixels |
/// | 6 | U32 | scale, 16.16 fixed |
pub fn output_scale(width: u16, height: u16, scale: f64) -> [u8; 10] {
    let mut msg = [0u8; 10];
    msg[0] = MSG_DENSITY;
    msg[2..4].copy_from_slice(&width.to_be_bytes());
    msg[4..6].copy_from_slice(&height.to_be_bytes());
    msg[6..10].copy_from_slice(&to_fixed(scale).to_be_bytes());
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_carries_fractions_exactly() {
        assert_eq!(to_fixed(2.0), 0x0002_0000);
        assert_eq!(to_fixed(1.5), 0x0001_8000);
        assert_eq!(from_fixed(0x0001_8000), 1.5);
    }

    #[test]
    fn output_scale_has_the_documented_layout() {
        assert_eq!(
            output_scale(3456, 1802, 2.0),
            [0xE0, 0, 0x0D, 0x80, 0x07, 0x0A, 0x00, 0x02, 0x00, 0x00]
        );
    }
}
