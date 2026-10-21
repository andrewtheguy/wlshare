//! H.264 for the tiles picture's moving regions, by the host's hardware
//! encoder: VA-API through the system's libavcodec, the one already linked
//! for the camera's decoder, as its `h264_vaapi` encoder.
//!
//! A region of the tiles picture is a stream over its rectangle alone
//! ([`wlshare_rfb::tiles`]), VP9 by libvpx unless the daemon hands the encoder
//! a [`RegionCoder`] and the client takes H.264. This is that coder, for a
//! host whose GPU has an H.264 encoder a VA-API driver reaches — Intel's iHD,
//! AMD's Mesa — so a region that moves costs the processor its conversion to
//! NV12 and nothing of the search, which is what the tiles picture is for: the
//! least of the encoder's time on a link with room.
//!
//! **What a stream is.** Constant quantizer, pinned to the dial the client's
//! list names as the VP9 regions' is, with no bitrate and nothing dropped: the
//! 1–100 dial maps onto QP 40 down to 16, finest last. High profile, CABAC,
//! no B-frames, one frame in and one out (`async_depth` 1), so a frame is on
//! the wire the moment it is coded. BT.601 at studio swing, said in the
//! stream's VUI, as the VP9 regions say it in their keyframe. Each frame is
//! one Annex B access unit, the parameter sets ahead of every IDR, and the
//! first frame of a stream is one; so is the first after the dial moves,
//! since a VA-API quantizer is the encoder's for its life and a new one is a
//! new encoder. No SEI, no AUD.
//!
//! **Which encoder.** The daemon looks once, at startup ([`Hardware::probe`]):
//! each render node in turn, the low-power encoder first — Intel's
//! fixed-function block, which leaves the execution units to the desktop —
//! and the shader one after it, each tried on a frame of a small picture,
//! since a driver lists more profiles than it will open. What works is what
//! every session uses; with nothing working the regions are VP9 and the log
//! says why. The low-power block codes CQP without the HuC firmware, which
//! only bitrate control needs.
//!
//! **Failure is the tiles encoder's to absorb.** A coder that refuses a
//! stream or fails one midway is given up for VP9 for the rest of the
//! session ([`wlshare_rfb::tiles::TilesEncoder`]); nothing here ends a
//! session. Each session opens the device itself ([`Hardware::coder`]), so a
//! VA display is one thread's and the sessions share nothing of it.
//!
//! Tested against libavcodec's own H.264 decoder ([`crate::decode`]), which
//! shares nothing with the GPU: a test that finds no encoder says so and
//! passes, since the machines that build this seldom have one.

use std::ffi::{CStr, CString, c_char};
use std::ptr;

use anyhow::Context as _;
use ffmpeg_sys_next as ff;
use log::{debug, info};
use wlshare_rfb::tiles::{RegionCodec, RegionCoder, RegionError, RegionStream};
use wlshare_rfb::vp9::Rect;

/// The hardware encoder the probe found, which every session's coder opens.
#[derive(Debug)]
pub struct Hardware {
    /// The render node, `/dev/dri/renderD128` or a sibling.
    device: CString,
    /// Whether the low-power encoder is the one that works.
    low_power: bool,
}

/// The picture the probe codes one frame of, small enough for any encoder.
const PROBE_SIZE: (u16, u16) = (64, 64);

/// Where render nodes are.
const DRI: &str = "/dev/dri";

impl Hardware {
    /// Look for a working VA-API H.264 encoder: each render node in turn, the
    /// low-power encoder ahead of the shader one, each opened on
    /// [`PROBE_SIZE`] and asked for a frame. `None` with nothing working, the
    /// reasons logged.
    pub fn probe() -> Option<Self> {
        let mut nodes: Vec<std::path::PathBuf> = match std::fs::read_dir(DRI) {
            Ok(entries) => entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD"))).collect(),
            Err(e) => {
                info!("no hardware H.264 encoder: {DRI} cannot be read ({e}); the tiles' regions are VP9");
                return None;
            }
        };
        nodes.sort();
        if nodes.is_empty() {
            info!("no hardware H.264 encoder: no render node under {DRI}; the tiles' regions are VP9");
            return None;
        }
        for node in nodes {
            let device = CString::new(node.to_string_lossy().into_owned()).expect("a path has no NUL");
            for low_power in [true, false] {
                let hardware = Self { device: device.clone(), low_power };
                match hardware.try_frame() {
                    Ok(()) => {
                        info!("hardware H.264 for the tiles' regions: {} through VA-API, its {} encoder", node.display(), hardware.kind());
                        return Some(hardware);
                    }
                    Err(e) => debug!("the {} encoder on {}: {e:#}", hardware.kind(), node.display()),
                }
            }
            info!("{}: VA-API opens no H.264 encoder there (run with RUST_LOG=debug for each attempt)", node.display());
        }
        info!("no hardware H.264 encoder; the tiles' regions are VP9");
        None
    }

    /// Which encoder: the fixed-function block or the shader one.
    fn kind(&self) -> &'static str {
        if self.low_power { "low-power" } else { "shader" }
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

    /// A coder for one session: its own VA display on the device.
    pub fn coder(&self) -> anyhow::Result<Coder> {
        let mut device = ptr::null_mut();
        // SAFETY: the path is a NUL-terminated string and the reference is
        // checked before use; what libavutil allocates is freed by `Drop`.
        unsafe {
            check(ff::av_hwdevice_ctx_create(&mut device, ff::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI, self.device.as_ptr(), ptr::null_mut(), 0), "opening the VA-API device")
                .with_context(|| format!("VA-API on {}", self.device.to_string_lossy()))?;
        }
        anyhow::ensure!(!device.is_null(), "libavutil opened no device");
        Ok(Coder { device, low_power: self.low_power })
    }
}

/// One session's H.264 coder: a VA display, which each of its streams takes
/// an encoder on.
pub struct Coder {
    device: *mut ff::AVBufferRef,
    low_power: bool,
}

// SAFETY: the device reference is this value's alone, and it and every stream
// opened on it are used from one thread at a time, the session's.
unsafe impl Send for Coder {}

impl Coder {
    /// A stream over a `width`×`height` region at `quality`.
    fn stream(&mut self, width: u16, height: u16, quality: u8) -> anyhow::Result<Stream> {
        anyhow::ensure!(width > 0 && height > 0, "a {width}x{height} region has no pixels");
        let (w, h) = (usize::from(width), usize::from(height));
        // Strides a multiple of 64, and a row to spare, so a conversion into a
        // rectangle at the bottom edge has a whole row's stride to read.
        let stride = w.next_multiple_of(64);
        let mut stream = Stream {
            device: ptr::null_mut(),
            frames: ptr::null_mut(),
            context: ptr::null_mut(),
            hw_frame: ptr::null_mut(),
            sw_frame: ptr::null_mut(),
            packet: ptr::null_mut(),
            size: (width, height),
            stride,
            y: vec![16; stride * (h + 1)],
            uv: vec![128; stride * (h.div_ceil(2) + 1)],
            filled: false,
            quality,
            coded_at: quality,
            low_power: self.low_power,
            pts: 0,
        };
        // SAFETY: each allocation is checked, and a failure part way through
        // frees what came before it through `Drop`.
        unsafe {
            stream.device = ff::av_buffer_ref(self.device);
            stream.hw_frame = ff::av_frame_alloc();
            stream.sw_frame = ff::av_frame_alloc();
            stream.packet = ff::av_packet_alloc();
            anyhow::ensure!(!stream.device.is_null() && !stream.hw_frame.is_null() && !stream.sw_frame.is_null() && !stream.packet.is_null(), "libavcodec could not allocate a stream");
            let sw = &mut *stream.sw_frame;
            sw.format = ff::AVPixelFormat::AV_PIX_FMT_NV12 as i32;
            sw.width = i32::from(width);
            sw.height = i32::from(height);
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
    fn codec(&self) -> RegionCodec {
        RegionCodec::H264
    }

    fn open(&mut self, width: u16, height: u16, quality: u8) -> Result<Box<dyn RegionStream>, RegionError> {
        Ok(Box::new(self.stream(width, height, quality).map_err(|e| format!("{e:#}"))?))
    }
}

/// One region's stream: an encoder on the VA display, the NV12 picture in
/// front of it, and the frame and packet it reuses.
pub struct Stream {
    device: *mut ff::AVBufferRef,
    frames: *mut ff::AVBufferRef,
    context: *mut ff::AVCodecContext,
    hw_frame: *mut ff::AVFrame,
    /// Points into `y` and `uv`, which it does not own.
    sw_frame: *mut ff::AVFrame,
    packet: *mut ff::AVPacket,
    size: (u16, u16),
    stride: usize,
    y: Vec<u8>,
    uv: Vec<u8>,
    /// The planes hold a picture: one was read whole.
    filled: bool,
    /// The dial as last set.
    quality: u8,
    /// The dial the open encoder was made for.
    coded_at: u8,
    low_power: bool,
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
    /// Open the encoder at the current dial: the frames context on the device
    /// at the region's size and the codec context over it.
    unsafe fn open(&mut self) -> anyhow::Result<()> {
        // SAFETY: the caller holds the stream's pointers; each allocation is
        // checked, and `close` frees what this made.
        unsafe {
            self.close();
            self.frames = ff::av_hwframe_ctx_alloc(self.device);
            anyhow::ensure!(!self.frames.is_null(), "libavutil could not allocate a frames context");
            let frames = &mut *((*self.frames).data as *mut ff::AVHWFramesContext);
            frames.format = ff::AVPixelFormat::AV_PIX_FMT_VAAPI;
            frames.sw_format = ff::AVPixelFormat::AV_PIX_FMT_NV12;
            frames.width = i32::from(self.size.0);
            frames.height = i32::from(self.size.1);
            frames.initial_pool_size = 4;
            check(ff::av_hwframe_ctx_init(self.frames), "making VA-API surfaces")?;

            let name = c"h264_vaapi";
            let codec = ff::avcodec_find_encoder_by_name(name.as_ptr());
            anyhow::ensure!(!codec.is_null(), "this libavcodec has no h264_vaapi encoder");
            self.context = ff::avcodec_alloc_context3(codec);
            anyhow::ensure!(!self.context.is_null(), "libavcodec could not allocate an encoder");
            let ctx = &mut *self.context;
            ctx.width = i32::from(self.size.0);
            ctx.height = i32::from(self.size.1);
            ctx.pix_fmt = ff::AVPixelFormat::AV_PIX_FMT_VAAPI;
            ctx.time_base = ff::AVRational { num: 1, den: FPS };
            ctx.framerate = ff::AVRational { num: FPS, den: 1 };
            // Keyframes only where asked: the stream's first, and one after the dial moves.
            ctx.gop_size = i32::MAX;
            ctx.max_b_frames = 0;
            ctx.colorspace = ff::AVColorSpace::AVCOL_SPC_SMPTE170M;
            ctx.color_primaries = ff::AVColorPrimaries::AVCOL_PRI_SMPTE170M;
            ctx.color_trc = ff::AVColorTransferCharacteristic::AVCOL_TRC_SMPTE170M;
            ctx.color_range = ff::AVColorRange::AVCOL_RANGE_MPEG;
            ctx.hw_frames_ctx = ff::av_buffer_ref(self.frames);
            anyhow::ensure!(!ctx.hw_frames_ctx.is_null(), "libavutil could not share the frames context");
            let mut options = ptr::null_mut();
            let qp = qp(self.quality).to_string();
            for (key, value) in [
                ("rc_mode", "CQP"),
                ("qp", qp.as_str()),
                ("low_power", if self.low_power { "1" } else { "0" }),
                ("profile", "high"),
                ("coder", "cabac"),
                ("async_depth", "1"),
                ("idr_interval", "0"),
                ("sei", "0"),
                ("aud", "0"),
            ] {
                let (key, value) = (CString::new(key).unwrap(), CString::new(value).unwrap());
                ff::av_dict_set(&mut options, key.as_ptr(), value.as_ptr(), 0);
            }
            let opened = ff::avcodec_open2(self.context, codec, &mut options);
            ff::av_dict_free(&mut options);
            check(opened, "opening the h264_vaapi encoder")?;
            self.coded_at = self.quality;
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
    /// picture.
    fn convert(&mut self, pixels: &[u8], stride: usize, rect: Rect) -> anyhow::Result<()> {
        use yuv::{BufferStoreMut, YuvBiPlanarImageMut, YuvConversionMode, YuvRange, YuvStandardMatrix};
        let (w, h) = (usize::from(self.size.0), usize::from(self.size.1));
        let (left, top) = (usize::from(rect.x) & !1, usize::from(rect.y) & !1);
        let right = (usize::from(rect.x) + usize::from(rect.width)).next_multiple_of(2).min(w);
        let bottom = (usize::from(rect.y) + usize::from(rect.height)).next_multiple_of(2).min(h);
        if left >= right || top >= bottom {
            return Ok(());
        }
        let (rw, rh) = (right - left, bottom - top);
        anyhow::ensure!(stride >= w * 4 && pixels.len() >= (h - 1) * stride + w * 4, "a {w}x{h} picture at a stride of {stride} does not fit in {} bytes", pixels.len());
        let plane_stride = self.stride;
        let mut image = YuvBiPlanarImageMut {
            y_plane: BufferStoreMut::Borrowed(&mut self.y[top * plane_stride + left..]),
            y_stride: plane_stride as u32,
            uv_plane: BufferStoreMut::Borrowed(&mut self.uv[(top / 2) * plane_stride + left..]),
            uv_stride: plane_stride as u32,
            width: rw as u32,
            height: rh as u32,
        };
        yuv::bgra_to_yuv_nv12(&mut image, &pixels[top * stride + left * 4..], stride as u32, YuvRange::Limited, YuvStandardMatrix::Bt601, YuvConversionMode::Professional)
            .map_err(|e| anyhow::anyhow!("converting a {rw}x{rh} rectangle to NV12: {e}"))
    }

    fn frame(&mut self, keyframe: bool, out: &mut Vec<u8>) -> anyhow::Result<bool> {
        anyhow::ensure!(self.filled, "a stream was asked for a frame before any picture was read into it");
        // SAFETY: the pointers are this stream's; the surface is taken from the
        // pool, filled from the planes the software frame points into, coded
        // and given back, and the packet's bytes are read within its size.
        unsafe {
            if self.coded_at != self.quality {
                self.open().context("reopening the encoder at the new dial")?;
            }
            check(ff::av_hwframe_get_buffer(self.frames, self.hw_frame, 0), "taking a VA-API surface")?;
            let uploaded = ff::av_hwframe_transfer_data(self.hw_frame, self.sw_frame, 0);
            if uploaded < 0 {
                ff::av_frame_unref(self.hw_frame);
                check(uploaded, "uploading the picture to the surface")?;
            }
            let frame = &mut *self.hw_frame;
            frame.pts = self.pts;
            frame.pict_type = if keyframe { ff::AVPictureType::AV_PICTURE_TYPE_I } else { ff::AVPictureType::AV_PICTURE_TYPE_NONE };
            // The first picture of an encoder is an IDR whatever is asked.
            self.pts += 1;
            let sent = ff::avcodec_send_frame(self.context, self.hw_frame);
            ff::av_frame_unref(self.hw_frame);
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
    fn set_quality(&mut self, quality: u8) {
        self.quality = quality;
    }

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
    /// with `pixels`'s grey, within `tolerance` levels after studio swing.
    fn decodes_to(decoder: &mut crate::decode::Decoder, unit: &[u8], pixels: &[u8], w: usize, h: usize, tolerance: u8) {
        let mut out = Vec::new();
        let decoded = decoder.decode(unit, &mut out).unwrap().expect("a picture for every unit");
        assert_eq!((decoded.width, decoded.height), (w, h));
        for y in 0..h {
            for x in 0..w {
                let grey = pixels[(y * w + x) * 4];
                let expected = 16.0 + f64::from(grey) * 219.0 / 255.0;
                let got = f64::from(out[y * w + x]);
                assert!((got - expected).abs() <= f64::from(tolerance), "luma at {x},{y}: {got}, not {expected:.1}");
            }
        }
    }

    /// Frames coded by the hardware decode with libavcodec's software decoder
    /// to the picture they were coded from: the first an IDR with its
    /// parameter sets, the next a P frame of the damage alone, and the first
    /// after the dial moves an IDR again. Passes with a word where this
    /// machine has no encoder.
    #[test]
    fn frames_decode_to_the_picture_with_libavcodecs_software_decoder() {
        let Some(hardware) = Hardware::probe() else {
            eprintln!("no VA-API H.264 encoder on this machine; the hardware coder is not exercised");
            return;
        };
        let (w, h) = (96usize, 70usize);
        let mut coder = hardware.coder().unwrap();
        let mut stream = coder.open(w as u16, h as u16, 100).unwrap();
        let mut pixels = picture(w, h);
        let mut decoder = crate::decode::Decoder::new().unwrap();

        stream.read_bgrx(&pixels, w * 4, Some(&[Rect { x: 1, y: 1, width: 1, height: 1 }])).unwrap();
        let mut unit = Vec::new();
        assert!(stream.encode(false, &mut unit).unwrap(), "a frame of the first picture");
        let types = nal_types(&unit);
        assert!(types.contains(&7) && types.contains(&8) && types.contains(&5), "the first frame carries SPS, PPS and an IDR: {types:?}");
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6);

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
        decodes_to(&mut decoder, &unit, &pixels, w, h, 6);

        // The dial moves: the encoder is made again and the next frame is an IDR.
        stream.set_quality(60);
        stream.read_bgrx(&pixels, w * 4, Some(&[])).unwrap();
        unit.clear();
        assert!(stream.encode(false, &mut unit).unwrap());
        assert!(nal_types(&unit).contains(&5), "an IDR after the dial moved");
        decodes_to(&mut decoder, &unit, &pixels, w, h, 12);

        // A keyframe asked for is one.
        unit.clear();
        assert!(stream.encode(true, &mut unit).unwrap());
        assert!(nal_types(&unit).contains(&5), "an IDR where one was asked");
    }
}
