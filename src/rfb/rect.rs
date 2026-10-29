// A file of its own so that a tool reading the capture file, which names it,
// can take it without the rest of the module (`examples/wlcap.rs`).

/// A rectangle of a picture, in pixels: where it differs from the picture
/// read before it, for an encoder told its damage. What lies past the
/// picture's edge is not part of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}
