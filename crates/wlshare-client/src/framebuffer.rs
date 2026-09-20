//! The picture the session decodes into and the window draws from.
//!
//! One buffer, `B, G, R, X` — [`PixelFormat::NATIVE`], which is what the server
//! sends when asked for its own format, and what the apps upload as it is: a
//! Metal `bgra8Unorm` texture on the Mac, a Win2D `B8G8R8A8UIntNormalized`
//! bitmap with its alpha ignored on Windows. Nothing swizzles anywhere on this
//! path.
//!
//! The session writes it under a lock and the window reads it under the same
//! one, so the two are never in it together. What the window needs to know
//! beyond the pixels is [`Framebuffer::generation`], which changes when the
//! buffer is a different size and its texture has to be made again, and the
//! damage, which is the region to upload when it is not.

use anyhow::{bail, Context};
use wlshare_rfb::pixel::PixelFormat;

/// The most a framebuffer may take, which is 16384x16384 — twice an 8K
/// display each way. The size is the server's to say, and a u16 each way would
/// otherwise let it ask for 16 GiB and end the viewer with a failed allocation.
const MAX_BYTES: usize = 1 << 30;

/// A region of the framebuffer, in pixels from its top left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self {
            x,
            y,
            width: (self.x + self.width).max(other.x + other.width) - x,
            height: (self.y + self.height).max(other.y + other.height) - y,
        }
    }
}

pub struct Framebuffer {
    pixels: Vec<u8>,
    width: u16,
    height: u16,
    /// Everything written since the window last took it, as one rectangle.
    /// Tracking each rectangle separately would upload less, and measurably
    /// less only for a desktop whose damage is two far-apart specks; the merge
    /// the server already does makes that the uncommon shape.
    damage: Option<Region>,
    generation: u64,
}

impl Framebuffer {
    /// A black framebuffer of this size. A zero dimension is a framebuffer with
    /// no pixels, which is what a session has before its ServerInit.
    pub fn new(width: u16, height: u16) -> Self {
        Self { pixels: vec![0; Self::len(width, height)], width, height, damage: None, generation: 0 }
    }

    fn len(width: u16, height: u16) -> usize {
        usize::from(width) * usize::from(height) * 4
    }

    /// The format the pixels are in, which is the server's own.
    pub const FORMAT: PixelFormat = PixelFormat::NATIVE;

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Bytes between one row's first pixel and the next's.
    pub fn stride(&self) -> usize {
        usize::from(self.width) * 4
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Which framebuffer this is. A window that saw a different number last
    /// time is looking at a new size and must make its texture again.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Become a black framebuffer of a new size, or do nothing if that is the
    /// size already. The whole of a new one is damaged: the server repaints it
    /// after announcing it, but the window may draw in between. A size over
    /// [`MAX_BYTES`], or one there is not the memory for, is an error and
    /// leaves the framebuffer as it was.
    pub fn resize(&mut self, width: u16, height: u16) -> anyhow::Result<()> {
        if (width, height) == (self.width, self.height) {
            return Ok(());
        }
        let len = Self::len(width, height);
        if len > MAX_BYTES {
            bail!("a {width}x{height} desktop is larger than this viewer takes");
        }
        let mut pixels = Vec::new();
        pixels.try_reserve_exact(len).with_context(|| format!("allocating a {width}x{height} framebuffer"))?;
        pixels.resize(len, 0);
        self.pixels = pixels;
        self.width = width;
        self.height = height;
        self.generation += 1;
        self.damage = None;
        self.damage_all();
        Ok(())
    }

    /// The rectangle's bytes and the stride they are written at, for a decoder
    /// to fill, and `None` when the rectangle is not inside the framebuffer —
    /// a server's rectangle is checked here rather than trusted.
    pub fn rect_mut(&mut self, x: u16, y: u16, width: u16, height: u16) -> Option<(&mut [u8], usize)> {
        if u32::from(x) + u32::from(width) > u32::from(self.width) || u32::from(y) + u32::from(height) > u32::from(self.height) {
            return None;
        }
        let stride = self.stride();
        let start = usize::from(y) * stride + usize::from(x) * 4;
        // `get_mut` rather than an index: a zero-sized rectangle can begin one
        // pixel past the last one and still pass the check above.
        Some((self.pixels.get_mut(start..)?, stride))
    }

    /// Take a rectangle's pixels, rows top down and packed, as Raw sends them.
    pub fn put_raw(&mut self, x: u16, y: u16, width: u16, height: u16, raw: &[u8]) -> bool {
        let row = usize::from(width) * 4;
        if raw.len() != row * usize::from(height) {
            return false;
        }
        // Nothing to draw, and `chunks_exact` panics on a row of zero bytes.
        if width == 0 || height == 0 {
            return true;
        }
        let Some((out, stride)) = self.rect_mut(x, y, width, height) else { return false };
        for (n, line) in raw.chunks_exact(row).enumerate() {
            out[n * stride..n * stride + row].copy_from_slice(line);
        }
        self.damage_rect(x, y, width, height);
        true
    }

    /// Note that a rectangle has been written.
    pub fn damage_rect(&mut self, x: u16, y: u16, width: u16, height: u16) {
        if width == 0 || height == 0 {
            return;
        }
        let region = Region { x: u32::from(x), y: u32::from(y), width: u32::from(width), height: u32::from(height) };
        self.damage = Some(match self.damage {
            Some(had) => had.union(region),
            None => region,
        });
    }

    pub fn damage_all(&mut self) {
        self.damage_rect(0, 0, self.width, self.height);
    }

    /// The damage since this was last called, and clear it.
    pub fn take_damage(&mut self) -> Option<Region> {
        self.damage.take()
    }

    /// Whether anything written is still waiting for the window to take it.
    /// What the session holds a fence for: no damage is everything decoded
    /// already on its way to the screen.
    pub fn has_damage(&self) -> bool {
        self.damage.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(fb: &Framebuffer, x: usize, y: usize) -> [u8; 4] {
        let at = y * fb.stride() + x * 4;
        fb.pixels()[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn a_raw_rectangle_lands_where_it_was_addressed() {
        let mut fb = Framebuffer::new(4, 3);
        let raw = [1u8, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0, 10, 11, 12, 0];
        assert!(fb.put_raw(1, 1, 2, 2, &raw));

        assert_eq!(pixel(&fb, 1, 1), [1, 2, 3, 0]);
        assert_eq!(pixel(&fb, 2, 1), [4, 5, 6, 0]);
        assert_eq!(pixel(&fb, 1, 2), [7, 8, 9, 0]);
        assert_eq!(pixel(&fb, 2, 2), [10, 11, 12, 0]);
        // Nothing outside it moved.
        assert_eq!(pixel(&fb, 0, 0), [0; 4]);
        assert_eq!(pixel(&fb, 3, 2), [0; 4]);
        assert_eq!(fb.take_damage(), Some(Region { x: 1, y: 1, width: 2, height: 2 }));
    }

    #[test]
    fn a_rectangle_over_the_edge_is_refused_rather_than_wrapped() {
        let mut fb = Framebuffer::new(4, 3);
        assert!(fb.rect_mut(3, 0, 2, 1).is_none());
        assert!(fb.rect_mut(0, 2, 1, 2).is_none());
        assert!(fb.rect_mut(4, 3, 0, 0).is_none(), "a zero-sized rectangle past the last pixel has no slice");
        assert!(fb.rect_mut(4, 0, 0, 0).is_some(), "one at the end of a row does");
        assert!(!fb.put_raw(3, 0, 2, 1, &[0; 8]));
        // A rectangle whose pixels are not the size it claims is refused too.
        assert!(!fb.put_raw(0, 0, 2, 2, &[0; 12]));
        assert_eq!(fb.take_damage(), None);
    }

    #[test]
    fn an_empty_raw_rectangle_draws_nothing_and_is_not_an_error() {
        let mut fb = Framebuffer::new(4, 3);
        assert!(fb.put_raw(1, 1, 0, 2, &[]));
        assert!(fb.put_raw(1, 1, 2, 0, &[]));
        assert!(!fb.put_raw(1, 1, 0, 2, &[0; 4]), "an empty rectangle with pixels is still the wrong size");
        assert_eq!(fb.take_damage(), None);
    }

    #[test]
    fn damage_accumulates_into_the_rectangle_that_covers_it() {
        let mut fb = Framebuffer::new(64, 64);
        fb.damage_rect(1, 2, 3, 4);
        fb.damage_rect(30, 40, 2, 2);
        assert_eq!(fb.take_damage(), Some(Region { x: 1, y: 2, width: 31, height: 40 }));
        assert_eq!(fb.take_damage(), None);
        // An empty rectangle is not damage.
        fb.damage_rect(5, 5, 0, 9);
        assert_eq!(fb.take_damage(), None);
    }

    #[test]
    fn a_resize_makes_a_new_black_framebuffer_the_window_can_tell_apart() {
        let mut fb = Framebuffer::new(2, 2);
        assert!(fb.put_raw(0, 0, 2, 2, &[9; 16]));
        let generation = fb.generation();

        fb.resize(2, 2).unwrap();
        assert_eq!(fb.generation(), generation, "the same size is not a resize");
        assert_eq!(pixel(&fb, 0, 0), [9; 4]);

        fb.resize(3, 1).unwrap();
        assert_ne!(fb.generation(), generation);
        assert_eq!((fb.width(), fb.height()), (3, 1));
        assert_eq!(fb.pixels().len(), 12);
        assert_eq!(fb.pixels(), &[0; 12]);
        assert_eq!(fb.take_damage(), Some(Region { x: 0, y: 0, width: 3, height: 1 }));
    }

    #[test]
    fn a_desktop_too_large_to_hold_is_refused_and_changes_nothing() {
        let mut fb = Framebuffer::new(3, 1);
        let generation = fb.generation();
        assert!(fb.resize(u16::MAX, u16::MAX).is_err());
        assert_eq!((fb.width(), fb.height()), (3, 1));
        assert_eq!(fb.pixels().len(), 12);
        assert_eq!(fb.generation(), generation);
    }
}
