//! H.264 for the video stream and for the tiles picture's moving regions: the
//! host's hardware encoder through libva itself ([`vaapi`]), where the host
//! has one, and libx264 at its fastest preset through its own API ([`x264`])
//! where it has none.
//!
//! The video stream is one stream over the whole desktop
//! ([`crate::rfb::video`]), and a region of the tiles picture one over its
//! rectangle alone ([`crate::rfb::tiles`]), each coded by the [`H264Coder`]
//! the daemon hands its encoder. This is that coder. On a host whose GPU has
//! an H.264 encoder a VA-API driver reaches — Intel's iHD, AMD's Mesa — a
//! picture that moves costs the processor its conversion to NV12 and nothing
//! of the search. On a host with none, libx264's `ultrafast` codes it in
//! about a third of the processor's time a frame that VP9 at libvpx's fastest
//! speed took, for about twice the bytes.
//!
//! **What a stream is.** Every frame at the quantizer its encode is handed,
//! from the dial it is asked at, with no bitrate and nothing dropped: the
//! 1–100 dial maps onto QP 40 down to 16, finest last ([`qp`]). The quantizer
//! moves between any two frames with no keyframe, which is what lets the walk
//! ([`crate::walk`]) give a link's quality up and take it back on a running
//! stream. Neither library's usual wrapper does this: libavcodec fixes the
//! quantizer when it opens an encoder, its `h264_vaapi` and its libx264 alike,
//! so each backend here is the library's own API. No B-frames, one frame in
//! and one out, so a frame is on the wire the moment it is coded: the
//! hardware's at High profile with CABAC, one slice, the frame before as its
//! one reference; libx264's at Constrained Baseline, the profile `ultrafast`
//! comes to, with its `zerolatency` tuning, no scene cut, and
//! [`encoder_threads`] threads, as slices. BT.601 at studio swing, said in
//! the stream's VUI. The picture coded is the stream's rounded up to even
//! sides, which libx264 requires and 4:2:0 cannot crop below: a picture of an
//! odd width or height is coded a pixel larger, its last row again below it
//! and black to its right, and the gateway clips it. Each frame is one Annex B
//! access unit, the parameter sets ahead of every IDR, and the first frame of
//! a stream is one. No AUD; no SEI from the hardware, and from libx264 the one
//! naming its version and options, ahead of a stream's first IDR, which a
//! decoder passes over.
//!
//! **Which encoder.** The daemon looks once, at startup ([`Encoder::find`]):
//! with `hardware_h264` on, each render node in turn, the low-power encoder
//! first — Intel's fixed-function block, which leaves the execution units to
//! the desktop — and the shader one after it, each tried on two frames of a
//! small picture at two quantizers, since a driver lists more than it will
//! code; with none working, or the key off, libx264, tried the same way. What
//! works is what every session uses; with nothing working the daemon does not
//! start, since neither the video stream nor a region has another codec. The
//! low-power block codes a constant quantizer without the HuC firmware, which
//! only bitrate control needs.
//!
//! **Failure ends the session.** A coder that cannot be opened, refuses a
//! stream or fails one midway fails the encode of the picture it codes
//! ([`crate::rfb::video::VideoEncoder`], [`crate::rfb::tiles::TilesEncoder`]),
//! and with it the session that asked for the picture. Each session opens
//! the device itself ([`Encoder::coder`]), so a VA display is one session's and
//! the sessions share nothing of it.
//!
//! Tested against libavcodec's own H.264 decoder ([`crate::decode`]), which
//! shares nothing with either encoder: libx264's on every machine, the
//! hardware's where there is one, its test saying so and passing where there
//! is not, since the machines that build this seldom have one.

mod libva;
mod syntax;
mod vaapi;
mod x264;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use log::info;
use crate::rfb::{H264Coder, H264Error, H264Stream, Rect};

/// The H.264 encoder the startup found, which every session's coder opens.
#[derive(Debug)]
pub enum Encoder {
    /// The host's GPU, through libva.
    Vaapi {
        /// The render node, `/dev/dri/renderD128` or a sibling.
        device: PathBuf,
        /// Whether the low-power encoder is the one that works.
        low_power: bool,
    },
    /// libx264, on the processor.
    X264,
}

/// The picture the probe codes two frames of, small enough for any encoder.
const PROBE_SIZE: (u16, u16) = (64, 64);

impl Encoder {
    /// The encoder the video stream and the tiles' regions are coded by: the
    /// hardware's where `hardware` asks for it and [`vaapi::find`] finds one,
    /// libx264 otherwise, each tried on two frames first. An error with
    /// neither working.
    pub fn find(hardware: bool) -> anyhow::Result<Self> {
        if hardware {
            if let Some(encoder) = vaapi::find() {
                return Ok(encoder);
            }
        } else {
            info!("hardware_h264 is off: the hardware H.264 encoder is not looked for");
        }
        Self::X264.try_frames().context("no H.264 encoder: libx264 codes nothing here")?;
        info!("software H.264: libx264 at {}", x264::PRESET);
        Ok(Self::X264)
    }

    /// What codes the streams, for the log: `H.264 by …`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Vaapi { .. } => "the hardware",
            Self::X264 => "libx264",
        }
    }

    /// Open a coder and code two frames of a small picture of noise with it:
    /// an IDR at the dial's middle and a P frame at its fine end, each decoded
    /// by libavcodec and its median macroblock held to the QP asked, so an
    /// encoder that cannot move its quantizer between frames is not one.
    fn try_frames(&self) -> anyhow::Result<()> {
        let mut coder = self.coder()?;
        let (w, h) = PROBE_SIZE;
        let mut stream = coder.open(w, h).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut decoder = crate::decode::Decoder::with_qps()?;
        for (n, (quality, nal, kind)) in [(50, 5, "an IDR picture"), (100, 1, "a P frame")].into_iter().enumerate() {
            // A new picture each frame, so the P frame's macroblocks are coded
            // and not skipped at the first's quantizer.
            stream.read_bgrx(&noise(w.into(), h.into(), 0x9E37_79B9 + n as u32), usize::from(w) * 4, None).map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut out = Vec::new();
            anyhow::ensure!(stream.encode(n == 0, quality, &mut out).map_err(|e| anyhow::anyhow!("{e}"))?, "the encoder made no frame {n}");
            anyhow::ensure!(nal_types(&out).contains(&nal), "frame {n} is not {kind}");
            let mut blocks = decoder.qps(&out).context("decoding a trial frame")?;
            blocks.sort_unstable();
            let asked = i32::from(qp(quality));
            let median = blocks.get(blocks.len() / 2).copied();
            anyhow::ensure!(median == Some(asked), "frame {n} was asked QP {asked} and coded at {median:?}");
        }
        Ok(())
    }

    /// A coder for one session: for the hardware, its own VA display on the
    /// device.
    pub fn coder(&self) -> anyhow::Result<Coder> {
        Ok(match self {
            Self::Vaapi { device, low_power } => Coder::Vaapi(Arc::new(vaapi::Display::open(device)?), *low_power),
            Self::X264 => Coder::X264,
        })
    }
}

/// One session's H.264 coder: for the hardware, a VA display, which each of
/// its streams takes an encoder on; for libx264, nothing until a stream.
pub enum Coder {
    Vaapi(Arc<vaapi::Display>, bool),
    X264,
}

impl H264Coder for Coder {
    fn open(&mut self, width: u16, height: u16) -> Result<Box<dyn H264Stream>, H264Error> {
        let opened: anyhow::Result<Box<dyn H264Stream>> = (|| {
            anyhow::ensure!(width > 0 && height > 0, "a {width}x{height} picture has no pixels");
            let planes = Planes::new(width, height);
            let stream: Box<dyn H264Stream> = match self {
                Self::Vaapi(display, low_power) => Box::new(Stream { backend: vaapi::Stream::open(display.clone(), *low_power, planes.coded)?, planes }),
                Self::X264 => Box::new(Stream { backend: x264::Stream::open(planes.coded)?, planes }),
            };
            Ok(stream)
        })();
        opened.map_err(|e| format!("{e:#}").into())
    }
}

/// What a backend does with a picture the stream has read: code it, at
/// `qp`, as an IDR where `keyframe` says so, and append the access unit to
/// `out`. Whether it made a frame.
trait Backend: Send {
    fn frame(&mut self, planes: &Planes, keyframe: bool, qp: u8, out: &mut Vec<u8>) -> anyhow::Result<bool>;
}

/// One stream, the desktop's or a region's: the NV12 picture in front of an
/// encoder, and the encoder.
struct Stream<B> {
    planes: Planes,
    backend: B,
}

impl<B: Backend> H264Stream for Stream<B> {
    fn read_bgrx(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> Result<(), H264Error> {
        self.planes.read_bgrx(pixels, stride, changed).map_err(|e| format!("{e:#}").into())
    }

    fn encode(&mut self, keyframe: bool, quality: u8, out: &mut Vec<u8>) -> Result<bool, H264Error> {
        let coded: anyhow::Result<bool> = (|| {
            anyhow::ensure!(self.planes.filled, "a stream was asked for a frame before any picture was read into it");
            self.backend.frame(&self.planes, keyframe, qp(quality), out)
        })();
        coded.map_err(|e| format!("{e:#}").into())
    }
}

/// The QP a dial asks for: 40 at 1, 16 at 100, finer as the dial rises;
/// H.264's step doubles every six.
fn qp(quality: u8) -> u8 {
    let quality = i32::from(quality.clamp(1, 100));
    (40 - ((quality - 1) * 24 + 49) / 99) as u8
}

/// How many threads libx264 gets: the machine less two cores, at most
/// [`MAX_ENCODER_THREADS`]. An encode is a burst of milliseconds a frame that
/// the person at the other end is waiting on, and the encoder splits it
/// across its threads as slices; the two cores kept back are for the
/// compositor and the session, which are what make the next frame.
pub fn encoder_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(2)).clamp(1, MAX_ENCODER_THREADS)
}

/// The most threads libx264 takes, whatever the machine: about one a
/// megapixel, eight for 4K's 8.3, the largest desktop this is tuned for.
const MAX_ENCODER_THREADS: usize = 8;

/// The frame rate the level is figured at: twice wlshare's default
/// `max_fps`, so a daemon set up to 60 still keeps to the level it names.
const FPS: u32 = 60;

/// A stream's picture as its encoder takes it: NV12, BT.601 at studio swing,
/// converted from the framebuffer where it changed.
struct Planes {
    size: (u16, u16),
    /// The size coded: `size` rounded up to even sides.
    coded: (usize, usize),
    /// Of both planes: a multiple of 64, which leaves an odd picture's coded
    /// one its extra column.
    stride: usize,
    /// The bottom row pair of a conversion, copied: `B, G, R, X`.
    pair: Vec<u8>,
    /// The coded picture's rows and one to spare, so the bottom row pair's
    /// planes are a whole stride past its left edge.
    y: Vec<u8>,
    uv: Vec<u8>,
    /// The planes hold a picture: one was read whole.
    filled: bool,
}

impl Planes {
    fn new(width: u16, height: u16) -> Self {
        let (w, h) = (usize::from(width), usize::from(height));
        let stride = w.next_multiple_of(64);
        Self {
            size: (width, height),
            coded: (w.next_multiple_of(2), h.next_multiple_of(2)),
            stride,
            pair: Vec::new(),
            y: vec![16; stride * (h.next_multiple_of(2) + 1)],
            uv: vec![128; stride * (h.div_ceil(2) + 1)],
            filled: false,
        }
    }

    /// Read `changed` of `pixels` into the planes, or the whole picture where
    /// it is `None` or none has been read yet.
    fn read_bgrx(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> anyhow::Result<()> {
        let (w, h) = self.size;
        match changed.filter(|_| self.filled) {
            Some(rects) => {
                for rect in rects {
                    self.convert(pixels, stride, *rect)?;
                }
            }
            None => {
                self.convert(pixels, stride, Rect { x: 0, y: 0, width: w, height: h })?;
                self.filled = true;
            }
        }
        Ok(())
    }

    /// Convert `rect` of `pixels` into the NV12 planes: `B, G, R, X` rows
    /// `stride` apart, the rectangle widened to whole 2×2 blocks within the
    /// picture, and on to the coded picture's even height at the bottom.
    ///
    /// The converter takes its rows a pair at a time, each pair a whole
    /// stride of every plane long, and an odd height's last row as the end of
    /// each plane's slice; so every pair but the last is converted where it
    /// lies, with slices of exactly its pairs, and the last from a copy of its
    /// two rows — the picture's last row twice where its height is odd, so
    /// the row's chroma is its own.
    fn convert(&mut self, pixels: &[u8], stride: usize, rect: Rect) -> anyhow::Result<()> {
        let (w, h) = (usize::from(self.size.0), usize::from(self.size.1));
        let (left, top) = (usize::from(rect.x) & !1, usize::from(rect.y) & !1);
        let right = (usize::from(rect.x) + usize::from(rect.width)).next_multiple_of(2).min(w);
        let bottom = (usize::from(rect.y) + usize::from(rect.height)).next_multiple_of(2).min(h.next_multiple_of(2));
        if left >= right || top >= bottom {
            return Ok(());
        }
        let rw = right - left;
        anyhow::ensure!(stride >= w * 4 && pixels.len() >= (h - 1) * stride + w * 4, "a {w}x{h} picture at a stride of {stride} does not fit in {} bytes", pixels.len());
        let ps = self.stride;
        let last = bottom - 2;
        if last > top {
            let rgba = &pixels[top * stride + left * 4..last * stride + left * 4];
            nv12(&mut self.y[top * ps + left..last * ps + left], &mut self.uv[top / 2 * ps + left..last / 2 * ps + left], ps, rgba, stride, rw, last - top)?;
        }
        let mut pair = std::mem::take(&mut self.pair);
        pair.clear();
        for row in [last, (last + 1).min(h - 1)] {
            pair.extend_from_slice(&pixels[row * stride + left * 4..row * stride + right * 4]);
        }
        let converted = nv12(&mut self.y[last * ps + left..bottom * ps + left], &mut self.uv[last / 2 * ps + left..bottom / 2 * ps + left], ps, &pair, rw * 4, rw, 2);
        self.pair = pair;
        converted
    }
}

/// Convert `height` rows of `width` pixels, `B, G, R, X` rows `rgba_stride`
/// apart, into NV12 planes `stride` apart: BT.601 at studio swing. `height` is
/// even, and each slice is exactly its rows.
fn nv12(y: &mut [u8], uv: &mut [u8], stride: usize, rgba: &[u8], rgba_stride: usize, width: usize, height: usize) -> anyhow::Result<()> {
    use yuv::{BufferStoreMut, YuvBiPlanarImageMut, YuvConversionMode, YuvRange, YuvStandardMatrix};
    let mut image = YuvBiPlanarImageMut {
        y_plane: BufferStoreMut::Borrowed(y),
        y_stride: stride as u32,
        uv_plane: BufferStoreMut::Borrowed(uv),
        uv_stride: stride as u32,
        width: width as u32,
        height: height as u32,
    };
    yuv::bgra_to_yuv_nv12(&mut image, rgba, rgba_stride as u32, YuvRange::Limited, YuvStandardMatrix::Bt601, YuvConversionMode::Professional)
        .map_err(|e| anyhow::anyhow!("converting a {width}x{height} rectangle to NV12: {e}"))
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

/// A picture of noise as `B, G, R, X`, which no prediction codes for free.
fn noise(w: usize, h: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..w * h)
        .flat_map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let [b, g, r, _] = state.to_le_bytes();
            [b, g, r, 0]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dial_maps_onto_qp_40_down_to_16() {
        assert_eq!((qp(1), qp(50), qp(100)), (40, 28, 16));
        assert!((1..100).all(|q| qp(q) >= qp(q + 1)), "finer dial, lower QP");
    }

    #[test]
    fn nal_types_are_read_off_annex_b_start_codes() {
        assert_eq!(nal_types(&[0, 0, 0, 1, 0x67, 0xAA, 0, 0, 1, 0x68, 0, 0, 0, 1, 0x65, 0x88]), [7, 8, 5]);
        assert!(nal_types(&[1, 2, 3]).is_empty());
    }

    /// The luma of the test picture: a ramp across and down, which a
    /// quantizer at the fine end of the dial keeps within a few levels.
    fn luma(x: usize, y: usize) -> u8 {
        (40 + x / 2 + y) as u8
    }

    /// A grey picture of that luma as `B, G, R, X`: equal channels, so BT.601
    /// gives the luma back as itself at studio swing's scale.
    fn picture(w: usize, h: usize) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let grey = luma(x, y);
                pixels.extend_from_slice(&[grey, grey, grey, 0]);
            }
        }
        pixels
    }

    /// Decode `unit` with libavcodec's software decoder and compare its luma
    /// with `pixels`'s grey, within `tolerance` levels after studio swing: a
    /// picture of the region's size rounded up to even sides.
    fn decodes_to(decoder: &mut crate::decode::Decoder, unit: &[u8], pixels: &[u8], w: usize, h: usize, tolerance: u8) {
        let mut out = Vec::new();
        let decoded = decoder.decode(unit, &mut out).unwrap().expect("a picture for every unit");
        assert_eq!((decoded.width, decoded.height), (w.next_multiple_of(2), h.next_multiple_of(2)));
        for y in 0..h {
            for x in 0..w {
                let grey = pixels[(y * w + x) * 4];
                let expected = 16.0 + f64::from(grey) * 219.0 / 255.0;
                let got = f64::from(out[y * decoded.width + x]);
                assert!((got - expected).abs() <= f64::from(tolerance), "luma at {x},{y}: {got}, not {expected:.1}");
            }
        }
    }

    /// Frames `encoder` codes decode with libavcodec's software decoder to
    /// the picture they were coded from: the first an IDR with its parameter
    /// sets, the next a P frame of the damage alone, and a keyframe asked for
    /// one. Within a few levels of the picture, and `slack` more: the
    /// hardware's pixels are not held to libx264's, only to a rectangle's
    /// being coded where it changed.
    fn frames_decode_to_the_picture(encoder: &Encoder, (w, h): (usize, usize), slack: u8) {
        let mut coder = encoder.coder().unwrap();
        let mut stream = coder.open(w as u16, h as u16).unwrap();
        let mut pixels = picture(w, h);
        let mut decoder = crate::decode::Decoder::new().unwrap();

        stream.read_bgrx(&pixels, w * 4, Some(&[Rect { x: 1, y: 1, width: 1, height: 1 }])).unwrap();
        let mut unit = Vec::new();
        assert!(stream.encode(false, 100, &mut unit).unwrap(), "a frame of the first picture");
        let types = nal_types(&unit);
        assert!(types.contains(&7) && types.contains(&8) && types.contains(&5), "the first frame carries SPS, PPS and an IDR: {types:?}");
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6 + slack);

        // A rectangle changes, and the frame after it is a P frame.
        for y in 20..40 {
            for x in 31..60 {
                pixels[(y * w + x) * 4..][..3].copy_from_slice(&[200, 200, 200]);
            }
        }
        stream.read_bgrx(&pixels, w * 4, Some(&[Rect { x: 31, y: 20, width: 29, height: 20 }])).unwrap();
        unit.clear();
        assert!(stream.encode(false, 100, &mut unit).unwrap());
        let types = nal_types(&unit);
        assert!(types.contains(&1) && !types.contains(&5), "a P frame: {types:?}");
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6 + slack);

        // A rectangle along the bottom edge, away from the left one, changes:
        // its last rows are the picture's, with nothing after them.
        for y in h - 3..h {
            for x in 41..w {
                pixels[(y * w + x) * 4..][..3].copy_from_slice(&[180, 180, 180]);
            }
        }
        stream.read_bgrx(&pixels, w * 4, Some(&[Rect { x: 41, y: (h - 3) as u16, width: (w - 41) as u16, height: 3 }])).unwrap();
        unit.clear();
        assert!(stream.encode(false, 100, &mut unit).unwrap());
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6 + slack);

        // A keyframe asked for is one, with its parameter sets.
        unit.clear();
        assert!(stream.encode(true, 100, &mut unit).unwrap());
        let types = nal_types(&unit);
        assert!(types.contains(&7) && types.contains(&8) && types.contains(&5), "an IDR with its parameter sets where one was asked: {types:?}");
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6 + slack);
    }

    /// The QP each frame is coded at, as libavcodec's decoder reads it back:
    /// one stream of noise walked across the dial, a new picture of noise every
    /// frame so none is free, with no keyframe but the first. Every frame is
    /// at the QP its dial asks, and the coarse end codes it in well under half
    /// the bytes of the fine end, so the dial does set the quantizer frame by
    /// frame. Every macroblock is at that QP, or, short of `exact`, its median
    /// is and each is within [`MACROBLOCK_SLACK`] of it: a hardware encoder's
    /// macroblocks are its driver's, and are not held to libx264's.
    fn the_quantizer_moves_frame_by_frame(encoder: &Encoder, exact: bool) {
        let (w, h) = (96usize, 64usize);
        let mut coder = encoder.coder().unwrap();
        let mut stream = coder.open(w as u16, h as u16).unwrap();
        let mut decoder = crate::decode::Decoder::with_qps().unwrap();
        let walk = [90u8, 90, 50, 1, 100, 20, 70, 1, 100];
        let mut sizes = Vec::new();
        for (n, &quality) in walk.iter().enumerate() {
            stream.read_bgrx(&noise(w, h, 0x9E37_79B9 + n as u32), w * 4, None).unwrap();
            let mut unit = Vec::new();
            assert!(stream.encode(false, quality, &mut unit).unwrap());
            let idr = nal_types(&unit).contains(&5);
            assert_eq!(idr, n == 0, "frame {n}: an IDR only first");
            let asked = i32::from(qp(quality));
            let mut blocks = decoder.qps(&unit).unwrap();
            blocks.sort_unstable();
            assert_eq!(blocks.get(blocks.len() / 2), Some(&asked), "frame {n} at quality {quality}: the median macroblock's QP");
            let slack = if exact { 0 } else { MACROBLOCK_SLACK };
            assert!(!blocks.is_empty() && blocks.iter().all(|&q| q.abs_diff(asked) <= slack), "frame {n} at quality {quality}: macroblocks from {:?} to {:?}, not {asked} within {slack}", blocks.iter().min(), blocks.iter().max());
            sizes.push((quality, unit.len()));
        }
        let fine = sizes.iter().filter(|(q, _)| *q == 100).map(|(_, s)| *s).min().unwrap();
        let coarse = sizes.iter().filter(|(q, _)| *q == 1).map(|(_, s)| *s).max().unwrap();
        assert!(coarse * 2 < fine, "the dial at 1 codes noise in under half the bytes of 100: {sizes:?}");
    }

    /// How far a hardware encoder's macroblock may stand from its slice's QP.
    const MACROBLOCK_SLACK: u32 = 3;

    /// The hardware's frames decode to the picture, near enough, and its
    /// quantizer follows the dial frame by frame. Passes with a word where
    /// this machine has no encoder.
    #[test]
    fn hardware_frames_decode_to_the_picture_with_libavcodecs_software_decoder() {
        let Some(encoder) = vaapi::find() else {
            eprintln!("no VA-API H.264 encoder on this machine; the hardware coder is not exercised");
            return;
        };
        frames_decode_to_the_picture(&encoder, (96, 70), 18);
        frames_decode_to_the_picture(&encoder, (95, 69), 18);
        the_quantizer_moves_frame_by_frame(&encoder, false);
    }

    /// libx264's frames decode to the picture, on every machine: it is what a
    /// host without a GPU codes its streams with.
    #[test]
    fn libx264_frames_decode_to_the_picture_with_libavcodecs_software_decoder() {
        Encoder::X264.try_frames().expect("libx264 through its own API");
        frames_decode_to_the_picture(&Encoder::X264, (96, 70), 0);
    }

    /// libx264's quantizer follows the dial frame by frame.
    #[test]
    fn libx264_moves_its_quantizer_frame_by_frame() {
        the_quantizer_moves_frame_by_frame(&Encoder::X264, true);
    }

    /// The video encoding's frames, through their framing, decode with
    /// libavcodec's software decoder to the desktop they were read from: the
    /// first an IDR with its parameter sets, a frame of
    /// damage read from a buffer whose other rows are not the picture a P
    /// frame of the whole desktop, and an IDR where one is asked. A desktop of
    /// odd sides, coded a pixel larger.
    #[test]
    fn the_video_encodings_frames_decode_to_the_desktop() {
        use crate::rfb::video::VideoEncoder;
        let (w, h) = (161usize, 99usize);
        let mut coder = Encoder::X264.coder().unwrap();
        let mut encoder = VideoEncoder::new(&mut coder, w as u16, h as u16).unwrap();
        let mut decoder = crate::decode::Decoder::new().unwrap();
        let frame = |encoder: &mut VideoEncoder, pixels: &[u8], changed: Option<&[Rect]>, keyframe: bool| {
            encoder.read(pixels, w * 4, changed).unwrap();
            let mut out = Vec::new();
            assert!(encoder.encode(keyframe, crate::rfb::QUALITY, &mut out).unwrap(), "a frame");
            assert_eq!(u32::from_be_bytes(out[..4].try_into().unwrap()) as usize, out.len() - 4, "the length word is the access unit's");
            out.split_off(4)
        };

        let mut pixels = picture(w, h);
        let unit = frame(&mut encoder, &pixels, None, false);
        let types = nal_types(&unit);
        assert!(types.contains(&7) && types.contains(&8) && types.contains(&5), "the first frame is an IDR with its parameter sets: {types:?}");
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6);

        // Damage, in a buffer whose rows outside it are not the picture.
        let damage = Rect { x: 40, y: 20, width: 60, height: 30 };
        let mut rows = vec![0xAAu8; w * h * 4];
        for y in 20..50 {
            for x in 40..100 {
                pixels[(y * w + x) * 4..][..3].copy_from_slice(&[210, 210, 210]);
            }
            rows[y * w * 4..][..w * 4].copy_from_slice(&pixels[y * w * 4..][..w * 4]);
        }
        let unit = frame(&mut encoder, &rows, Some(&[damage]), false);
        let types = nal_types(&unit);
        assert!(types.contains(&1) && !types.contains(&5), "a P frame: {types:?}");
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6);

        let unit = frame(&mut encoder, &pixels, None, true);
        assert!(nal_types(&unit).contains(&5), "an IDR where one was asked");
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6);
    }

    /// A region of odd sides, as one clipped to the edge of an odd desktop
    /// is, is coded by libx264 a pixel larger rather than refused, and the
    /// region's own pixels decode to the picture.
    #[test]
    fn libx264_codes_a_region_of_odd_sides() {
        frames_decode_to_the_picture(&Encoder::X264, (95, 69), 0);
    }

    /// libx264's stream is Constrained Baseline, the profile `ultrafast`
    /// comes to and every browser's decoder takes, with no B-frames to hold
    /// a picture back: each frame in is one frame out.
    #[test]
    fn libx264_codes_constrained_baseline_one_frame_in_one_out() {
        let (w, h) = (64usize, 48usize);
        let mut coder = Encoder::X264.coder().unwrap();
        let mut stream = coder.open(w as u16, h as u16).unwrap();
        let pixels = picture(w, h);
        stream.read_bgrx(&pixels, w * 4, None).unwrap();
        let mut unit = Vec::new();
        assert!(stream.encode(false, 90, &mut unit).unwrap());
        let sps = unit.windows(4).position(|s| s[..3] == [0, 0, 1] && s[3] & 0x1f == 7).expect("an SPS");
        let (profile, constraints) = (unit[sps + 4], unit[sps + 5]);
        assert_eq!(profile, 66, "Baseline");
        assert_ne!(constraints & 0x40, 0, "constraint_set1: Constrained Baseline");
        for _ in 0..5 {
            unit.clear();
            assert!(stream.encode(false, 90, &mut unit).unwrap(), "every frame in is a frame out");
        }
    }
}
