//! The scroll message: how a client scrolls by a distance.
//!
//! Standard RFB has no scroll magnitude. A wheel is buttons 4 to 7 of the
//! pointer mask, and all a client can vary is how many times it presses one, so
//! a touchpad glide or two fingers on a phone arrive as whole notches or not at
//! all. The compositor's own vocabulary is richer: a `wl_pointer` axis carries a
//! distance, which is what a touchpad plugged into the machine sends.
//!
//! This private message is that distance. It has no pseudo-encoding, because the
//! server holds no state for it and has nothing to answer: a client that knows
//! the server is wlshare sends a [`scroll`] where it would have pressed a wheel
//! button, after the `PointerEvent` that says where. The wheel buttons stay what
//! they are, a notch apiece, for a client that has only notches to send.
//!
//! The distance is in the output's logical pixels, the unit an axis is in and
//! the one a desktop's applications scroll by — not framebuffer pixels, which
//! are that many times the output's scale. Positive is to the right and
//! downward.

/// The message type; outside every registered type.
pub const MSG_SCROLL: u8 = 0xE5;

/// The bytes of a `Scroll`, type included.
pub const SCROLL_LEN: usize = 6;

/// Client → server `Scroll`: scroll by `dx` and `dy` logical pixels, where the
/// pointer is.
///
/// | Offset | Type | Field |
/// |---|---|---|
/// | 0 | U8 | `0xE5` |
/// | 1 | U8 | padding |
/// | 2 | S16 | horizontal distance, logical pixels, positive rightward |
/// | 4 | S16 | vertical distance, logical pixels, positive downward |
pub fn scroll(dx: i16, dy: i16) -> [u8; SCROLL_LEN] {
    let [x0, x1] = dx.to_be_bytes();
    let [y0, y1] = dy.to_be_bytes();
    [MSG_SCROLL, 0, x0, x1, y0, y1]
}

/// The `Scroll` at the front of `buf`, whose first byte is [`MSG_SCROLL`]: its
/// distances and its length, or `None` while it is not all there.
pub fn parse(buf: &[u8]) -> Option<((i16, i16), usize)> {
    debug_assert_eq!(buf.first(), Some(&MSG_SCROLL));
    if buf.len() < SCROLL_LEN {
        return None;
    }
    Some(((crate::i16_at(buf, 2), crate::i16_at(buf, 4)), SCROLL_LEN))
}

#[cfg(test)]
mod tests;
