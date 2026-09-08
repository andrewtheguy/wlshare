//! The framebuffer: the captured output's pixels, and what changed when.
//!
//! One copy of the desktop, `B, G, R, X` per pixel with no row padding, written
//! by the compositor thread from each captured frame and read by every session
//! when it encodes. Damage is kept as a log of rectangles by generation, so a
//! session that last sent generation *n* asks for everything after *n* and gets
//! the union; a session further behind than the log reaches gets the whole
//! framebuffer, which is the right answer to being that far behind.

use std::collections::VecDeque;

use crate::shared::ClientId;

/// A rectangle in framebuffer pixels, exclusive at the far edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn whole(width: u16, height: u16) -> Self {
        Self { x: 0, y: 0, width, height }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    fn right(&self) -> u32 {
        u32::from(self.x) + u32::from(self.width)
    }

    fn bottom(&self) -> u32 {
        u32::from(self.y) + u32::from(self.height)
    }

    /// The part of `self` inside a `width`×`height` framebuffer.
    pub fn clipped(&self, width: u16, height: u16) -> Option<Self> {
        let right = self.right().min(u32::from(width));
        let bottom = self.bottom().min(u32::from(height));
        if u32::from(self.x) >= right || u32::from(self.y) >= bottom {
            return None;
        }
        Some(Self { x: self.x, y: self.y, width: (right - u32::from(self.x)) as u16, height: (bottom - u32::from(self.y)) as u16 })
    }

    fn intersects_or_touches(&self, other: &Self) -> bool {
        u32::from(self.x) <= other.right()
            && u32::from(other.x) <= self.right()
            && u32::from(self.y) <= other.bottom()
            && u32::from(other.y) <= self.bottom()
    }

    fn union(&self, other: &Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self { x, y, width: (right - u32::from(x)) as u16, height: (bottom - u32::from(y)) as u16 }
    }
}

/// Merge overlapping and touching rectangles until none do, and collapse to the
/// bounding box past `limit` rectangles: a client decodes a few large rectangles
/// faster than many slivers, and the gateway trims damage itself.
pub fn merge(mut rects: Vec<Rect>, limit: usize) -> Vec<Rect> {
    rects.retain(|r| !r.is_empty());
    loop {
        let mut merged = false;
        'outer: for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                if rects[i].intersects_or_touches(&rects[j]) {
                    let u = rects[i].union(&rects[j]);
                    rects[i] = u;
                    rects.swap_remove(j);
                    merged = true;
                    break 'outer;
                }
            }
        }
        if !merged {
            break;
        }
    }
    if rects.len() > limit {
        let bounds = rects.iter().skip(1).fold(rects[0], |acc, r| acc.union(r));
        return vec![bounds];
    }
    rects
}

/// Who asked for the size the framebuffer now has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeOrigin {
    /// The compositor, or a change nobody asked this server for.
    Server,
    /// A client's SetDesktopSize.
    Client(ClientId),
}

const LOG_LIMIT: usize = 512;

/// How a captured frame is laid out, next to the framebuffer's own top-row-first
/// XRGB8888.
///
/// Both differences come from the compositor rather than from anything this
/// server asks for: wlr-screencopy reports y-invert per frame, and the one shm
/// format it offers is whatever its renderer prefers to read back. wlroots'
/// GLES2 renderer takes that from Mesa's GL_IMPLEMENTATION_COLOR_READ_FORMAT,
/// which on Intel is RGBA and so lands on XBGR8888, while its pixman renderer
/// says XRGB8888. Neither is wrong, so both are handled here rather than in the
/// encoders: past this point a frame is always XRGB8888, rows top down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameLayout {
    /// The frame's first row is its bottom one.
    pub flipped: bool,
    /// The frame is XBGR8888 or ABGR8888 where the framebuffer is XRGB8888:
    /// red and blue trade places on the way in.
    pub swapped_rb: bool,
}

pub struct Framebuffer {
    pub width: u16,
    pub height: u16,
    /// `width * height * 4` bytes, rows contiguous.
    pub pixels: Vec<u8>,
    /// Bumped on every change to the pixels or the size.
    pub generation: u64,
    /// The generation at which the framebuffer took its current size.
    pub size_generation: u64,
    pub resize_origin: ResizeOrigin,
    /// Whether any pixels have been captured into the current size yet.
    pub painted: bool,
    log: VecDeque<(u64, Rect)>,
}

impl Framebuffer {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; usize::from(width) * usize::from(height) * 4],
            generation: 1,
            size_generation: 1,
            resize_origin: ResizeOrigin::Server,
            painted: false,
            log: VecDeque::new(),
        }
    }

    pub fn stride(&self) -> usize {
        usize::from(self.width) * 4
    }

    /// Take a new size, blank until the first frame at it arrives.
    pub fn resize(&mut self, width: u16, height: u16, origin: ResizeOrigin) {
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(usize::from(width) * usize::from(height) * 4, 0);
        self.generation += 1;
        self.size_generation = self.generation;
        self.resize_origin = origin;
        self.painted = false;
        self.log.clear();
    }

    /// Copy the damaged parts of a captured frame in, putting it into the
    /// framebuffer's own layout on the way. `src` rows are `stride` bytes apart
    /// and the frame is the framebuffer's size.
    pub fn apply(&mut self, src: &[u8], stride: usize, damage: &[Rect], layout: FrameLayout) {
        let row_bytes = self.stride();
        let last = usize::from(self.height).saturating_sub(1);
        let mut noted = Vec::with_capacity(damage.len());
        for rect in damage {
            let Some(r) = rect.clipped(self.width, self.height) else { continue };
            let x0 = usize::from(r.x) * 4;
            let len = usize::from(r.width) * 4;
            for row in usize::from(r.y)..usize::from(r.y) + usize::from(r.height) {
                let src_row = if layout.flipped { last - row } else { row };
                let s = src_row * stride + x0;
                let d = row * row_bytes + x0;
                let (dst, frame) = (&mut self.pixels[d..d + len], &src[s..s + len]);
                if layout.swapped_rb {
                    // Both are four bytes per pixel with the unused byte last,
                    // so only the two colour bytes at either end move.
                    for (out, px) in dst.chunks_exact_mut(4).zip(frame.chunks_exact(4)) {
                        out[0] = px[2];
                        out[1] = px[1];
                        out[2] = px[0];
                        out[3] = px[3];
                    }
                } else {
                    dst.copy_from_slice(frame);
                }
            }
            noted.push(r);
        }
        if noted.is_empty() {
            return;
        }
        self.generation += 1;
        self.painted = true;
        for r in noted {
            self.log.push_back((self.generation, r));
        }
        while self.log.len() > LOG_LIMIT {
            self.log.pop_front();
        }
    }

    /// What changed after `since`, merged, or `None` when the answer is
    /// everything: a size change, a log that no longer reaches back that far, or
    /// a client that has seen nothing yet.
    pub fn damage_since(&self, since: u64) -> Option<Vec<Rect>> {
        if since < self.size_generation || since == 0 {
            return None;
        }
        if since >= self.generation {
            return Some(Vec::new());
        }
        // The log must contain generation `since + 1` (or the framebuffer has
        // not changed since); if its first entry is newer, entries were dropped.
        if self.log.front().is_some_and(|(g, _)| *g > since + 1) {
            return None;
        }
        let rects: Vec<Rect> = self.log.iter().filter(|(g, _)| *g > since).map(|(_, r)| *r).collect();
        Some(merge(rects, 32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touching_and_overlapping_rectangles_merge() {
        let rects = vec![
            Rect { x: 0, y: 0, width: 10, height: 10 },
            Rect { x: 10, y: 0, width: 10, height: 10 },
            Rect { x: 50, y: 50, width: 5, height: 5 },
            Rect { x: 0, y: 0, width: 0, height: 4 },
        ];
        let merged = merge(rects, 10);
        assert_eq!(merged, vec![Rect { x: 0, y: 0, width: 20, height: 10 }, Rect { x: 50, y: 50, width: 5, height: 5 }]);
        assert_eq!(merge(merged.clone(), 1), vec![Rect { x: 0, y: 0, width: 55, height: 55 }]);
    }

    #[test]
    fn damage_is_by_generation_and_a_resize_means_everything() {
        let mut fb = Framebuffer::new(4, 4);
        assert_eq!(fb.damage_since(0), None);
        let frame = vec![7u8; 4 * 4 * 4];
        fb.apply(&frame, 16, &[Rect { x: 1, y: 1, width: 2, height: 1 }], FrameLayout::default());
        let g1 = fb.generation;
        assert_eq!(fb.damage_since(g1), Some(vec![]));
        assert_eq!(fb.damage_since(g1 - 1), Some(vec![Rect { x: 1, y: 1, width: 2, height: 1 }]));
        assert_eq!(&fb.pixels[20..28], &[7; 8]);
        assert_eq!(&fb.pixels[16..20], &[0; 4]);
        fb.apply(&frame, 16, &[Rect { x: 3, y: 1, width: 5, height: 1 }], FrameLayout::default()); // clipped to x 3..4
        assert_eq!(fb.damage_since(g1 - 1), Some(vec![Rect { x: 1, y: 1, width: 3, height: 1 }]));
        fb.resize(2, 2, ResizeOrigin::Client(ClientId(3)));
        assert_eq!(fb.damage_since(g1), None);
        assert!(!fb.painted);
        assert_eq!(fb.pixels.len(), 16);
    }

    #[test]
    fn a_client_behind_the_log_is_told_to_repaint() {
        let mut fb = Framebuffer::new(2, 2);
        let frame = vec![1u8; 16];
        let start = fb.generation;
        for i in 0..LOG_LIMIT + 5 {
            fb.apply(&frame, 8, &[Rect { x: (i % 2) as u16, y: 0, width: 1, height: 1 }], FrameLayout::default());
        }
        assert_eq!(fb.damage_since(start), None);
        assert_eq!(fb.damage_since(fb.generation - 3).map(|r| r.len()), Some(1));
    }

    #[test]
    fn a_swapped_frame_arrives_with_red_and_blue_the_right_way_round() {
        // One pixel, XBGR8888 in memory: R=1, G=2, B=3, unused=4. The
        // framebuffer wants it as B, G, R, X.
        let mut fb = Framebuffer::new(1, 1);
        fb.apply(&[1, 2, 3, 4], 4, &[Rect::whole(1, 1)], FrameLayout { flipped: false, swapped_rb: true });
        assert_eq!(fb.pixels, vec![3, 2, 1, 4]);

        // The same frame read straight through is the identity, so the two
        // paths differ only in the two colour bytes.
        let mut fb = Framebuffer::new(1, 1);
        fb.apply(&[1, 2, 3, 4], 4, &[Rect::whole(1, 1)], FrameLayout::default());
        assert_eq!(fb.pixels, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_swapped_frame_flips_and_swaps_together_and_only_where_damaged() {
        // Two rows of two pixels, bottom row first, red and blue reversed. Only
        // the top-left pixel is damaged, and after the flip that is the frame's
        // *last* row -- so the pixel that lands is [30, 20, 10, 40] swapped.
        let mut fb = Framebuffer::new(2, 2);
        let frame: Vec<u8> = vec![
            1, 2, 3, 4, 5, 6, 7, 8, // frame row 0 = framebuffer row 1
            10, 20, 30, 40, 50, 60, 70, 80, // frame row 1 = framebuffer row 0
        ];
        fb.apply(&frame, 8, &[Rect { x: 0, y: 0, width: 1, height: 1 }], FrameLayout { flipped: true, swapped_rb: true });
        assert_eq!(&fb.pixels[0..4], &[30, 20, 10, 40]);
        assert_eq!(&fb.pixels[4..16], &[0; 12], "nothing outside the damage is touched");
    }

    #[test]
    fn a_frame_with_no_damage_inside_the_framebuffer_changes_nothing() {
        let mut fb = Framebuffer::new(2, 2);
        let g = fb.generation;
        fb.apply(&[9; 16], 8, &[Rect { x: 5, y: 5, width: 1, height: 1 }], FrameLayout::default());
        assert_eq!(fb.generation, g);
        assert!(!fb.painted);
    }
}
