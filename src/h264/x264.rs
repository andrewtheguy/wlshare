//! libx264 through its own API: an encoder at `superfast` with `zerolatency`,
//! its motion searched by uneven multi-hexagon over 128 pixels, each picture
//! handed the quantizer it is to be coded at.
//!
//! The quantizer is forced on each picture by `i_qpplus1`, under the constant
//! quantizer rate control, whose own constant is never used. A forced
//! quantizer is taken within the control's bounds, which are its I and B
//! pictures' quantizers, about three either side of the constant; the ratios
//! between them are set wide ([`RATIO`]), so the bounds are every quantizer
//! there is. Nothing moves a macroblock's from its picture's: adaptive
//! quantization off, which `superfast` would leave on, and no macroblock tree
//! or lookahead, which `zerolatency` takes away. Under it, with weighted
//! prediction off, which a desktop has no fades for, x264 makes no half-size
//! picture and estimates no rate on one: with the wide search below, which
//! finds a window dragged far in a frame without that picture's motion to
//! start from, about a fifth of the processor time off a frame at 4:4:4 and
//! three tenths at 4:2:0, in the bytes it had, a dragged window's a quarter
//! more of very few. libavcodec's wrapper would not do: it ignores a
//! quantizer changed between frames, and copies every picture besides.
//!
//! **What did not change is taken on trust.** x264 is handed the whole
//! picture and, left to itself, works out macroblock by macroblock that most
//! of a desktop is as the frame before had it: a transform and a quantizer
//! over each one's difference, for every plane, to find nothing. The planes
//! know how many frames have been coded since a read last wrote into each
//! macroblock (`Planes::still`), and one that [`REFINED`] frames have coded
//! as it stands is flagged constant (`X264_MBINFO_CONSTANT`): x264 skips it
//! without looking, as long as the frame before coded it no coarser than
//! this one is asked, so a frame at a finer quantizer than the last still
//! refines what stands still. The frames before the flag are the ones that
//! bring a macroblock from its first coding to what its quantizer can hold;
//! flagged from the first, it would stay as that left it. The whole-picture
//! work stays, the copy in and the half-pel planes.
//!
//! **The preset and the search** ([measured](../../docs/video-performance.md)).
//! A page scrolled moves farther in a frame than `ultrafast`'s or
//! `superfast`'s diamond search over 16 pixels reaches; the multi-hexagon over
//! 128 finds it, and a scroll is sent in a sixth to a half of the bytes.
//! `superfast` over `ultrafast` is CABAC, the deblocking filter, the intra
//! partitions and the 8×8 transform, which halve a video's bytes; its stream
//! is High profile. Together, at the same quality, about two fifths of
//! `ultrafast`'s bytes for about 1.7 times its processor time.

use std::ptr;

use x264_sys as x;

use super::{Backend, FPS, Planes, encoder_threads};
use crate::rfb::Chroma;

/// libx264's preset.
pub const PRESET: &str = "superfast";

/// How many frames code a macroblock as it stands before it is flagged
/// constant: the one that carries its change and one that refines it.
const REFINED: u8 = 2;

/// The constant quantizer no picture is coded at, each one's being forced:
/// the middle of what the dial asks for.
const CONSTANT: i32 = 28;

/// The ratio between the constant quantizer's I and P pictures, and its P
/// and B: wide enough that its bounds, which a forced quantizer is held
/// within, are every quantizer.
const RATIO: f32 = 512.0;

/// How far, in pixels, the motion search reaches from its prediction.
const MOTION_RANGE: i32 = 128;

/// One libx264 encoder over a stream's coded picture.
pub struct Stream {
    encoder: *mut x::x264_t,
    chroma: Chroma,
    pts: i64,
    /// A flag for each macroblock of the picture, row by row, written for
    /// each P frame: constant where nothing changed for [`REFINED`] frames. x264 keeps the pointer in
    /// its frame, so the buffer is this stream's for as long as its encoder.
    constant: Vec<u8>,
}

// SAFETY: the encoder is this value's alone, used from one thread at a time,
// the session's; its own threads are its business.
unsafe impl Send for Stream {}

impl Stream {
    /// An encoder over a `width`×`height` picture, both even, at `chroma`:
    /// High profile at 4:2:0, and High 4:4:4 Predictive at 4:4:4, which is
    /// x264's for that picture with nothing asked.
    pub fn open((width, height): (usize, usize), chroma: Chroma) -> anyhow::Result<Self> {
        // SAFETY: the parameters are filled by x264's own default before any
        // field is set, and the encoder it opens is checked and owned by the
        // value returned.
        unsafe {
            let mut param: x::x264_param_t = std::mem::zeroed();
            anyhow::ensure!(
                x::x264_param_default_preset(
                    &mut param,
                    c"superfast".as_ptr(),
                    c"zerolatency".as_ptr()
                ) == 0,
                "libx264 has no preset {PRESET}"
            );
            // `zerolatency` takes the lookahead and the frame threads away,
            // which would each hold frames back, and slices the picture
            // among the threads instead.
            param.i_threads = encoder_threads() as i32;
            param.i_width = width as i32;
            param.i_height = height as i32;
            param.i_csp = csp(chroma);
            // x264 codes 4:4:4's chroma six steps coarser than its luma,
            // twice the samples at half the quality; taken back, so the
            // colour of a glyph's edge is at the quantizer the dial asks, as
            // its luma is.
            if chroma == Chroma::Yuv444 {
                param.analyse.i_chroma_qp_offset = -6;
            }
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
            param.analyse.i_me_method = x::X264_ME_UMH as i32;
            param.analyse.i_me_range = MOTION_RANGE;
            // Every picture's quantizer is forced, whatever the constant,
            // which x264 will not open without.
            param.rc.i_rc_method = x::X264_RC_CQP as i32;
            param.rc.i_qp_constant = CONSTANT;
            param.rc.f_ip_factor = RATIO;
            param.rc.f_pb_factor = RATIO;
            // No fades on a desktop, and with none looked for no half-size
            // picture is made.
            param.analyse.i_weighted_pred = x::X264_WEIGHTP_NONE as i32;
            // Every macroblock at its picture's forced quantizer.
            param.rc.i_aq_mode = x::X264_AQ_NONE as i32;
            // Each P frame says which macroblocks did not change.
            param.analyse.b_mb_info = 1;
            param.i_log_level = x::X264_LOG_WARNING as i32;
            let encoder = x::x264_encoder_open(&mut param);
            anyhow::ensure!(
                !encoder.is_null(),
                "libx264 opened no encoder over a {width}x{height} picture"
            );
            Ok(Self {
                encoder,
                chroma,
                pts: 0,
                constant: vec![0; width.div_ceil(16) * height.div_ceil(16)],
            })
        }
    }
}

/// x264's name for the planes a stream at `chroma` is handed.
fn csp(chroma: Chroma) -> i32 {
    (match chroma {
        Chroma::Yuv420 => x::X264_CSP_NV12,
        Chroma::Yuv444 => x::X264_CSP_I444,
    }) as i32
}

impl Backend for Stream {
    fn frame(
        &mut self,
        planes: &Planes,
        keyframe: bool,
        qp: u8,
        out: &mut Vec<u8>,
    ) -> anyhow::Result<bool> {
        // SAFETY: the picture points into the planes, which outlive the call
        // and which x264 reads and does not keep; the NAL units it returns are
        // its own, laid end to end, and read within the size it says.
        unsafe {
            let mut picture: x::x264_picture_t = std::mem::zeroed();
            x::x264_picture_init(&mut picture);
            picture.img.i_csp = csp(self.chroma);
            let sources: &[&Vec<u8>] = match self.chroma {
                Chroma::Yuv420 => &[&planes.y, &planes.uv],
                Chroma::Yuv444 => &[&planes.y, &planes.u, &planes.v],
            };
            picture.img.i_plane = sources.len() as i32;
            for (plane, source) in sources.iter().enumerate() {
                picture.img.plane[plane] = source.as_ptr().cast_mut();
                picture.img.i_stride[plane] = planes.stride as i32;
            }
            // The first picture of an encoder is an IDR whatever is asked.
            picture.i_type = if keyframe {
                x::X264_TYPE_IDR
            } else {
                x::X264_TYPE_AUTO
            } as i32;
            // A keyframe predicts from nothing, so nothing of it is constant,
            // and a frame that refines is to look at all of it.
            if !keyframe && !planes.refine {
                anyhow::ensure!(
                    self.constant.len() == planes.still.len(),
                    "a picture of {} macroblocks for an encoder of {}",
                    planes.still.len(),
                    self.constant.len()
                );
                for (flag, frames) in self.constant.iter_mut().zip(&planes.still) {
                    *flag = if *frames >= REFINED {
                        x::X264_MBINFO_CONSTANT as u8
                    } else {
                        0
                    };
                }
                picture.prop.mb_info = self.constant.as_mut_ptr();
            }
            picture.i_qpplus1 = i32::from(qp) + 1;
            picture.i_pts = self.pts;
            self.pts += 1;
            let mut coded: x::x264_picture_t = std::mem::zeroed();
            let (mut nals, mut count) = (ptr::null_mut(), 0);
            let size = x::x264_encoder_encode(
                self.encoder,
                &mut nals,
                &mut count,
                &mut picture,
                &mut coded,
            );
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
