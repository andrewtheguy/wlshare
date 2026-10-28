//! H.264 for the tiles picture's moving regions, through the system's
//! libavcodec, the one already linked for the camera's decoder: the host's
//! hardware encoder by VA-API, as its `h264_vaapi` encoder, where the host has
//! one, and libx264 at its fastest preset where it has none.
//!
//! A region of the tiles picture is a stream over its rectangle alone
//! ([`crate::rfb::tiles`]), coded by the [`RegionCoder`] the daemon hands
//! the encoder. This is that coder. On a host whose GPU has an H.264 encoder
//! a VA-API driver reaches — Intel's iHD, AMD's Mesa — a region that moves
//! costs the processor its conversion to NV12 and nothing of the search,
//! which is what the tiles picture is for: the least of the encoder's time on
//! a link with room. On a host with none, libx264's `ultrafast` codes it in
//! about a third of the processor's time a frame that VP9 at libvpx's fastest
//! speed takes, for about twice the bytes, which a link with room has.
//!
//! **What a stream is.** Constant quantizer, pinned to the dial the client's
//! list names, with no bitrate and nothing dropped: the
//! 1–100 dial maps onto QP 40 down to 16, finest last. No B-frames, one frame
//! in and one out, so a frame is on the wire the moment it is coded: the
//! hardware's at High profile with CABAC and `async_depth` 1, libx264's at
//! Constrained Baseline, the profile `ultrafast` comes to, with its
//! `zerolatency` tuning, no scene cut, and the VP9 encoder's threads, as
//! slices. BT.601 at studio swing, said in the stream's VUI. The picture coded is the region's
//! rounded up to even sides, which libx264 requires and 4:2:0 cannot crop
//! below: a region of an odd width or height is coded a pixel larger, its last
//! row again below it and black to its right, and the gateway clips it to the
//! region. Each frame is one Annex B access unit, the parameter sets ahead of
//! every IDR, and the first frame of a stream is one. The dial is the
//! stream's for its life: a held tiles picture's streams are at the dial its
//! list names whatever the link does. No AUD; no SEI from
//! the hardware, and from libx264 the one naming its version and options,
//! ahead of a stream's first IDR, which a decoder passes over.
//!
//! **Which encoder.** The daemon looks once, at startup ([`Encoder::find`]):
//! with `hardware_h264` on, each render node in turn, the low-power encoder
//! first — Intel's fixed-function block, which leaves the execution units to
//! the desktop — and the shader one after it, each tried on a frame of a
//! small picture, since a driver lists more profiles than it will open; with
//! none working, or the key off, libx264, tried the same way. What works is
//! what every session uses; with nothing working the daemon does not start,
//! since a region has no other codec. The low-power block codes CQP without
//! the HuC firmware, which only bitrate control needs.
//!
//! **Failure ends the session.** A coder that cannot be opened, refuses a
//! stream or fails one midway fails the tiles picture's encode
//! ([`crate::rfb::tiles::TilesEncoder`]), and with it the session that
//! asked for the picture. Each session opens the device itself
//! ([`Encoder::coder`]), so a VA display is one thread's and the sessions
//! share nothing of it.
//!
//! Tested against libavcodec's own H.264 decoder ([`crate::decode`]), which
//! shares nothing with either encoder: libx264's on every machine, the
//! hardware's where there is one, its test saying so and passing where there
//! is not, since the machines that build this seldom have one.

use std::ffi::{CStr, CString, c_char};
use std::ptr;

use anyhow::Context as _;
use ffmpeg_sys_next as ff;
use log::{debug, info};
use crate::rfb::tiles::{RegionCoder, RegionError, RegionStream};
use crate::rfb::vp9::Rect;

/// The H.264 encoder the startup found, which every session's coder opens.
#[derive(Debug)]
pub enum Encoder {
    /// The host's GPU, through VA-API.
    Vaapi {
        /// The render node, `/dev/dri/renderD128` or a sibling.
        device: CString,
        /// Whether the low-power encoder is the one that works.
        low_power: bool,
    },
    /// libx264, on the processor.
    X264,
}

/// Which encoder a coder and its streams are: [`Encoder`] less the device's
/// path, which the coder holds open instead.
#[derive(Clone, Copy, Debug)]
enum Backend {
    Vaapi { low_power: bool },
    X264,
}

/// The picture the probe codes one frame of, small enough for any encoder.
const PROBE_SIZE: (u16, u16) = (64, 64);

/// Where render nodes are.
const DRI: &str = "/dev/dri";

/// libx264's preset: its least search, the least of the processor's time.
const X264_PRESET: &str = "ultrafast";

impl Encoder {
    /// The encoder the tiles' regions are coded by: the hardware's where
    /// `hardware` asks for it and [`Self::vaapi`] finds one, libx264
    /// otherwise, each tried on a frame first. An error with neither working.
    pub fn find(hardware: bool) -> anyhow::Result<Self> {
        if hardware {
            if let Some(encoder) = Self::vaapi() {
                return Ok(encoder);
            }
        } else {
            info!("hardware_h264 is off: the hardware H.264 encoder is not looked for");
        }
        Self::X264.try_frame().context("no H.264 encoder for the tiles' regions: libx264 codes nothing here")?;
        info!("software H.264 for the tiles' regions: libx264 at {X264_PRESET}");
        Ok(Self::X264)
    }

    /// Look for a working VA-API H.264 encoder: each render node in turn, the
    /// low-power encoder ahead of the shader one, each opened on
    /// [`PROBE_SIZE`] and asked for a frame. `None` with nothing working, the
    /// reasons logged.
    fn vaapi() -> Option<Self> {
        let mut nodes: Vec<std::path::PathBuf> = match std::fs::read_dir(DRI) {
            Ok(entries) => entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD"))).collect(),
            Err(e) => {
                info!("no hardware H.264 encoder: {DRI} cannot be read ({e})");
                return None;
            }
        };
        nodes.sort();
        if nodes.is_empty() {
            info!("no hardware H.264 encoder: no render node under {DRI}");
            return None;
        }
        for node in nodes {
            let device = CString::new(node.to_string_lossy().into_owned()).expect("a path has no NUL");
            for low_power in [true, false] {
                let encoder = Self::Vaapi { device: device.clone(), low_power };
                match encoder.try_frame() {
                    Ok(()) => {
                        info!("hardware H.264 for the tiles' regions: {} through VA-API, its {} encoder", node.display(), kind(low_power));
                        return Some(encoder);
                    }
                    Err(e) => debug!("the {} encoder on {}: {e:#}", kind(low_power), node.display()),
                }
            }
            info!("{}: VA-API opens no H.264 encoder there (run with RUST_LOG=debug for each attempt)", node.display());
        }
        info!("no hardware H.264 encoder");
        None
    }

    /// What codes the regions, for the log: `H.264 by …`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Vaapi { .. } => "the hardware",
            Self::X264 => "libx264",
        }
    }

    /// Open a coder and code one grey frame of a small picture with it.
    fn try_frame(&self) -> anyhow::Result<()> {
        let mut coder = self.coder()?;
        let (w, h) = PROBE_SIZE;
        let mut stream = coder.stream(w, h, 50)?;
        let pixels = vec![0x80u8; usize::from(w) * usize::from(h) * 4];
        stream.read_bgrx(&pixels, usize::from(w) * 4, None).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut out = Vec::new();
        anyhow::ensure!(stream.encode(true, &mut out).map_err(|e| anyhow::anyhow!("{e}"))?, "the encoder made no frame of the picture");
        anyhow::ensure!(nal_types(&out).contains(&5), "the first frame is not an IDR picture");
        Ok(())
    }

    /// A coder for one session: for the hardware, its own VA display on the
    /// device.
    pub fn coder(&self) -> anyhow::Result<Coder> {
        let (path, low_power) = match self {
            Self::Vaapi { device, low_power } => (device, *low_power),
            Self::X264 => return Ok(Coder { device: ptr::null_mut(), backend: Backend::X264 }),
        };
        let mut device = ptr::null_mut();
        // SAFETY: the path is a NUL-terminated string and the reference is
        // checked before use; what libavutil allocates is freed by `Drop`.
        unsafe {
            check(ff::av_hwdevice_ctx_create(&mut device, ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI, path.as_ptr(), ptr::null_mut(), 0), "opening the VA-API device")
                .with_context(|| format!("VA-API on {}", path.to_string_lossy()))?;
        }
        anyhow::ensure!(!device.is_null(), "libavutil opened no device");
        Ok(Coder { device, backend: Backend::Vaapi { low_power } })
    }
}

/// Which VA-API encoder: the fixed-function block or the shader one.
fn kind(low_power: bool) -> &'static str {
    if low_power { "low-power" } else { "shader" }
}

/// One session's H.264 coder: for the hardware, a VA display, which each of
/// its streams takes an encoder on; for libx264, nothing until a stream.
pub struct Coder {
    /// The VA display; null for libx264.
    device: *mut ff::AVBufferRef,
    backend: Backend,
}

// SAFETY: the device reference is this value's alone, and it and every stream
// opened on it are used from one thread at a time, the session's.
unsafe impl Send for Coder {}

impl Coder {
    /// A stream over a `width`×`height` region at `quality`.
    fn stream(&mut self, width: u16, height: u16, quality: u8) -> anyhow::Result<Stream> {
        anyhow::ensure!(width > 0 && height > 0, "a {width}x{height} region has no pixels");
        let (w, h) = (usize::from(width), usize::from(height));
        // Strides a multiple of 64, which leaves an odd region's coded picture
        // its extra column, and the coded picture's rows and one to spare, so
        // the bottom row pair's planes are a whole stride past its left edge.
        let stride = w.next_multiple_of(64);
        let mut stream = Stream {
            device: ptr::null_mut(),
            frames: ptr::null_mut(),
            context: ptr::null_mut(),
            hw_frame: ptr::null_mut(),
            sw_frame: ptr::null_mut(),
            packet: ptr::null_mut(),
            size: (width, height),
            coded: (w.next_multiple_of(2) as i32, h.next_multiple_of(2) as i32),
            stride,
            pair: Vec::new(),
            y: vec![16; stride * (h.next_multiple_of(2) + 1)],
            uv: vec![128; stride * (h.div_ceil(2) + 1)],
            filled: false,
            quality,
            backend: self.backend,
            pts: 0,
        };
        // SAFETY: each allocation is checked, and a failure part way through
        // frees what came before it through `Drop`.
        unsafe {
            if let Backend::Vaapi { .. } = self.backend {
                stream.device = ff::av_buffer_ref(self.device);
                stream.hw_frame = ff::av_frame_alloc();
                anyhow::ensure!(!stream.device.is_null() && !stream.hw_frame.is_null(), "libavcodec could not allocate a stream");
            }
            stream.sw_frame = ff::av_frame_alloc();
            stream.packet = ff::av_packet_alloc();
            anyhow::ensure!(!stream.sw_frame.is_null() && !stream.packet.is_null(), "libavcodec could not allocate a stream");
            let sw = &mut *stream.sw_frame;
            sw.format = ff::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
            sw.width = stream.coded.0;
            sw.height = stream.coded.1;
            sw.data[0] = stream.y.as_mut_ptr();
            sw.data[1] = stream.uv.as_mut_ptr();
            sw.linesize[0] = stride as i32;
            sw.linesize[1] = stride as i32;
            stream.open()?;
        }
        Ok(stream)
    }
}

impl Drop for Coder {
    fn drop(&mut self) {
        // SAFETY: the reference is this value's and `av_buffer_unref` takes null.
        unsafe { ff::av_buffer_unref(&mut self.device) }
    }
}

impl RegionCoder for Coder {
    fn open(&mut self, width: u16, height: u16, quality: u8) -> Result<Box<dyn RegionStream>, RegionError> {
        Ok(Box::new(self.stream(width, height, quality).map_err(|e| format!("{e:#}"))?))
    }
}

/// One region's stream: an encoder, the NV12 picture in front of it, and the
/// frames and packet it reuses — for the hardware, its encoder on the VA
/// display and the surface the picture is uploaded to.
pub struct Stream {
    /// The VA display and its surfaces; null for libx264.
    device: *mut ff::AVBufferRef,
    frames: *mut ff::AVBufferRef,
    context: *mut ff::AVCodecContext,
    /// The surface a frame is uploaded to; null for libx264.
    hw_frame: *mut ff::AVFrame,
    /// Points into `y` and `uv`, which it does not own.
    sw_frame: *mut ff::AVFrame,
    packet: *mut ff::AVPacket,
    size: (u16, u16),
    /// The size coded: `size` rounded up to even sides.
    coded: (i32, i32),
    stride: usize,
    /// The bottom row pair of a conversion, copied: `B, G, R, X`.
    pair: Vec<u8>,
    y: Vec<u8>,
    uv: Vec<u8>,
    /// The planes hold a picture: one was read whole.
    filled: bool,
    /// The dial the stream is coded at.
    quality: u8,
    backend: Backend,
    pts: i64,
}

// SAFETY: as `Coder`'s: owned pointers, one thread at a time.
unsafe impl Send for Stream {}

/// The QP a dial pins the quantizer to: 40 at 1, 16 at 100, as VP9's 8–63 is
/// walked by the dial the other way; H.264's step doubles every six.
fn qp(quality: u8) -> i32 {
    let quality = i32::from(quality.clamp(1, 100));
    40 - ((quality - 1) * 24 + 49) / 99
}

/// The frame rate the level is figured at: wlshare's default `max_fps`, the
/// most a region is asked for.
const FPS: i32 = 60;

impl Stream {
    /// Open the encoder at the stream's dial: for the hardware, the frames
    /// context on the device at the region's size and the codec context over
    /// it; for libx264, the codec context over the planes.
    unsafe fn open(&mut self) -> anyhow::Result<()> {
        // SAFETY: the caller holds the stream's pointers; each allocation is
        // checked, and `close` frees what this made.
        unsafe {
            self.close();
            let qp = qp(self.quality).to_string();
            let (name, pix_fmt, options) = match self.backend {
                Backend::Vaapi { low_power } => {
                    self.frames = ff::av_hwframe_ctx_alloc(self.device);
                    anyhow::ensure!(!self.frames.is_null(), "libavutil could not allocate a frames context");
                    let frames = &mut *((*self.frames).data as *mut ff::AVHWFramesContext);
                    frames.format = ff::AVPixelFormat::AV_PIX_FMT_VAAPI;
                    frames.sw_format = ff::AVPixelFormat::AV_PIX_FMT_NV12;
                    frames.width = self.coded.0;
                    frames.height = self.coded.1;
                    frames.initial_pool_size = 4;
                    check(ff::av_hwframe_ctx_init(self.frames), "making VA-API surfaces")?;
                    let options = vec![
                        ("rc_mode", "CQP"),
                        ("qp", qp.as_str()),
                        ("low_power", if low_power { "1" } else { "0" }),
                        ("profile", "high"),
                        ("coder", "cabac"),
                        ("async_depth", "1"),
                        ("idr_interval", "0"),
                        ("sei", "0"),
                        ("aud", "0"),
                    ];
                    (c"h264_vaapi", ff::AVPixelFormat::AV_PIX_FMT_VAAPI, options)
                }
                // `zerolatency` takes the lookahead and the frame threads away,
                // which would each hold frames back, and slices the picture
                // among the threads instead; a forced I picture is an IDR.
                Backend::X264 => (
                    c"libx264",
                    ff::AVPixelFormat::AV_PIX_FMT_NV12,
                    vec![("preset", X264_PRESET), ("tune", "zerolatency"), ("qp", qp.as_str()), ("forced-idr", "1"), ("x264-params", "scenecut=0")],
                ),
            };
            let codec = ff::avcodec_find_encoder_by_name(name.as_ptr());
            anyhow::ensure!(!codec.is_null(), "this libavcodec has no {} encoder", name.to_string_lossy());
            self.context = ff::avcodec_alloc_context3(codec);
            anyhow::ensure!(!self.context.is_null(), "libavcodec could not allocate an encoder");
            let ctx = &mut *self.context;
            ctx.width = self.coded.0;
            ctx.height = self.coded.1;
            ctx.pix_fmt = pix_fmt;
            ctx.time_base = ff::AVRational { num: 1, den: FPS };
            ctx.framerate = ff::AVRational { num: FPS, den: 1 };
            // Keyframes only where asked, the stream's first among them. 2^30
            // is libx264's infinite interval.
            ctx.gop_size = 1 << 30;
            ctx.max_b_frames = 0;
            ctx.colorspace = ff::AVColorSpace::AVCOL_SPC_SMPTE170M;
            ctx.color_primaries = ff::AVColorPrimaries::AVCOL_PRI_SMPTE170M;
            ctx.color_trc = ff::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE170M;
            ctx.color_range = ff::AVColorRange::AVCOL_RANGE_MPEG;
            match self.backend {
                Backend::Vaapi { .. } => {
                    ctx.hw_frames_ctx = ff::av_buffer_ref(self.frames);
                    anyhow::ensure!(!ctx.hw_frames_ctx.is_null(), "libavutil could not share the frames context");
                }
                Backend::X264 => ctx.thread_count = wlshare_vp9::encoder_threads() as i32,
            }
            let mut dict = ptr::null_mut();
            for (key, value) in options {
                let (key, value) = (CString::new(key).unwrap(), CString::new(value).unwrap());
                ff::av_dict_set(&mut dict, key.as_ptr(), value.as_ptr(), 0);
            }
            let opened = ff::avcodec_open2(self.context, codec, &mut dict);
            ff::av_dict_free(&mut dict);
            check(opened, &format!("opening the {} encoder", name.to_string_lossy()))?;
            Ok(())
        }
    }

    /// Free the encoder and its surfaces, if open.
    unsafe fn close(&mut self) {
        // SAFETY: each free takes a pointer this value owns, and accepts null.
        unsafe {
            ff::avcodec_free_context(&mut self.context);
            ff::av_buffer_unref(&mut self.frames);
        }
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

    fn frame(&mut self, keyframe: bool, out: &mut Vec<u8>) -> anyhow::Result<bool> {
        anyhow::ensure!(self.filled, "a stream was asked for a frame before any picture was read into it");
        // SAFETY: the pointers are this stream's; for the hardware the surface
        // is taken from the pool, filled from the planes the software frame
        // points into, coded and given back; libavcodec copies the software
        // frame, which owns no buffer, for libx264; and the packet's bytes are
        // read within its size.
        unsafe {
            let frame = match self.backend {
                Backend::Vaapi { .. } => {
                    check(ff::av_hwframe_get_buffer(self.frames, self.hw_frame, 0), "taking a VA-API surface")?;
                    let uploaded = ff::av_hwframe_transfer_data(self.hw_frame, self.sw_frame, 0);
                    if uploaded < 0 {
                        ff::av_frame_unref(self.hw_frame);
                        check(uploaded, "uploading the picture to the surface")?;
                    }
                    self.hw_frame
                }
                Backend::X264 => self.sw_frame,
            };
            (*frame).pts = self.pts;
            // The first picture of an encoder is an IDR whatever is asked.
            (*frame).pict_type = if keyframe { ff::AVPictureType::AV_PICTURE_TYPE_I } else { ff::AVPictureType::AV_PICTURE_TYPE_NONE };
            self.pts += 1;
            let sent = ff::avcodec_send_frame(self.context, frame);
            if let Backend::Vaapi { .. } = self.backend {
                ff::av_frame_unref(self.hw_frame);
            }
            check(sent, "encoding a frame")?;
            let mut coded = false;
            loop {
                let received = ff::avcodec_receive_packet(self.context, self.packet);
                if received == ff::AVERROR(ff::EAGAIN) || received == ff::AVERROR_EOF {
                    break;
                }
                check(received, "receiving a frame")?;
                let packet = &*self.packet;
                out.extend_from_slice(std::slice::from_raw_parts(packet.data, packet.size as usize));
                ff::av_packet_unref(self.packet);
                coded = true;
            }
            Ok(coded)
        }
    }
}

impl RegionStream for Stream {
    fn read_bgrx(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> Result<(), RegionError> {
        let (w, h) = self.size;
        match changed.filter(|_| self.filled) {
            Some(rects) => {
                for rect in rects {
                    self.convert(pixels, stride, *rect).map_err(|e| format!("{e:#}"))?;
                }
            }
            None => {
                self.convert(pixels, stride, Rect { x: 0, y: 0, width: w, height: h }).map_err(|e| format!("{e:#}"))?;
                self.filled = true;
            }
        }
        Ok(())
    }

    fn encode(&mut self, keyframe: bool, out: &mut Vec<u8>) -> Result<bool, RegionError> {
        self.frame(keyframe, out).map_err(|e| format!("{e:#}").into())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: each free takes a pointer this value owns, and accepts null;
        // the software frame points into this value's own vectors and owns no
        // buffer of its own.
        unsafe {
            self.close();
            ff::av_packet_free(&mut self.packet);
            ff::av_frame_free(&mut self.hw_frame);
            ff::av_frame_free(&mut self.sw_frame);
            ff::av_buffer_unref(&mut self.device);
        }
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

/// A libavcodec return value as a result: negative is an error code.
fn check(code: i32, what: &str) -> anyhow::Result<()> {
    if code >= 0 {
        return Ok(());
    }
    let mut text = [0 as c_char; 128];
    // SAFETY: the buffer is its stated length, and av_strerror terminates it.
    let message = unsafe {
        ff::av_strerror(code, text.as_mut_ptr(), text.len());
        CStr::from_ptr(text.as_ptr()).to_string_lossy().into_owned()
    };
    anyhow::bail!("{what}: {message} ({code})")
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
    /// one; and a stream at the dial's coarse end codes noise in far fewer
    /// bytes than one at its fine end. Within a few levels
    /// of the picture, and `slack` more: the hardware's pixels are not held
    /// to libx264's, only to a rectangle's being coded where it changed.
    fn frames_decode_to_the_picture(encoder: &Encoder, (w, h): (usize, usize), slack: u8) {
        let mut coder = encoder.coder().unwrap();
        let mut stream = coder.open(w as u16, h as u16, 100).unwrap();
        let mut pixels = picture(w, h);
        let mut decoder = crate::decode::Decoder::new().unwrap();

        stream.read_bgrx(&pixels, w * 4, Some(&[Rect { x: 1, y: 1, width: 1, height: 1 }])).unwrap();
        let mut unit = Vec::new();
        assert!(stream.encode(false, &mut unit).unwrap(), "a frame of the first picture");
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
        assert!(stream.encode(false, &mut unit).unwrap());
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
        assert!(stream.encode(false, &mut unit).unwrap());
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6 + slack);

        // A keyframe asked for is one, with its parameter sets.
        unit.clear();
        assert!(stream.encode(true, &mut unit).unwrap());
        let types = nal_types(&unit);
        assert!(types.contains(&7) && types.contains(&8) && types.contains(&5), "an IDR with its parameter sets where one was asked: {types:?}");
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6 + slack);

        // A picture of noise, which no prediction codes for free, on streams
        // at the dial's two ends: the coarser quantizer codes it in well
        // under half the bytes, so the dial does set the quantizer.
        let mut state = 0x9E37_79B9u32;
        let noise: Vec<u8> = (0..w * h)
            .flat_map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let [b, g, r, _] = state.to_le_bytes();
                [b, g, r, 0]
            })
            .collect();
        let mut sizes = Vec::new();
        for quality in [100, 1] {
            let mut stream = coder.open(w as u16, h as u16, quality).unwrap();
            stream.read_bgrx(&noise, w * 4, None).unwrap();
            unit.clear();
            assert!(stream.encode(true, &mut unit).unwrap());
            sizes.push(unit.len());
        }
        assert!(sizes[1] * 2 < sizes[0], "the dial at 1 codes noise in under half the bytes of 100: {sizes:?}");
    }

    /// The hardware's frames decode to the picture, near enough. Passes with a
    /// word where this machine has no encoder.
    #[test]
    fn hardware_frames_decode_to_the_picture_with_libavcodecs_software_decoder() {
        let Some(encoder) = Encoder::vaapi() else {
            eprintln!("no VA-API H.264 encoder on this machine; the hardware coder is not exercised");
            return;
        };
        frames_decode_to_the_picture(&encoder, (96, 70), 18);
        frames_decode_to_the_picture(&encoder, (95, 69), 18);
    }

    /// libx264's frames decode to the picture, on every machine: it is what a
    /// host without a GPU codes its regions with.
    #[test]
    fn libx264_frames_decode_to_the_picture_with_libavcodecs_software_decoder() {
        Encoder::X264.try_frame().expect("libx264 through this libavcodec");
        frames_decode_to_the_picture(&Encoder::X264, (96, 70), 0);
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
        let mut stream = coder.open(w as u16, h as u16, 90).unwrap();
        let pixels = picture(w, h);
        stream.read_bgrx(&pixels, w * 4, None).unwrap();
        let mut unit = Vec::new();
        assert!(stream.encode(false, &mut unit).unwrap());
        let sps = unit.windows(4).position(|s| s[..3] == [0, 0, 1] && s[3] & 0x1f == 7).expect("an SPS");
        let (profile, constraints) = (unit[sps + 4], unit[sps + 5]);
        assert_eq!(profile, 66, "Baseline");
        assert_ne!(constraints & 0x40, 0, "constraint_set1: Constrained Baseline");
        for _ in 0..5 {
            unit.clear();
            assert!(stream.encode(false, &mut unit).unwrap(), "every frame in is a frame out");
        }
    }
}
