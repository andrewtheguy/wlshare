//! A machine's hardware H.264 encoder, through VA-API: for the wlshare daemon
//! on that machine, and, experimentally, for a daemon on another, lent over
//! the network by an agent (`wlshare-enc-agent`).
//!
//! **The encoder** ([`vaapi`]) is the GPU's through libva itself, its
//! bindings written out by hand (`libva`) and the stream's headers this
//! module's own (`syntax`): every frame at the quantizer it is handed, one
//! frame in and one out. It takes a picture as [`Planes`], NV12, and is one
//! [`Backend`] of the daemon's streams, libx264 being the daemon's other.
//!
//! **The agent** ([`crate::agent`]) is for a daemon on a host with no GPU, whose
//! streams are otherwise libx264's, on its processor. The daemon's end is a
//! [`Backend`] too: it sends the agent what changed of the picture and is
//! sent the access unit back, over TCP behind a pre-shared key. The agent's
//! end keeps a copy of the picture and an encoder over it.

mod libva;
mod syntax;
pub mod vaapi;

/// The frame rate a stream's level is figured at: twice wlshare's default
/// `max_fps`, so a daemon set up to 60 still keeps to the level it names.
pub const FPS: u32 = 60;

/// What an encoder does with a picture: code it, at `qp`, as an IDR where
/// `keyframe` says so, and append the access unit to `out`. Whether it made a
/// frame.
pub trait Backend: Send {
    fn frame(
        &mut self,
        planes: &Planes,
        keyframe: bool,
        qp: u8,
        out: &mut Vec<u8>,
    ) -> anyhow::Result<bool>;
}

/// A stream's picture as its encoder takes it: NV12, BT.601 at studio swing,
/// and what is known of where it changed.
pub struct Planes {
    /// The size coded: the picture's rounded up to even sides.
    pub coded: (usize, usize),
    /// Of both planes: a multiple of 64, which leaves an odd picture's coded
    /// one its extra column.
    pub stride: usize,
    /// The coded picture's rows and one to spare, so the bottom row pair's
    /// planes are a whole stride past its left edge.
    pub y: Vec<u8>,
    pub uv: Vec<u8>,
    /// How many macroblocks the coded picture is across.
    pub across: usize,
    /// For each macroblock of the coded picture, row by row, how many frames
    /// have been coded since its samples were last written: none for one
    /// that changed since the last frame. Any other is in the planes sample
    /// for sample as that frame had it, which an encoder told so need not
    /// find out, and an agent need not be sent.
    pub still: Vec<u8>,
    /// The next frame is to look at every macroblock again, however long it
    /// has stood.
    pub refine: bool,
}

impl Planes {
    /// Planes for a `width`×`height` picture, black, every macroblock new.
    pub fn new(width: u16, height: u16) -> Self {
        let (w, h) = (usize::from(width), usize::from(height));
        let stride = w.next_multiple_of(64);
        let across = w.next_multiple_of(2).div_ceil(16);
        Self {
            coded: (w.next_multiple_of(2), h.next_multiple_of(2)),
            stride,
            y: vec![16; stride * (h.next_multiple_of(2) + 1)],
            uv: vec![128; stride * (h.div_ceil(2) + 1)],
            across,
            still: vec![0; across * h.next_multiple_of(2).div_ceil(16)],
            refine: false,
        }
    }

    /// A frame was made of the planes: one more has coded every macroblock
    /// as it stands, and a refinement asked for is done.
    pub fn coded_frame(&mut self) {
        for frames in &mut self.still {
            *frames = frames.saturating_add(1);
        }
        self.refine = false;
    }

    /// The samples from `(left, top)` to short of `(right, bottom)` of the
    /// coded picture were written: their macroblocks have changed.
    pub fn touch(&mut self, (left, top): (usize, usize), (right, bottom): (usize, usize)) {
        for row in top / 16..=(bottom - 1) / 16 {
            self.still[row * self.across + left / 16..=row * self.across + (right - 1) / 16]
                .fill(0);
        }
    }
}

/// The NAL unit types of an Annex B access unit, in order.
pub fn nal_types(unit: &[u8]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut at = 0;
    while at + 3 <= unit.len() {
        if unit[at] == 0 && unit[at + 1] == 0 && unit[at + 2] == 1 {
            if let Some(&header) = unit.get(at + 3) {
                types.push(header & 0x1f);
            }
            at += 3;
        } else {
            at += 1;
        }
    }
    types
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nal_types_are_read_off_annex_b_start_codes() {
        assert_eq!(
            nal_types(&[
                0, 0, 0, 1, 0x67, 0xAA, 0, 0, 1, 0x68, 0, 0, 0, 1, 0x65, 0x88
            ]),
            [7, 8, 5]
        );
        assert!(nal_types(&[1, 2, 3]).is_empty());
    }
}
