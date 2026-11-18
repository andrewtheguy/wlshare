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
//! end keeps a copy of the picture and an encoder over it, or, in memory the
//! two share, reads the picture where the daemon wrote it.

mod libva;
mod syntax;
pub mod vaapi;

/// The frame rate a stream's level is figured at: twice wlshare's default
/// `max_fps`, so a daemon set up to 60 still keeps to the level it names.
pub const FPS: u32 = 60;

/// What an encoder does with a picture: code it, at `qp`, as an IDR where
/// `keyframe` says so, and append the access unit to `out`. Whether it made a
/// frame. The planes are its to read and not to change, but for one thing:
/// an encoder whose frame failed with their buffers still in the kernel's
/// hands (the agent's stream, sending zero-copy) keeps those buffers and
/// leaves the planes copies of them, sample for sample.
pub trait Backend: Send {
    fn frame(
        &mut self,
        planes: &mut Planes,
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
    pub y: Plane,
    pub uv: Plane,
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
    /// The size a `width`×`height` picture is coded at, its sides rounded
    /// up to even: what an encoder is opened over, which can be before the
    /// planes are made room for.
    pub fn coded_size(width: u16, height: u16) -> (usize, usize) {
        (
            usize::from(width).next_multiple_of(2),
            usize::from(height).next_multiple_of(2),
        )
    }

    /// How a `width`×`height` picture's planes are laid out: the stride, and
    /// the bytes of the luma plane and of the chroma plane.
    pub fn layout(width: u16, height: u16) -> (usize, usize, usize) {
        let (w, h) = (usize::from(width), usize::from(height));
        let stride = w.next_multiple_of(64);
        (
            stride,
            stride * (h.next_multiple_of(2) + 1),
            stride * (h.div_ceil(2) + 1),
        )
    }

    /// Planes for a `width`×`height` picture, black, every macroblock new.
    pub fn new(width: u16, height: u16) -> Self {
        let (_, y, uv) = Self::layout(width, height);
        Self::over(width, height, vec![16; y].into(), vec![128; uv].into())
    }

    /// Planes for a `width`×`height` picture over samples that are already
    /// somewhere, [`Self::layout`]'s bytes of each plane, every macroblock
    /// new.
    pub fn over(width: u16, height: u16, y: Plane, uv: Plane) -> Self {
        let (stride, y_bytes, uv_bytes) = Self::layout(width, height);
        assert!(
            y.len() == y_bytes && uv.len() == uv_bytes,
            "planes of {} and {} bytes for a {width}x{height} picture, which has {y_bytes} and {uv_bytes}",
            y.len(),
            uv.len()
        );
        let across = usize::from(width).next_multiple_of(2).div_ceil(16);
        Self {
            coded: Self::coded_size(width, height),
            stride,
            y,
            uv,
            across,
            still: vec![0; across * usize::from(height).next_multiple_of(2).div_ceil(16)],
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

/// A plane's samples: its own, or where they lie in memory another machine
/// writes them into, a picture of the encoding agent's shared memory
/// ([`crate::agent::shared`]), which the agent's encoder reads from there.
pub struct Plane(Samples);

enum Samples {
    Own(Vec<u8>),
    Shared(crate::agent::shared::View),
}

impl Plane {
    /// The samples as a buffer of their own: these, or a copy of shared ones.
    pub fn into_vec(self) -> Vec<u8> {
        match self.0 {
            Samples::Own(samples) => samples,
            Samples::Shared(view) => view.to_vec(),
        }
    }
}

impl From<Vec<u8>> for Plane {
    fn from(samples: Vec<u8>) -> Self {
        Self(Samples::Own(samples))
    }
}

impl From<crate::agent::shared::View> for Plane {
    fn from(view: crate::agent::shared::View) -> Self {
        Self(Samples::Shared(view))
    }
}

impl std::ops::Deref for Plane {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match &self.0 {
            Samples::Own(samples) => samples,
            Samples::Shared(view) => view,
        }
    }
}

impl std::ops::DerefMut for Plane {
    fn deref_mut(&mut self) -> &mut [u8] {
        match &mut self.0 {
            Samples::Own(samples) => samples,
            Samples::Shared(view) => view,
        }
    }
}

/// A copy is a plane's own, whatever the samples it was made from.
impl Clone for Plane {
    fn clone(&self) -> Self {
        self.to_vec().into()
    }
}

impl PartialEq for Plane {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl std::fmt::Debug for Plane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let whose = match self.0 {
            Samples::Own(_) => "own",
            Samples::Shared(_) => "shared",
        };
        write!(f, "Plane({} bytes, {whose})", self.len())
    }
}

impl<'a> IntoIterator for &'a Plane {
    type Item = &'a u8;
    type IntoIter = std::slice::Iter<'a, u8>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut Plane {
    type Item = &'a mut u8;
    type IntoIter = std::slice::IterMut<'a, u8>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// The most bytes an access unit of a `width`×`height` picture is taken to
/// be: the size of the buffer the hardware codes a frame into
/// ([`vaapi::Stream`]), and so the most a daemon takes of one from an agent.
/// Raw 4:2:0 is 384 bytes a macroblock; the rest is headroom for the headers
/// and a coarse estimate.
pub fn most_coded((width, height): (usize, usize)) -> usize {
    width.div_ceil(16) * height.div_ceil(16) * 400 + 64 * 1024
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
