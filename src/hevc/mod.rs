//! HEVC at 4:4:4 for the video stream: NVIDIA's encoder through NVENC's own
//! API ([`nvenc`]), where the host has the NVIDIA driver and a GPU whose
//! encoder codes 4:4:4.
//!
//! The H.264 stream is 4:2:0, which halves a desktop's colour both ways: text
//! in colour, and coloured text's edges, are what it blurs. This codes the
//! same stream at 4:4:4, every pixel's colour its own, for a client that can
//! decode HEVC's Range Extensions. It is one more [`VideoCoder`] beside the
//! H.264 one ([`crate::h264`]), with what makes a stream the same: every
//! frame at the quantizer its encode is handed, from the same dial onto the
//! same QP 40 down to 16 ([`crate::h264::qp`]), HEVC's quantizer stepping as
//! H.264's does; no B-frames, one frame in and one out; no keyframe but the
//! first and those asked; the parameter sets — VPS, SPS and PPS — ahead of
//! every IDR; BT.601 at studio swing, said in the VUI. The picture coded is
//! the stream's rounded up to even sides and to the smallest the encoder
//! codes, black past the stream's own, and the gateway clips it.
//!
//! **Which encoder.** NVENC alone: VA-API has no NVIDIA encoder, and Intel's
//! `VAProfileHEVCMain444` is a backend still to come. The daemon looks once,
//! with `hardware_h264` on ([`Encoder::find`]): the driver loaded, a CUDA
//! context on the first GPU, its HEVC encoder asked whether it codes 4:4:4,
//! and two frames of a small picture coded at two quantizers, each decoded by
//! libavcodec and its slices' QP read back from its headers
//! ([`syntax::Headers`]), since libavcodec's HEVC decoder says none. With
//! none working the video stream is H.264 alone.
//!
//! **What a session holds.** A CUDA context of its own ([`Encoder::coder`]),
//! as a VA display is one session's, and a stream's encoder in it, its
//! picture converted from the framebuffer where it changed into 4:4:4 planes
//! on the processor and those rows alone uploaded to the GPU.
//!
//! Tested against libavcodec's software HEVC decoder ([`crate::decode`]),
//! which shares nothing with NVENC, where the machine has an NVENC that codes
//! 4:4:4, its test saying so and passing where it has none.

mod nvenc;
pub mod syntax;
mod sys;

use std::sync::Arc;

use anyhow::Context as _;
use log::{debug, info};

use crate::h264::qp;
use crate::rfb::{CoderError, Rect, VideoCoder, VideoStream};

/// The HEVC 4:4:4 encoder the startup found, which every session's coder
/// opens: the first NVIDIA GPU's, through NVENC.
#[derive(Debug)]
pub struct Encoder {
    /// The smallest and largest pictures the encoder codes.
    min: (usize, usize),
    max: (usize, usize),
}

/// The picture the probe codes two frames of, rounded up to the encoder's
/// smallest.
const PROBE_SIZE: (u16, u16) = (64, 64);

impl Encoder {
    /// The encoder, where `hardware` asks for it and the host has one that
    /// codes two frames; `None` otherwise, the reason logged.
    pub fn find(hardware: bool) -> Option<Self> {
        if !hardware {
            info!("hardware_h264 is off: NVENC's HEVC 4:4:4 encoder is not looked for");
            return None;
        }
        match Self::probe() {
            Ok(encoder) => {
                info!("hardware HEVC 4:4:4: the first NVIDIA GPU through NVENC");
                Some(encoder)
            }
            Err(e) => {
                info!("no HEVC 4:4:4 encoder: {e:#}");
                None
            }
        }
    }

    fn probe() -> anyhow::Result<Self> {
        let context = Arc::new(nvenc::Context::open()?);
        let caps = nvenc::Stream::caps(&context)?;
        let size = |(w, h): (u32, u32)| (w as usize, h as usize);
        let encoder = Self { min: size(caps.min), max: size(caps.max) };
        debug!("NVENC's HEVC codes 4:4:4 from {:?} to {:?}", encoder.min, encoder.max);
        encoder.try_frames(Coder { context, min: encoder.min, max: encoder.max }).context("coding two trial frames")?;
        Ok(encoder)
    }

    /// A coder for one session: a CUDA context of its own.
    pub fn coder(&self) -> anyhow::Result<Coder> {
        Ok(Coder { context: Arc::new(nvenc::Context::open()?), min: self.min, max: self.max })
    }

    /// Code two frames of a small picture of noise with `coder`: an IDR at
    /// the dial's middle, its parameter sets ahead of it, and a P frame at
    /// its fine end, each decoded by libavcodec as 4:4:4 of the coded size,
    /// and each slice's QP, read from the headers, the one asked. An encoder
    /// that cannot move its quantizer between frames is not one.
    fn try_frames(&self, mut coder: Coder) -> anyhow::Result<()> {
        let (w, h) = PROBE_SIZE;
        let mut stream = coder.open(w, h).map_err(|e| anyhow::anyhow!("{e}"))?;
        let coded = coded_size((w.into(), h.into()), self.min);
        let mut decoder = crate::decode::Decoder::hevc()?;
        let mut headers = syntax::Headers::default();
        for (n, (quality, kind)) in [(50, "an IDR picture"), (100, "a P frame")].into_iter().enumerate() {
            stream.read_bgrx(&crate::h264::noise(w.into(), h.into(), 0x9E37_79B9 + n as u32), usize::from(w) * 4, None).map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut out = Vec::new();
            anyhow::ensure!(stream.encode(n == 0, quality, &mut out).map_err(|e| anyhow::anyhow!("{e}"))?, "the encoder made no frame {n}");
            let types = syntax::nal_types(&out);
            let idr = types.contains(&syntax::IDR_W_RADL) || types.contains(&syntax::IDR_N_LP);
            anyhow::ensure!(idr == (n == 0), "frame {n} is not {kind}: {types:?}");
            if n == 0 {
                anyhow::ensure!([syntax::VPS, syntax::SPS, syntax::PPS].iter().all(|t| types.contains(t)), "the IDR has not its parameter sets: {types:?}");
            }
            let mut picture = Vec::new();
            let decoded = decoder.decode(&out, &mut picture).context("decoding a trial frame")?.with_context(|| format!("frame {n} decoded to no picture"))?;
            anyhow::ensure!(decoded.full_chroma && (decoded.width, decoded.height) == coded, "frame {n} decoded to {decoded:?}, not 4:4:4 at {coded:?}");
            let asked = i32::from(qp(quality));
            let slices = headers.read(&out).context("reading a trial frame's headers")?;
            anyhow::ensure!(!slices.is_empty() && slices.iter().all(|s| s.qp == asked), "frame {n} was asked QP {asked} and coded at {:?}", slices.iter().map(|s| s.qp).collect::<Vec<_>>());
        }
        Ok(())
    }
}

/// The size a stream of `size` is coded at: even sides, at least `min`.
fn coded_size((w, h): (usize, usize), (min_w, min_h): (usize, usize)) -> (usize, usize) {
    (w.max(min_w).next_multiple_of(2), h.max(min_h).next_multiple_of(2))
}

/// One session's HEVC coder: a CUDA context, which each of its streams takes
/// an encoder in.
pub struct Coder {
    context: Arc<nvenc::Context>,
    min: (usize, usize),
    max: (usize, usize),
}

impl VideoCoder for Coder {
    fn open(&mut self, width: u16, height: u16) -> Result<Box<dyn VideoStream>, CoderError> {
        let opened: anyhow::Result<Box<dyn VideoStream>> = (|| {
            anyhow::ensure!(width > 0 && height > 0, "a {width}x{height} picture has no pixels");
            let size = (usize::from(width), usize::from(height));
            let coded = coded_size(size, self.min);
            anyhow::ensure!(coded.0 <= self.max.0 && coded.1 <= self.max.1, "a {width}x{height} picture is past the {:?} NVENC codes", self.max);
            let encoder = nvenc::Stream::open(self.context.clone(), coded, qp(crate::rfb::QUALITY))?;
            let stream: Box<dyn VideoStream> = Box::new(Stream { planes: Planes::new(size, coded), encoder });
            Ok(stream)
        })();
        opened.map_err(|e| format!("{e:#}").into())
    }
}

/// One stream: the 4:4:4 picture in front of the encoder, and the encoder.
struct Stream {
    planes: Planes,
    encoder: nvenc::Stream,
}

impl VideoStream for Stream {
    fn read_bgrx(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> Result<(), CoderError> {
        self.planes.read_bgrx(pixels, stride, changed).map_err(|e| format!("{e:#}").into())
    }

    fn encode(&mut self, keyframe: bool, quality: u8, out: &mut Vec<u8>) -> Result<bool, CoderError> {
        let coded: anyhow::Result<bool> = (|| {
            anyhow::ensure!(self.planes.filled, "a stream was asked for a frame before any picture was read into it");
            let rows = self.planes.take_rows();
            let [y, u, v] = &self.planes.planes;
            let upload = nvenc::Upload { planes: [y, u, v], stride: self.planes.stride, rows: rows.as_deref() };
            self.encoder.frame(&upload, keyframe, qp(quality), out)
        })();
        coded.map_err(|e| format!("{e:#}").into())
    }
}

/// A stream's picture as its encoder takes it: three planes, `Y`, `Cb`, `Cr`,
/// BT.601 at studio swing, each the coded size and converted from the
/// framebuffer where it changed, with the rows that changed since the frame
/// before, which are all the encoder uploads.
struct Planes {
    size: (usize, usize),
    /// Of every plane: a multiple of 64.
    stride: usize,
    /// The coded picture's rows and one to spare, so a rectangle's slice of a
    /// plane is a whole stride a row from its left edge to the last.
    planes: [Vec<u8>; 3],
    /// The planes hold a picture: one was read whole.
    filled: bool,
    /// The rows read since the frame before, or `None` for every row.
    rows: Option<Vec<(usize, usize)>>,
    /// A rectangle's last row, copied: `B, G, R, X`.
    row: Vec<u8>,
}

impl Planes {
    fn new(size: (usize, usize), coded: (usize, usize)) -> Self {
        let stride = coded.0.next_multiple_of(64);
        let len = stride * (coded.1 + 1);
        Self { size, stride, planes: [vec![16; len], vec![128; len], vec![128; len]], filled: false, rows: None, row: Vec::new() }
    }

    /// Read `changed` of `pixels` into the planes, or the whole picture where
    /// it is `None` or none has been read yet.
    fn read_bgrx(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> anyhow::Result<()> {
        let (w, h) = self.size;
        anyhow::ensure!(stride >= w * 4 && pixels.len() >= (h - 1) * stride + w * 4, "a {w}x{h} picture at a stride of {stride} does not fit in {} bytes", pixels.len());
        match changed.filter(|_| self.filled) {
            Some(rects) => {
                for rect in rects {
                    if let Some(rows) = self.convert(pixels, stride, *rect)?
                        && let Some(read) = &mut self.rows
                    {
                        read.push(rows);
                    }
                }
            }
            None => {
                self.convert(pixels, stride, Rect { x: 0, y: 0, width: w as u16, height: h as u16 })?;
                self.filled = true;
                self.rows = None;
            }
        }
        Ok(())
    }

    /// The rows to upload for the next frame, merged, and from then on none:
    /// `None` for every row.
    fn take_rows(&mut self) -> Option<Vec<(usize, usize)>> {
        let mut rows = self.rows.replace(Vec::new())?;
        rows.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(rows.len());
        for (top, bottom) in rows {
            match merged.last_mut() {
                Some(last) if top <= last.1 => last.1 = last.1.max(bottom),
                _ => merged.push((top, bottom)),
            }
        }
        Some(merged)
    }

    /// Convert `rect` of `pixels`, cut to the picture, into the planes; the
    /// rows it spans, or `None` where it holds no pixel.
    ///
    /// The converter takes its source a whole stride a row, so a rectangle's
    /// last row, which may end the framebuffer short of one, is converted
    /// from a copy of its pixels, and every row above it where it lies.
    fn convert(&mut self, pixels: &[u8], stride: usize, rect: Rect) -> anyhow::Result<Option<(usize, usize)>> {
        let (w, h) = self.size;
        let (left, top) = (usize::from(rect.x).min(w), usize::from(rect.y).min(h));
        let right = (usize::from(rect.x) + usize::from(rect.width)).min(w);
        let bottom = (usize::from(rect.y) + usize::from(rect.height)).min(h);
        if left >= right || top >= bottom {
            return Ok(None);
        }
        let (rw, last) = (right - left, bottom - 1);
        if last > top {
            self.yuv444(&pixels[top * stride + left * 4..last * stride + left * 4], stride, (left, top), rw, last - top)?;
        }
        let mut row = std::mem::take(&mut self.row);
        row.clear();
        row.extend_from_slice(&pixels[last * stride + left * 4..last * stride + right * 4]);
        let converted = self.yuv444(&row, rw * 4, (left, last), rw, 1);
        self.row = row;
        converted?;
        Ok(Some((top, bottom)))
    }

    /// Convert `height` rows of `width` pixels, `B, G, R, X` rows `stride`
    /// apart, into the planes at `(left, top)`: BT.601 at studio swing.
    fn yuv444(&mut self, bgrx: &[u8], stride: usize, (left, top): (usize, usize), width: usize, height: usize) -> anyhow::Result<()> {
        use yuv::{BufferStoreMut, YuvConversionMode, YuvPlanarImageMut, YuvRange, YuvStandardMatrix};
        let ps = self.stride;
        let span = top * ps + left..(top + height) * ps + left;
        let [y, u, v] = &mut self.planes;
        let mut image = YuvPlanarImageMut {
            y_plane: BufferStoreMut::Borrowed(&mut y[span.clone()]),
            y_stride: ps as u32,
            u_plane: BufferStoreMut::Borrowed(&mut u[span.clone()]),
            u_stride: ps as u32,
            v_plane: BufferStoreMut::Borrowed(&mut v[span]),
            v_stride: ps as u32,
            width: width as u32,
            height: height as u32,
        };
        yuv::bgra_to_yuv444(&mut image, bgrx, stride as u32, YuvRange::Limited, YuvStandardMatrix::Bt601, YuvConversionMode::Professional)
            .map_err(|e| anyhow::anyhow!("converting a {width}x{height} rectangle to 4:4:4: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coded_picture_is_even_and_at_least_the_encoders_smallest() {
        assert_eq!(coded_size((1920, 1080), (145, 49)), (1920, 1080));
        assert_eq!(coded_size((161, 99), (145, 49)), (162, 100));
        assert_eq!(coded_size((64, 64), (145, 49)), (146, 64));
    }

    /// The rows a frame uploads are those its reads changed, merged, and
    /// every row where a read was whole.
    #[test]
    fn the_rows_uploaded_are_those_read_merged() {
        let (w, h) = (40, 30);
        let mut planes = Planes::new((w, h), (w, h));
        let pixels = vec![0x80; w * h * 4];
        planes.read_bgrx(&pixels, w * 4, Some(&[Rect { x: 0, y: 3, width: 2, height: 2 }])).unwrap();
        assert_eq!(planes.take_rows(), None, "the first read is whole");
        let rects = [Rect { x: 1, y: 10, width: 5, height: 4 }, Rect { x: 9, y: 2, width: 3, height: 3 }, Rect { x: 20, y: 12, width: 3, height: 6 }, Rect { x: 0, y: 29, width: 4, height: 9 }];
        planes.read_bgrx(&pixels, w * 4, Some(&rects)).unwrap();
        assert_eq!(planes.take_rows(), Some(vec![(2, 5), (10, 18), (29, 30)]));
        assert_eq!(planes.take_rows(), Some(vec![]), "nothing read since");
        planes.read_bgrx(&pixels, w * 4, None).unwrap();
        assert_eq!(planes.take_rows(), None, "a whole read uploads every row");
    }

    /// Every row and column of a rectangle read is converted, its last
    /// among them, and nothing outside it: the planes' luma is the picture's
    /// at studio swing within a level.
    #[test]
    fn a_rectangle_read_is_converted_whole() {
        let (w, h) = (64, 40);
        let mut planes = Planes::new((w, h), (w + 2, h));
        let before = picture(w, h, |x, y| (40 + x / 2 + y) as u8);
        planes.read_bgrx(&before, w * 4, None).unwrap();
        let mut after = before.clone();
        for y in 20..40 {
            for x in 31..60 {
                after[(y * w + x) * 4..][..3].copy_from_slice(&[200, 200, 200]);
            }
        }
        planes.read_bgrx(&after, w * 4, Some(&[Rect { x: 31, y: 20, width: 29, height: 20 }])).unwrap();
        for y in 0..h {
            for x in 0..w {
                let grey = after[(y * w + x) * 4];
                let expected = 16.0 + f64::from(grey) * 219.0 / 255.0;
                let got = f64::from(planes.planes[0][y * planes.stride + x]);
                assert!((got - expected).abs() <= 1.0, "luma at {x},{y}: {got}, not {expected:.1}");
            }
        }
    }

    /// A grey `B, G, R, X` picture of luma `luma(x, y)`.
    fn picture(w: usize, h: usize, luma: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| [luma(x, y), luma(x, y), luma(x, y), 0]).collect()
    }

    /// A picture's colour where it changes from one pixel to the next comes
    /// back pixel for pixel at 4:4:4, which 4:2:0 cannot: a column of red
    /// beside one of blue, each a pixel wide, is each its own chroma.
    #[test]
    fn the_planes_keep_every_pixels_colour() {
        let (w, h) = (8, 4);
        let mut pixels = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                pixels[(y * w + x) * 4..][..3].copy_from_slice(if x % 2 == 0 { &[0, 0, 255] } else { &[255, 0, 0] });
            }
        }
        let mut planes = Planes::new((w, h), (w, h));
        planes.read_bgrx(&pixels, w * 4, None).unwrap();
        let [_, u, v] = &planes.planes;
        for x in 0..w {
            // Red: Cr high, Cb low; blue the other way.
            let (cb, cr) = (u[x], v[x]);
            assert_eq!(x % 2 == 0, cr > 200 && cb < 128, "pixel {x}: Cb {cb}, Cr {cr}");
        }
    }

    /// Decode `unit` and hold its luma to `pixels`'s grey within
    /// [`LUMA_SLACK`] levels after studio swing, and its chroma within
    /// [`CHROMA_SLACK`] of flat: a 4:4:4 picture of the coded size. The
    /// hardware's pixels are not held to a software encoder's, only to a
    /// rectangle's being coded where it changed; a sharp edge at the fine end
    /// of the dial rings by a few levels at its corners.
    fn decodes_to(decoder: &mut crate::decode::Decoder, unit: &[u8], pixels: &[u8], (w, h): (usize, usize), coded: (usize, usize)) {
        let mut out = Vec::new();
        let decoded = decoder.decode(unit, &mut out).unwrap().expect("a picture for every unit");
        assert!(decoded.full_chroma, "4:4:4");
        assert_eq!((decoded.width, decoded.height), coded);
        let plane = coded.0 * coded.1;
        for y in 0..h {
            for x in 0..w {
                let grey = pixels[(y * w + x) * 4];
                let expected = 16.0 + f64::from(grey) * 219.0 / 255.0;
                let got = f64::from(out[y * coded.0 + x]);
                assert!((got - expected).abs() <= LUMA_SLACK, "luma at {x},{y}: {got}, not {expected:.1}");
                for (n, chroma) in [out[plane + y * coded.0 + x], out[2 * plane + y * coded.0 + x]].into_iter().enumerate() {
                    assert!(chroma.abs_diff(128) <= CHROMA_SLACK, "chroma {n} at {x},{y}: {chroma}");
                }
            }
        }
    }

    /// How far a decoded pixel's luma may stand from the picture's, and its
    /// chroma from grey's.
    const LUMA_SLACK: f64 = 12.0;
    const CHROMA_SLACK: u8 = 4;

    /// The frames NVENC codes decode with libavcodec's software decoder to
    /// the picture, damage alone read for a P frame, an IDR where asked; and
    /// its quantizer follows the dial frame by frame, read back from each
    /// slice's header, the coarse end in well under half the bytes of the
    /// fine. Passes with a word where this machine has no NVENC that codes
    /// 4:4:4.
    #[test]
    fn nvencs_frames_decode_to_the_picture_with_libavcodecs_software_decoder() {
        let Some(encoder) = Encoder::find(true) else {
            eprintln!("no NVENC HEVC 4:4:4 encoder on this machine; the HEVC coder is not exercised");
            return;
        };
        for (w, h) in [(200usize, 120usize), (201, 121), (64, 40)] {
            let coded = coded_size((w, h), encoder.min);
            let mut coder = encoder.coder().unwrap();
            let mut stream = coder.open(w as u16, h as u16).unwrap();
            let mut decoder = crate::decode::Decoder::hevc().unwrap();
            let mut headers = syntax::Headers::default();
            let luma = |x: usize, y: usize| (40 + x / 2 + y) as u8;
            let mut pixels = picture(w, h, luma);

            stream.read_bgrx(&pixels, w * 4, Some(&[Rect { x: 1, y: 1, width: 1, height: 1 }])).unwrap();
            let mut unit = Vec::new();
            assert!(stream.encode(false, 100, &mut unit).unwrap(), "a frame of the first picture");
            let types = syntax::nal_types(&unit);
            let idr = types.contains(&syntax::IDR_W_RADL) || types.contains(&syntax::IDR_N_LP);
            assert!(idr && [syntax::VPS, syntax::SPS, syntax::PPS].iter().all(|t| types.contains(t)), "the first frame is an IDR with its parameter sets: {types:?}");
            decodes_to(&mut decoder, &unit, &pixels, (w, h), coded);
            headers.read(&unit).unwrap();
            let sps = headers.sps(0).expect("the stream's sequence parameter set").clone();
            assert_eq!((sps.profile_idc, sps.chroma_format_idc, sps.bit_depth), (4, 3, (8, 8)), "Range Extensions, 4:4:4, 8 bits");
            assert_eq!((sps.size.0 - sps.crop[0] - sps.crop[1], sps.size.1 - sps.crop[2] - sps.crop[3]), (coded.0 as u32, coded.1 as u32), "the conformance window is the coded picture");
            assert_eq!(sps.colour, Some((false, 6, 6, 6)), "BT.601 at studio swing in the VUI");

            // A rectangle changes, read from a buffer whose other rows are not
            // the picture, and the frame after it is a P frame.
            let damage = Rect { x: 31, y: 20, width: 29, height: 20 };
            let mut rows = vec![0xAAu8; w * h * 4];
            for y in 20..40 {
                for x in 31..60 {
                    pixels[(y * w + x) * 4..][..3].copy_from_slice(&[200, 200, 200]);
                }
                rows[y * w * 4..][..w * 4].copy_from_slice(&pixels[y * w * 4..][..w * 4]);
            }
            stream.read_bgrx(&rows, w * 4, Some(&[damage])).unwrap();
            unit.clear();
            assert!(stream.encode(false, 100, &mut unit).unwrap());
            let types = syntax::nal_types(&unit);
            assert!(!types.contains(&syntax::IDR_W_RADL) && !types.contains(&syntax::IDR_N_LP), "a P frame: {types:?}");
            decodes_to(&mut decoder, &unit, &pixels, (w, h), coded);

            // A keyframe asked for is one, with its parameter sets.
            unit.clear();
            assert!(stream.encode(true, 100, &mut unit).unwrap());
            let types = syntax::nal_types(&unit);
            assert!(types.contains(&syntax::SPS) && (types.contains(&syntax::IDR_W_RADL) || types.contains(&syntax::IDR_N_LP)), "an IDR with its parameter sets where one was asked: {types:?}");
            decodes_to(&mut decoder, &unit, &pixels, (w, h), coded);
        }

        // The quantizer, walked across the dial over noise, a new picture
        // every frame so none is free, with no keyframe but the first.
        let (w, h) = (192usize, 128usize);
        let mut coder = encoder.coder().unwrap();
        let mut stream = coder.open(w as u16, h as u16).unwrap();
        let mut decoder = crate::decode::Decoder::hevc().unwrap();
        let mut headers = syntax::Headers::default();
        let mut sizes = Vec::new();
        for (n, &quality) in [90u8, 90, 50, 1, 100, 20, 70, 1, 100].iter().enumerate() {
            stream.read_bgrx(&crate::h264::noise(w, h, 0x9E37_79B9 + n as u32), w * 4, None).unwrap();
            let mut unit = Vec::new();
            assert!(stream.encode(false, quality, &mut unit).unwrap());
            let types = syntax::nal_types(&unit);
            assert_eq!(types.contains(&syntax::IDR_W_RADL) || types.contains(&syntax::IDR_N_LP), n == 0, "frame {n}: an IDR only first: {types:?}");
            let mut out = Vec::new();
            decoder.decode(&unit, &mut out).unwrap().expect("a picture");
            let slices = headers.read(&unit).unwrap();
            let asked = i32::from(qp(quality));
            assert!(!slices.is_empty() && slices.iter().all(|s| s.qp == asked), "frame {n} at quality {quality}: slices at {slices:?}, not {asked}");
            sizes.push((quality, unit.len()));
        }
        let fine = sizes.iter().filter(|(q, _)| *q == 100).map(|(_, s)| *s).min().unwrap();
        let coarse = sizes.iter().filter(|(q, _)| *q == 1).map(|(_, s)| *s).max().unwrap();
        assert!(coarse * 2 < fine, "the dial at 1 codes noise in under half the bytes of 100: {sizes:?}");
    }
}
