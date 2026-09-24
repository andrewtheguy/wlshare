//! The VP9 encoding: the whole framebuffer as one VP9 stream, for a desktop
//! client that would rather have a picture that moves than one that is exact.
//!
//! A private encoding, [`crate::ENCODING_VP9`], which only a client that lists
//! it is ever sent. Its rectangle always covers the whole framebuffer, and its
//! body is a length word and one VP9 frame:
//!
//! ```text
//! u32 length   the frame's bytes
//! u8[length]   one VP9 frame (a superframe counts as one)
//! ```
//!
//! The rectangles of successive updates are one stream, each frame coded
//! against the frames before it, so none of them means anything alone. The
//! stream begins with a keyframe, and so does every picture a decoder has to be
//! able to start from: the first after the encoding is listed, the first at a
//! new framebuffer size, and the one that answers a non-incremental request.
//!
//! **Fixed.** Every frame is 8-bit **4:4:4** (profile 1) — a colour sample per
//! pixel, since the loss 4:2:0 costs a desktop is its text's colour and not its
//! edges — converted from the framebuffer as BT.601 at studio swing, which the
//! keyframe header says so a decoder does not guess. The quantizer is pinned by
//! a 1–100 quality dial ([`quality_to_q`]), which only the encoder's owner
//! moves ([`Vp9Encoder::set_quality`]): no bitrate, no adaptive quantization,
//! no dropped frames, so every frame is sent at exactly the dial's quality at
//! the time. The client's pixel format does not apply to this encoding; what it
//! decodes to is its own business, and [`Vp9Decoder`] writes `B, G, R, X`.
//!
//! libvpx does the coding, from the static archive `libvpx-prebuilt` publishes,
//! behind `encode` on the server's side and `decode` on the client's. Its C API
//! returns error codes rather than asserting, so every call is checked and a
//! failure ends one session rather than the process.

use thiserror::Error;

#[cfg(any(feature = "encode", feature = "decode"))]
use std::os::raw::c_int;

#[cfg(any(feature = "encode", feature = "decode"))]
use vpx_sys as vpx;

/// The coarsest end of the quality dial.
pub const QUALITY_MIN: u8 = 1;
/// The finest end of the quality dial.
pub const QUALITY_MAX: u8 = 100;

/// The finest quantizer the dial reaches. Below about 8 a VP9 desktop is
/// visually lossless and costs several times the bytes to be so, so a dial
/// that went further would have a top end that bought only bandwidth.
const Q_FINEST: u32 = 8;
/// VP9's coarsest quantizer.
const Q_COARSEST: u32 = 63;

/// libvpx's realtime speed, 0–9: inside the 5–8 band its own live-encoding
/// guidance names.
#[cfg(feature = "encode")]
const CPU_USED: c_int = 7;

/// The 1–100 quality dial as a VP9 quantizer, the dial's ends becoming
/// [`Q_COARSEST`] and [`Q_FINEST`]. Out-of-range input is clamped.
pub fn quality_to_q(quality: u8) -> u32 {
    let quality = u32::from(quality.clamp(QUALITY_MIN, QUALITY_MAX));
    Q_COARSEST - (quality - 1) * (Q_COARSEST - Q_FINEST) / 99
}

/// Why a picture could not be encoded or a frame decoded.
#[derive(Debug, Error)]
pub enum Vp9Error {
    /// libvpx refused a call, in its own words.
    #[error("vp9 {call}: {detail}")]
    Codec { call: &'static str, detail: String },
    #[error("a {0}x{1} picture has no pixels to encode")]
    Empty(u16, u16),
    #[error("a {0}x{1} picture at a stride of {2} does not fit in {3} bytes")]
    Picture(usize, usize, usize, usize),
    #[error("a frame of {0} bytes is over the {max}-byte ceiling", max = crate::client::MAX_RECT_BODY)]
    FrameTooLong(usize),
    #[error("the frame decoded to no picture")]
    NoPicture,
    #[error("the frame is {0}x{1} and its rectangle {2}x{3}")]
    Size(u32, u32, usize, usize),
    #[error("the frame is not 8-bit 4:4:4 (format {0}, {1} bits)")]
    Format(vpx::vpx_img_fmt_t, u32),
}

/// Turn a libvpx return code into a [`Vp9Error`] naming the call.
#[cfg(any(feature = "encode", feature = "decode"))]
fn check(err: vpx::vpx_codec_err_t, call: &'static str) -> Result<(), Vp9Error> {
    if err == vpx::vpx_codec_err_t_VPX_CODEC_OK {
        return Ok(());
    }
    // SAFETY: `vpx_codec_err_to_string` takes a code, any code, and returns a
    // pointer to a static string compiled into the archive.
    let detail = unsafe { std::ffi::CStr::from_ptr(vpx::vpx_codec_err_to_string(err)) };
    Err(Vp9Error::Codec { call, detail: detail.to_string_lossy().into_owned() })
}

/// How many threads the encoder gets: the machine less two cores, at most
/// eight. An encode is a burst of tens of milliseconds a frame that the person
/// at the other end is waiting on, and libvpx splits it across threads by
/// rows and tile columns; the two cores kept back are for the compositor and
/// the session, which are what make the next frame. Six threads on a
/// six-core host coded a scrolling 4K frame in three quarters of the time
/// three did, and a keyframe in a little over half.
#[cfg(feature = "encode")]
fn encoder_threads() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(2)).clamp(1, 8) as u32
}

/// How many threads the decoder gets: half the machine, at most four. A
/// decode gains nothing past the stream's tile columns, and the window still
/// needs somewhere to draw.
#[cfg(feature = "decode")]
fn decoder_threads() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() / 2).clamp(1, 4) as u32
}

/// Whether `len` bytes hold a `width`×`height` picture of 4-byte pixels whose
/// rows are `stride` apart.
#[cfg_attr(not(any(feature = "encode", feature = "decode")), allow(dead_code))]
fn fits(width: usize, height: usize, stride: usize, len: usize) -> bool {
    width == 0 || height == 0 || (stride >= width * 4 && len >= (height - 1) * stride + width * 4)
}

/// `B, G, R, X` pixels into BT.601 studio-swing 4:4:4 planes, `width` samples
/// a row. The `yuv` crate's conversion, which takes the AVX2 or NEON path the
/// machine has: a 4K frame is eight million pixels, and a scalar loop over
/// them was a fifth of the time an encode took.
#[cfg_attr(not(feature = "encode"), allow(dead_code))]
fn bgrx_to_i444(pixels: &[u8], stride: usize, width: usize, height: usize, planes: [&mut [u8]; 3]) {
    use yuv::{BufferStoreMut, YuvConversionMode, YuvPlanarImageMut, YuvRange, YuvStandardMatrix};
    let [y_plane, u_plane, v_plane] = planes;
    let (width, height) = (width as u32, height as u32);
    let mut image = YuvPlanarImageMut {
        y_plane: BufferStoreMut::Borrowed(y_plane),
        y_stride: width,
        u_plane: BufferStoreMut::Borrowed(u_plane),
        u_stride: width,
        v_plane: BufferStoreMut::Borrowed(v_plane),
        v_stride: width,
        width,
        height,
    };
    // The X byte is the alpha channel of a BGRA picture, which this ignores.
    yuv::bgra_to_yuv444(&mut image, pixels, stride as u32, YuvRange::Limited, YuvStandardMatrix::Bt601, YuvConversionMode::Balanced)
        .expect("the picture fits its buffers, which the caller checked");
}

/// BT.601 studio-swing 4:4:4 planes back into `B, G, R, X`, the X byte zero as
/// every other decoder here writes it. `strides` are the planes'. The `yuv`
/// crate's conversion, as [`bgrx_to_i444`] is: on a 4K frame the scalar loop
/// it replaces was a third of the client's whole decode step.
#[cfg_attr(not(feature = "decode"), allow(dead_code))]
fn i444_to_bgrx(planes: [&[u8]; 3], strides: [usize; 3], width: usize, height: usize, out: &mut [u8], stride: usize) {
    use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix};
    let [y_plane, u_plane, v_plane] = planes;
    let image = YuvPlanarImage {
        y_plane,
        y_stride: strides[0] as u32,
        u_plane,
        u_stride: strides[1] as u32,
        v_plane,
        v_stride: strides[2] as u32,
        width: width as u32,
        height: height as u32,
    };
    yuv::yuv444_to_bgra(&image, out, stride as u32, YuvRange::Limited, YuvStandardMatrix::Bt601)
        .expect("the picture fits its buffers, which the caller checked");
    // It writes an opaque alpha where the X byte goes.
    for row in 0..height {
        for pixel in out[row * stride..row * stride + width * 4].as_chunks_mut::<4>().0 {
            pixel[3] = 0;
        }
    }
}

/// One connection's VP9 stream, server side: a libvpx encoder at one picture
/// size. A framebuffer of another size needs another encoder, whose first
/// frame is a keyframe by construction.
#[cfg(feature = "encode")]
pub struct Vp9Encoder {
    /// Boxed so that its address never changes: libvpx is handed a pointer to
    /// it at init and every call after.
    ctx: Box<vpx::vpx_codec_ctx_t>,
    /// The configuration libvpx was initialised with, kept so the quantizer
    /// can be moved by [`Self::set_quality`] with everything else unchanged.
    cfg: vpx::vpx_codec_enc_cfg_t,
    /// The dial's position, clamped.
    quality: u8,
    /// An image that borrows `planes`: `vpx_img_wrap` allocated nothing, so it
    /// is never freed, and its plane pointers are set before every encode.
    img: vpx::vpx_image_t,
    width: u16,
    height: u16,
    /// Y, U and V, `width × height` each, reused from frame to frame.
    planes: [Vec<u8>; 3],
    /// Where timestamps are measured from, in the millisecond timebase below.
    started: std::time::Instant,
    /// The last timestamp given, which the next must pass.
    pts: i64,
}

#[cfg(feature = "encode")]
impl Vp9Encoder {
    /// An encoder for a `width`×`height` picture at `quality` (1–100).
    pub fn new(width: u16, height: u16, quality: u8) -> Result<Self, Vp9Error> {
        if width == 0 || height == 0 {
            return Err(Vp9Error::Empty(width, height));
        }
        let quality = quality.clamp(QUALITY_MIN, QUALITY_MAX);
        let q = quality_to_q(quality);
        let threads = encoder_threads();

        // SAFETY: takes nothing and returns a static interface descriptor.
        let iface = unsafe { vpx::vpx_codec_vp9_cx() };
        // SAFETY: zeroed, then written through by `config_default`, whose
        // failure is returned before anything reads it.
        let mut cfg: vpx::vpx_codec_enc_cfg_t = unsafe { std::mem::zeroed() };
        check(unsafe { vpx::vpx_codec_enc_config_default(iface, &mut cfg, 0) }, "config_default")?;

        cfg.g_w = u32::from(width);
        cfg.g_h = u32::from(height);
        // Profile 1 is 8-bit 4:4:4, which is the image format wrapped below.
        cfg.g_profile = 1;
        cfg.g_threads = threads;
        // Milliseconds, so a timestamp is wall-clock time: the desktop decides
        // when a frame happens, not a frame rate.
        cfg.g_timebase.num = 1;
        cfg.g_timebase.den = 1000;
        // libvpx's default holds 25 frames before emitting any; zero makes one
        // encode one frame out.
        cfg.g_lag_in_frames = 0;
        cfg.g_pass = vpx::vpx_enc_pass_VPX_RC_ONE_PASS;
        // TCP loses nothing, so resilience would be compression spent for no
        // reason, and so would a periodic keyframe: every keyframe is asked for.
        cfg.g_error_resilient = 0;
        cfg.kf_mode = vpx::vpx_kf_mode_VPX_KF_DISABLED;
        // Constant quality, the quantizer pinned top and bottom: the bytes land
        // wherever the picture puts them.
        cfg.rc_end_usage = vpx::vpx_rc_mode_VPX_Q;
        cfg.rc_min_quantizer = q;
        cfg.rc_max_quantizer = q;
        // A dropped frame would be pixels the damage log already counts as sent,
        // and a resized one a picture of the wrong size.
        cfg.rc_dropframe_thresh = 0;
        cfg.rc_resize_allowed = 0;

        // SAFETY: zeroed and written through by `enc_init_ver`; `cfg` is the
        // struct libvpx filled in, edited by field; the ABI version is the one
        // the linked archive's headers declare.
        let mut ctx: Box<vpx::vpx_codec_ctx_t> = Box::new(unsafe { std::mem::zeroed() });
        check(
            unsafe { vpx::vpx_codec_enc_init_ver(&mut *ctx, iface, &cfg, 0, vpx::VPX_ENCODER_ABI_VERSION as c_int) },
            "enc_init_ver",
        )?;

        // From here `Drop` releases the context, so errors go through `encoder`.
        let samples = usize::from(width) * usize::from(height);
        let mut encoder = Self {
            ctx,
            cfg,
            quality,
            // SAFETY: zeroed, then filled in by `vpx_img_wrap` below.
            img: unsafe { std::mem::zeroed() },
            width,
            height,
            planes: [vec![0; samples], vec![0; samples], vec![0; samples]],
            started: std::time::Instant::now(),
            pts: -1,
        };

        // SAFETY: the context is live and every one of these controls takes an
        // `int`, which a variadic call cannot check.
        unsafe {
            // What `VPX_Q` actually reads; the quantizer bounds alone leave it
            // at its default.
            encoder.control(vpx::vp8e_enc_control_id_VP8E_SET_CQ_LEVEL, q as c_int, "cq_level")?;
            encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_TUNE_CONTENT, vpx::vp9e_tune_content_VP9E_CONTENT_SCREEN as c_int, "tune_content")?;
            encoder.control(vpx::vp8e_enc_control_id_VP8E_SET_CPUUSED, CPU_USED, "cpuused")?;
            // Said in the bitstream, so a decoder converts back with the matrix
            // and range the conversion used rather than guessing BT.709.
            encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_COLOR_SPACE, vpx::vpx_color_space_VPX_CS_BT_601 as c_int, "color_space")?;
            encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_COLOR_RANGE, vpx::vpx_color_range_VPX_CR_STUDIO_RANGE as c_int, "color_range")?;
            // Adaptive quantization would move the quantizer off the dial.
            encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_AQ_MODE, 0, "aq_mode")?;
            if threads > 1 {
                // What makes the threads work on one picture: rows within a
                // tile, and tile columns for them to split. libvpx clamps the
                // columns to what the width allows.
                encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_ROW_MT, 1, "row_mt")?;
                encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_TILE_COLUMNS, threads.ilog2() as c_int, "tile_columns")?;
            }
            // A non-null pointer that is never read is libvpx's own way of
            // asking for the layout and no allocation.
            let wrapped = vpx::vpx_img_wrap(
                &mut encoder.img,
                vpx::vpx_img_fmt_VPX_IMG_FMT_I444,
                cfg.g_w,
                cfg.g_h,
                1,
                std::ptr::dangling_mut::<u8>(),
            );
            if wrapped.is_null() {
                return Err(Vp9Error::Codec { call: "img_wrap", detail: format!("refused a {width}x{height} picture") });
            }
        }
        Ok(encoder)
    }

    /// The picture size this encoder codes.
    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// The quality the next frame is coded at, clamped to the dial.
    pub fn quality(&self) -> u8 {
        self.quality
    }

    /// Move the dial on the running encoder, clamped to it. The next frame is
    /// coded at the new quantizer against the frames before it: rebuilding the
    /// encoder would cost a keyframe, the most bytes a frame can be, at the
    /// moment a slow link can least afford them.
    pub fn set_quality(&mut self, quality: u8) -> Result<(), Vp9Error> {
        let quality = quality.clamp(QUALITY_MIN, QUALITY_MAX);
        let q = quality_to_q(quality);
        self.quality = quality;
        if q == self.cfg.rc_min_quantizer {
            return Ok(());
        }
        self.cfg.rc_min_quantizer = q;
        self.cfg.rc_max_quantizer = q;
        // SAFETY: `cfg` is what libvpx accepted at init with the quantizer
        // bounds changed, and libvpx validates it again rather than trusting
        // it. Both halves are needed: the bounds in the configuration, and the
        // level `VPX_Q` actually reads, as at init.
        unsafe {
            check(vpx::vpx_codec_enc_config_set(&mut *self.ctx, &self.cfg), "enc_config_set")?;
            self.control(vpx::vp8e_enc_control_id_VP8E_SET_CQ_LEVEL, q as c_int, "cq_level")?;
        }
        Ok(())
    }

    /// Encode the picture — [`Self::size`] of `B, G, R, X` pixels whose rows
    /// are `stride` bytes apart — and append the rectangle's body to `out`: the
    /// length word and the frame. `keyframe` makes it one a decoder can start
    /// from; an encoder's first frame is one either way.
    pub fn encode_rect(&mut self, pixels: &[u8], stride: usize, keyframe: bool, out: &mut Vec<u8>) -> Result<(), Vp9Error> {
        let (width, height) = (usize::from(self.width), usize::from(self.height));
        if !fits(width, height, stride, pixels.len()) {
            return Err(Vp9Error::Picture(width, height, stride, pixels.len()));
        }
        let [y, u, v] = &mut self.planes;
        bgrx_to_i444(pixels, stride, width, height, [y.as_mut_slice(), u.as_mut_slice(), v.as_mut_slice()]);

        // Strictly increasing, which two encodes inside one millisecond would
        // otherwise not be.
        self.pts = (self.started.elapsed().as_millis() as i64).max(self.pts + 1);
        // `vpx_enc_frame_flags_t` is a C `long`, which is not 64 bits everywhere.
        let flags = if keyframe { vpx::VPX_EFLAG_FORCE_KF as vpx::vpx_enc_frame_flags_t } else { 0 };
        let length_at = out.len();
        out.extend_from_slice(&[0; 4]);

        // SAFETY: the planes belong to `self` and are not touched until the
        // next encode, and their strides are the picture's width; libvpx does
        // not write to an input image. The packets it hands back point into the
        // encoder and are copied out before it is called again.
        unsafe {
            for (i, plane) in self.planes.iter().enumerate() {
                self.img.planes[i] = plane.as_ptr().cast_mut();
                self.img.stride[i] = width as c_int;
            }
            // A duration of one tick: there is no rate control to read it.
            check(
                vpx::vpx_codec_encode(&mut *self.ctx, &self.img, self.pts, 1, flags, vpx::VPX_DL_REALTIME as std::os::raw::c_ulong),
                "encode",
            )?;
            let mut iter: vpx::vpx_codec_iter_t = std::ptr::null();
            loop {
                let packet = vpx::vpx_codec_get_cx_data(&mut *self.ctx, &mut iter);
                if packet.is_null() {
                    break;
                }
                if (*packet).kind != vpx::vpx_codec_cx_pkt_kind_VPX_CODEC_CX_FRAME_PKT {
                    continue;
                }
                let frame = &(*packet).data.frame;
                out.extend_from_slice(std::slice::from_raw_parts(frame.buf.cast::<u8>(), frame.sz));
            }
        }
        let len = out.len() - length_at - 4;
        if len > crate::client::MAX_RECT_BODY {
            out.truncate(length_at);
            return Err(Vp9Error::FrameTooLong(len));
        }
        out[length_at..length_at + 4].copy_from_slice(&(len as u32).to_be_bytes());
        Ok(())
    }

    /// One `vpx_codec_control_` call with an `int` argument, checked.
    ///
    /// # Safety
    ///
    /// `id` must be a control whose argument is an `int`: the call is variadic,
    /// and any other type compiles and corrupts the stack.
    unsafe fn control(&mut self, id: vpx::vp8e_enc_control_id, value: c_int, call: &'static str) -> Result<(), Vp9Error> {
        check(unsafe { vpx::vpx_codec_control_(&mut *self.ctx, id as c_int, value) }, call)
    }
}

// SAFETY: an encoder is owned by one session and every call takes `&mut self`;
// libvpx keeps no thread-local state per encoder, so moving one between threads
// is sound. Not `Sync`: two concurrent calls on one context are not.
#[cfg(feature = "encode")]
unsafe impl Send for Vp9Encoder {}

#[cfg(feature = "encode")]
impl Drop for Vp9Encoder {
    fn drop(&mut self) {
        // SAFETY: the only handle, destroyed once. The wrapped image allocated
        // nothing and is not freed.
        unsafe {
            vpx::vpx_codec_destroy(&mut *self.ctx);
        }
    }
}

/// One connection's VP9 stream, client side: every VP9 rectangle is decoded
/// by the same decoder, in the order they arrive, since each frame is coded
/// against the ones before it.
#[cfg(feature = "decode")]
pub struct Vp9Decoder {
    /// Boxed for the same reason as the encoder's.
    ctx: Box<vpx::vpx_codec_ctx_t>,
}

#[cfg(feature = "decode")]
impl Vp9Decoder {
    pub fn new() -> Result<Self, Vp9Error> {
        // SAFETY: a static interface, a zeroed context written through by
        // `dec_init_ver`, and the ABI version of the linked archive's headers.
        unsafe {
            let iface = vpx::vpx_codec_vp9_dx();
            let cfg = vpx::vpx_codec_dec_cfg_t { threads: decoder_threads(), w: 0, h: 0 };
            let mut ctx: Box<vpx::vpx_codec_ctx_t> = Box::new(std::mem::zeroed());
            check(vpx::vpx_codec_dec_init_ver(&mut *ctx, iface, &cfg, 0, vpx::VPX_DECODER_ABI_VERSION as c_int), "dec_init_ver")?;
            Ok(Self { ctx })
        }
    }

    /// Decode a VP9 rectangle payload — the frame, without its length word —
    /// of `width`×`height` pixels into `out`, whose first byte is the
    /// rectangle's first pixel and whose rows are `stride` bytes apart, as
    /// `B, G, R, X`.
    pub fn decode_rect(&mut self, payload: &[u8], width: usize, height: usize, out: &mut [u8], stride: usize) -> Result<(), Vp9Error> {
        if !fits(width, height, stride, out.len()) {
            return Err(Vp9Error::Picture(width, height, stride, out.len()));
        }
        // SAFETY: `payload` outlives the call. The image libvpx hands back
        // belongs to the decoder and is read before it is called again; its
        // planes are read only inside the size and strides it reports, after
        // the checks below.
        unsafe {
            check(
                vpx::vpx_codec_decode(&mut *self.ctx, payload.as_ptr(), payload.len() as std::os::raw::c_uint, std::ptr::null_mut(), 0),
                "decode",
            )?;
            let mut iter: vpx::vpx_codec_iter_t = std::ptr::null();
            let img = vpx::vpx_codec_get_frame(&mut *self.ctx, &mut iter);
            if img.is_null() {
                return Err(Vp9Error::NoPicture);
            }
            let img = &*img;
            if (img.d_w as usize, img.d_h as usize) != (width, height) {
                return Err(Vp9Error::Size(img.d_w, img.d_h, width, height));
            }
            if img.fmt != vpx::vpx_img_fmt_VPX_IMG_FMT_I444 || img.bit_depth != 8 {
                return Err(Vp9Error::Format(img.fmt, img.bit_depth));
            }
            if width == 0 || height == 0 {
                return Ok(());
            }
            let plane = |i: usize| {
                let stride = img.stride[i] as usize;
                std::slice::from_raw_parts(img.planes[i], (height - 1) * stride + width)
            };
            let strides = [img.stride[0] as usize, img.stride[1] as usize, img.stride[2] as usize];
            i444_to_bgrx([plane(0), plane(1), plane(2)], strides, width, height, out, stride);
        }
        Ok(())
    }
}

// SAFETY: as the encoder's — one owner, `&mut self` for every call.
#[cfg(feature = "decode")]
unsafe impl Send for Vp9Decoder {}

#[cfg(feature = "decode")]
impl Drop for Vp9Decoder {
    fn drop(&mut self) {
        // SAFETY: the only handle, destroyed once.
        unsafe {
            vpx::vpx_codec_destroy(&mut *self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quality_dial_spans_the_quantizer_finest_last() {
        assert_eq!(quality_to_q(QUALITY_MIN), Q_COARSEST);
        assert_eq!(quality_to_q(QUALITY_MAX), Q_FINEST);
        assert_eq!(quality_to_q(0), Q_COARSEST, "clamped, not wrapped");
        assert_eq!(quality_to_q(255), Q_FINEST);
        for quality in QUALITY_MIN..QUALITY_MAX {
            assert!(quality_to_q(quality) >= quality_to_q(quality + 1), "a higher dial is never coarser");
        }
    }

    /// The two conversions are each other's inverse to within the rounding a
    /// studio-swing round trip costs, on every channel's full range.
    #[test]
    fn the_colour_conversion_round_trips() {
        let colours: Vec<[u8; 4]> = (0..=255u8)
            .step_by(15)
            .flat_map(|r| (0..=255u8).step_by(51).flat_map(move |g| (0..=255u8).step_by(85).map(move |b| [b, g, r, 0])))
            .collect();
        let width = colours.len();
        let pixels: Vec<u8> = colours.iter().flatten().copied().collect();
        let mut planes = [vec![0; width], vec![0; width], vec![0; width]];
        let [y, u, v] = &mut planes;
        bgrx_to_i444(&pixels, width * 4, width, 1, [y.as_mut_slice(), u.as_mut_slice(), v.as_mut_slice()]);
        assert!(planes[0].iter().all(|&y| (16..=235).contains(&y)), "luma stays in studio range");

        let mut back = vec![0xAA; width * 4];
        i444_to_bgrx([&planes[0], &planes[1], &planes[2]], [width; 3], width, 1, &mut back, width * 4);
        for (i, (want, got)) in pixels.as_chunks::<4>().0.iter().zip(back.as_chunks::<4>().0).enumerate() {
            // Studio swing folds 256 levels into 219, so a channel can come
            // back three off; a wrong matrix would be off by tens.
            for channel in 0..3 {
                assert!(want[channel].abs_diff(got[channel]) <= 3, "pixel {i}: {want:?} came back {got:?}");
            }
            assert_eq!(got[3], 0, "X is written as zero");
        }
    }

    #[test]
    fn a_picture_that_does_not_fit_its_buffer_is_refused() {
        assert!(fits(4, 2, 16, 32));
        assert!(fits(4, 2, 20, 36), "the last row needs no padding after it");
        assert!(!fits(4, 2, 12, 32), "a stride shorter than a row");
        assert!(!fits(4, 2, 16, 31));
        assert!(fits(0, 0, 0, 0));
    }
}

/// The encoder read back by the decoder. Both are libvpx, so what these test is
/// the half each side owns — the conversion, the configuration, the framing —
/// and what the bitstream says about itself, not libvpx's own coding.
#[cfg(all(test, feature = "encode", feature = "decode"))]
mod round_trip {
    use super::*;

    /// A dark terminal with one-pixel coloured glyph stems at odd columns: the
    /// picture 4:2:0 cannot carry, since each stem shares its colour sample with
    /// three pixels of background.
    fn stems(width: usize, height: usize) -> (Vec<u8>, Vec<(usize, usize)>) {
        let mut pixels: Vec<u8> = [30u8, 30, 30, 0].repeat(width * height);
        let mut at = Vec::new();
        for y in (0..height).step_by(2) {
            for x in (1..width).step_by(4) {
                pixels[(y * width + x) * 4..][..4].copy_from_slice(&[198, 121, 255, 0]);
                at.push((x, y));
            }
        }
        (pixels, at)
    }

    /// Encode `pixels` and read back the length word, returning the frame.
    fn encode(encoder: &mut Vp9Encoder, pixels: &[u8], keyframe: bool) -> Vec<u8> {
        let (width, _) = encoder.size();
        let mut out = vec![0xEE];
        encoder.encode_rect(pixels, usize::from(width) * 4, keyframe, &mut out).expect("an encode");
        assert_eq!(out[0], 0xEE, "appended, not overwritten");
        let len = u32::from_be_bytes(out[1..5].try_into().unwrap()) as usize;
        assert_eq!(len, out.len() - 5, "the length word is the frame's");
        out.split_off(5)
    }

    fn is_keyframe(frame: &[u8]) -> bool {
        // SAFETY: `peek_stream_info` reads `frame` and writes the struct.
        unsafe {
            let mut info: vpx::vpx_codec_stream_info_t = std::mem::zeroed();
            info.sz = std::mem::size_of::<vpx::vpx_codec_stream_info_t>() as u32;
            check(vpx::vpx_codec_peek_stream_info(vpx::vpx_codec_vp9_dx(), frame.as_ptr(), frame.len() as u32, &mut info), "peek").unwrap();
            info.is_kf != 0
        }
    }

    #[test]
    fn a_444_stream_keeps_a_one_pixel_stem_its_colour() {
        let (width, height) = (64, 48);
        let (pixels, at) = stems(width, height);
        let mut encoder = Vp9Encoder::new(width as u16, height as u16, QUALITY_MAX).unwrap();
        let frame = encode(&mut encoder, &pixels, false);
        assert!(is_keyframe(&frame), "an encoder's first frame is a keyframe");

        let mut decoder = Vp9Decoder::new().unwrap();
        let stride = width * 4 + 8;
        let mut out = vec![0; stride * height];
        decoder.decode_rect(&frame, width, height, &mut out, stride).unwrap();
        let worst = at
            .iter()
            .map(|&(x, y)| {
                let got = &out[y * stride + x * 4..][..3];
                got.iter().zip([198u8, 121, 255]).map(|(a, b)| a.abs_diff(b)).max().unwrap()
            })
            .max()
            .unwrap();
        assert!(worst <= 24, "a stem pixel came back {worst} code values off");
    }

    #[test]
    fn an_odd_sized_picture_decodes_at_its_own_size() {
        let (width, height) = (33, 17);
        let pixels = [40u8, 180, 90, 0].repeat(width * height);
        // At the finest quantizer, so that what is measured is the size and not
        // the quantization of a flat colour.
        let mut encoder = Vp9Encoder::new(width as u16, height as u16, QUALITY_MAX).unwrap();
        let frame = encode(&mut encoder, &pixels, false);
        let mut out = vec![0; width * height * 4];
        Vp9Decoder::new().unwrap().decode_rect(&frame, width, height, &mut out, width * 4).unwrap();
        for pixel in out.as_chunks::<4>().0 {
            assert!(pixel[0].abs_diff(40) <= 3 && pixel[1].abs_diff(180) <= 3 && pixel[2].abs_diff(90) <= 3, "{pixel:?}");
        }
    }

    /// Only the frames that are asked to be keyframes are, and the ones between
    /// decode against them.
    #[test]
    fn keyframes_come_when_asked_and_the_frames_between_decode() {
        let (width, height) = (64, 32);
        let mut encoder = Vp9Encoder::new(width as u16, height as u16, 60).unwrap();
        let mut decoder = Vp9Decoder::new().unwrap();
        let mut out = vec![0; width * height * 4];
        for step in 0..5usize {
            let mut pixels = [80u8, 40, 20, 0].repeat(width * height);
            for x in step * 4..step * 4 + 8 {
                for y in 0..height {
                    pixels[(y * width + x) * 4..][..3].copy_from_slice(&[240, 240, 240]);
                }
            }
            let frame = encode(&mut encoder, &pixels, step == 3);
            assert_eq!(is_keyframe(&frame), step == 0 || step == 3, "frame {step}");
            decoder.decode_rect(&frame, width, height, &mut out, width * 4).unwrap();
            let lit = &out[(step * 4 + 4) * 4..][..3];
            assert!(lit.iter().all(|&c| c > 200), "frame {step} did not decode to its own picture: {lit:?}");
        }
    }

    /// Moving the dial takes effect on the next frame without a keyframe, and
    /// the finer quantizer costs more bytes for the same change.
    #[test]
    fn the_dial_moves_on_a_running_encoder_without_a_keyframe() {
        let (width, height) = (64, 48);
        let (base, _) = stems(width, height);
        // Two encoders with the same history: the same base at the same
        // quality, so their next frames are deltas against the same reference.
        let mut encoder = Vp9Encoder::new(width as u16, height as u16, QUALITY_MIN).unwrap();
        let mut coarse_encoder = Vp9Encoder::new(width as u16, height as u16, QUALITY_MIN).unwrap();
        let mut decoder = Vp9Decoder::new().unwrap();
        let mut out = vec![0; width * height * 4];
        decoder.decode_rect(&encode(&mut encoder, &base, false), width, height, &mut out, width * 4).unwrap();
        encode(&mut coarse_encoder, &base, false);

        // The same change from the same picture, once at each end of the dial.
        let mut changed = base.clone();
        for (i, pixel) in changed.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            if i.is_multiple_of(3) {
                *pixel = [200, 60, 90, 0];
            }
        }
        let coarse = encode(&mut coarse_encoder, &changed, false);
        encoder.set_quality(QUALITY_MAX).unwrap();
        assert_eq!(encoder.quality(), QUALITY_MAX);
        let fine = encode(&mut encoder, &changed, false);
        assert!(!is_keyframe(&fine), "moving the dial is not a keyframe");
        decoder.decode_rect(&fine, width, height, &mut out, width * 4).unwrap();
        assert!(fine.len() > coarse.len(), "{} bytes at the finest end against {} at the coarsest", fine.len(), coarse.len());

        encoder.set_quality(0).unwrap();
        assert_eq!(encoder.quality(), QUALITY_MIN, "clamped, not wrapped");
    }

    /// What the daemon's settle rests on: the same unchanged picture encoded
    /// again at a finer quantizer sharpens it, as an inter frame. Were libvpx
    /// to skip blocks whose source had not moved, a desktop sent coarse on a
    /// link that was behind would stay coarse until it next changed, and the
    /// settle would have to spend a keyframe instead.
    #[test]
    fn a_finer_quantizer_sharpens_an_unchanged_picture_without_a_keyframe() {
        let (width, height) = (320, 240);
        // Speckle, so a coarse quantizer has detail to lose.
        let mut picture = [240u8, 240, 240, 0].repeat(width * height);
        let mut seed = 12_345u32;
        for pixel in picture.as_chunks_mut::<4>().0 {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if (seed >> 16).is_multiple_of(5) {
                *pixel = [20, 20, 20, 0];
            }
        }
        let error = |decoded: &[u8]| {
            let sum: u64 = decoded
                .as_chunks::<4>()
                .0
                .iter()
                .zip(picture.as_chunks::<4>().0)
                .map(|(a, b)| (0..3).map(|c| u64::from(a[c].abs_diff(b[c]))).sum::<u64>())
                .sum();
            sum as f64 / (width * height * 3) as f64
        };

        let mut encoder = Vp9Encoder::new(width as u16, height as u16, QUALITY_MIN).unwrap();
        let mut decoder = Vp9Decoder::new().unwrap();
        let mut out = vec![0; width * height * 4];
        decoder.decode_rect(&encode(&mut encoder, &picture, false), width, height, &mut out, width * 4).unwrap();
        let coarse = error(&out);

        encoder.set_quality(QUALITY_MAX).unwrap();
        let settle = encode(&mut encoder, &picture, false);
        assert!(!is_keyframe(&settle), "the settle frame cost a keyframe");
        decoder.decode_rect(&settle, width, height, &mut out, width * 4).unwrap();
        let settled = error(&out);

        let mut fresh = Vp9Encoder::new(width as u16, height as u16, QUALITY_MAX).unwrap();
        let mut fine_out = vec![0; width * height * 4];
        Vp9Decoder::new().unwrap().decode_rect(&encode(&mut fresh, &picture, false), width, height, &mut fine_out, width * 4).unwrap();
        let fine = error(&fine_out);
        assert!(
            settled < coarse / 4.0 && settled < fine * 2.0,
            "one frame at the finest quality left the unchanged picture at error {settled:.2} (coarse {coarse:.2}, \
             a keyframe at the finest {fine:.2}): the encoder skipped blocks that did not move"
        );
    }

    #[test]
    fn the_bitstream_says_bt601_studio_swing_444() {
        let mut encoder = Vp9Encoder::new(16, 16, 60).unwrap();
        let frame = encode(&mut encoder, &[10u8, 20, 200, 0].repeat(256), false);
        // SAFETY: as in `Vp9Decoder::decode_rect`.
        unsafe {
            let mut decoder = Vp9Decoder::new().unwrap();
            check(vpx::vpx_codec_decode(&mut *decoder.ctx, frame.as_ptr(), frame.len() as u32, std::ptr::null_mut(), 0), "decode").unwrap();
            let mut iter: vpx::vpx_codec_iter_t = std::ptr::null();
            let img = &*vpx::vpx_codec_get_frame(&mut *decoder.ctx, &mut iter);
            assert_eq!(img.cs, vpx::vpx_color_space_VPX_CS_BT_601);
            assert_eq!(img.range, vpx::vpx_color_range_VPX_CR_STUDIO_RANGE);
            assert_eq!(img.fmt, vpx::vpx_img_fmt_VPX_IMG_FMT_I444);
        }
    }

    #[test]
    fn a_frame_of_another_size_or_no_frame_at_all_is_an_error() {
        let mut encoder = Vp9Encoder::new(32, 16, 60).unwrap();
        let frame = encode(&mut encoder, &[0u8; 32 * 16 * 4], false);
        let mut out = vec![0; 64 * 64 * 4];
        let mut decoder = Vp9Decoder::new().unwrap();
        assert!(matches!(decoder.decode_rect(&frame, 16, 16, &mut out, 64), Err(Vp9Error::Size(32, 16, 16, 16))));
        assert!(decoder.decode_rect(&[0xFF, 0x00, 0x12], 32, 16, &mut out, 128).is_err());
        assert!(matches!(decoder.decode_rect(&frame, 32, 16, &mut out[..10], 128), Err(Vp9Error::Picture(..))));
        assert!(matches!(Vp9Encoder::new(0, 16, 60), Err(Vp9Error::Empty(0, 16))));
        assert!(matches!(encoder.encode_rect(&[0; 12], 128, false, &mut Vec::new()), Err(Vp9Error::Picture(..))));
    }
}
