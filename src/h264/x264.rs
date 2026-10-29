//! libx264 through its own API: an encoder at `ultrafast` with `zerolatency`,
//! each picture handed the quantizer it is to be coded at.
//!
//! The quantizer is forced on each picture by `i_qpplus1`, under the CRF rate
//! control. CRF's own choice is never made, every picture's quantizer being
//! forced, and nothing in `ultrafast` moves a macroblock's from its picture's:
//! no adaptive quantization, no macroblock tree, no lookahead. What CRF does
//! bring is the half-size picture x264 estimates its rate on, and the motion
//! it searches for there, which seeds the full search: `ultrafast`'s own
//! search does not follow a window dragged far in a frame, and with the
//! half-size one a drag is coded in a fifth of the bytes, for more of the
//! processor than a constant quantizer takes
//! ([measured](../../docs/video-performance.md)). A constant quantizer takes a
//! forced one too, but within its bounds, the constant's own I and B
//! pictures' quantizers about three either side of it, unless the ratios
//! between them are set wide. libavcodec's wrapper would not do: it ignores a
//! quantizer changed between frames, and copies every picture besides.

use std::ptr;

use x264_sys as x;

use super::{Backend, FPS, Planes, encoder_threads};

/// libx264's preset: its least search, the least of the processor's time.
pub const PRESET: &str = "ultrafast";

/// One libx264 encoder over a stream's coded picture.
pub struct Stream {
    encoder: *mut x::x264_t,
    pts: i64,
}

// SAFETY: the encoder is this value's alone, used from one thread at a time,
// the session's; its own threads are its business.
unsafe impl Send for Stream {}

impl Stream {
    /// An encoder over a `width`×`height` picture, both even.
    pub fn open((width, height): (usize, usize)) -> anyhow::Result<Self> {
        // SAFETY: the parameters are filled by x264's own default before any
        // field is set, and the encoder it opens is checked and owned by the
        // value returned.
        unsafe {
            let mut param: x::x264_param_t = std::mem::zeroed();
            anyhow::ensure!(x::x264_param_default_preset(&mut param, c"ultrafast".as_ptr(), c"zerolatency".as_ptr()) == 0, "libx264 has no preset {PRESET}");
            // `zerolatency` takes the lookahead and the frame threads away,
            // which would each hold frames back, and slices the picture
            // among the threads instead.
            param.i_threads = encoder_threads() as i32;
            param.i_width = width as i32;
            param.i_height = height as i32;
            param.i_csp = x::X264_CSP_NV12 as i32;
            param.i_fps_num = FPS;
            param.i_fps_den = 1;
            param.i_timebase_num = 1;
            param.i_timebase_den = FPS;
            // Keyframes only where asked, the stream's first among them.
            param.i_keyint_max = x::X264_KEYINT_MAX_INFINITE as i32;
            param.i_scenecut_threshold = 0;
            param.i_bframe = 0;
            param.b_repeat_headers = 1;
            param.b_annexb = 1;
            param.b_aud = 0;
            param.vui.i_colorprim = 6;
            param.vui.i_transfer = 6;
            param.vui.i_colmatrix = 6;
            param.vui.b_fullrange = 0;
            param.rc.i_rc_method = x::X264_RC_CRF as i32;
            param.i_log_level = x::X264_LOG_WARNING as i32;
            let encoder = x::x264_encoder_open(&mut param);
            anyhow::ensure!(!encoder.is_null(), "libx264 opened no encoder over a {width}x{height} picture");
            Ok(Self { encoder, pts: 0 })
        }
    }
}

impl Backend for Stream {
    fn frame(&mut self, planes: &Planes, keyframe: bool, qp: u8, out: &mut Vec<u8>) -> anyhow::Result<bool> {
        // SAFETY: the picture points into the planes, which outlive the call
        // and which x264 reads and does not keep; the NAL units it returns are
        // its own, laid end to end, and read within the size it says.
        unsafe {
            let mut picture: x::x264_picture_t = std::mem::zeroed();
            x::x264_picture_init(&mut picture);
            picture.img.i_csp = x::X264_CSP_NV12 as i32;
            picture.img.i_plane = 2;
            picture.img.plane[0] = planes.y.as_ptr().cast_mut();
            picture.img.plane[1] = planes.uv.as_ptr().cast_mut();
            picture.img.i_stride[0] = planes.stride as i32;
            picture.img.i_stride[1] = planes.stride as i32;
            // The first picture of an encoder is an IDR whatever is asked.
            picture.i_type = if keyframe { x::X264_TYPE_IDR } else { x::X264_TYPE_AUTO } as i32;
            picture.i_qpplus1 = i32::from(qp) + 1;
            picture.i_pts = self.pts;
            self.pts += 1;
            let mut coded: x::x264_picture_t = std::mem::zeroed();
            let (mut nals, mut count) = (ptr::null_mut(), 0);
            let size = x::x264_encoder_encode(self.encoder, &mut nals, &mut count, &mut picture, &mut coded);
            anyhow::ensure!(size >= 0, "libx264 failed to code a frame ({size})");
            if size == 0 {
                return Ok(false);
            }
            out.extend_from_slice(std::slice::from_raw_parts((*nals).p_payload, size as usize));
            Ok(true)
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: the encoder is this value's, opened and not yet closed.
        unsafe { x::x264_encoder_close(self.encoder) }
    }
}
