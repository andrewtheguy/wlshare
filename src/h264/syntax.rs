//! The H.264 headers the hardware's stream carries, written by the daemon:
//! its sequence and picture parameter sets and each slice's header, which
//! libva's encoders take as packed headers and put ahead of the slice data
//! they code. One shape of stream alone, the one [`super::vaapi`] codes: High
//! profile 4:2:0 at eight bits, progressive frames, CABAC, the 8×8 transform,
//! one slice a picture, P frames of one reference, no reordering, picture
//! order counted from the frame number (type 2), and a quantizer that each
//! slice moves from the picture set's.
//!
//! Each header is a whole NAL unit, its start code first and its emulation
//! prevention bytes in place; every one ends on a byte, the slice header
//! padded by CABAC's alignment bits, so a driver appends the slice data at a
//! byte.

/// `log2_max_frame_num`: frame numbers count to 256 and wrap.
pub const LOG2_MAX_FRAME_NUM: u32 = 8;

/// The quantizer the picture parameter set starts each slice from; a slice
/// says its own as a delta from it.
pub const PIC_INIT_QP: u8 = 26;

/// What the sequence parameter set says of a stream.
#[derive(Clone, Copy, Debug)]
pub struct Sequence {
    pub level_idc: u8,
    /// The picture in macroblocks.
    pub mbs: (u32, u32),
    /// The picture coded, its sides even: what is past it in the last
    /// macroblocks is cropped.
    pub size: (u32, u32),
}

/// What a slice's header says of its picture.
#[derive(Clone, Copy, Debug)]
pub struct Slice {
    /// An IDR picture, an I slice; otherwise a P slice.
    pub idr: bool,
    pub frame_num: u32,
    pub idr_pic_id: u32,
    pub qp: u8,
}

/// The NAL unit types and reference priorities written here.
const NAL_SLICE: u8 = 1;
const NAL_IDR: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;

impl Sequence {
    /// The sequence's crop, in 4:2:0's two-pixel units: right and bottom.
    pub fn crop(&self) -> (u32, u32) {
        ((self.mbs.0 * 16 - self.size.0) / 2, (self.mbs.1 * 16 - self.size.1) / 2)
    }

    /// The sequence parameter set, id 0.
    pub fn sps(&self) -> Vec<u8> {
        let mut w = Writer::nal(3, NAL_SPS);
        w.bits(100, 8); // profile_idc: High
        w.bits(0, 8); // constraint_set0..5_flag, reserved_zero_2bits
        w.bits(u32::from(self.level_idc), 8);
        w.ue(0); // seq_parameter_set_id
        w.ue(1); // chroma_format_idc: 4:2:0
        w.ue(0); // bit_depth_luma_minus8
        w.ue(0); // bit_depth_chroma_minus8
        w.bit(false); // qpprime_y_zero_transform_bypass_flag
        w.bit(false); // seq_scaling_matrix_present_flag
        w.ue(LOG2_MAX_FRAME_NUM - 4);
        w.ue(2); // pic_order_cnt_type
        w.ue(1); // max_num_ref_frames
        w.bit(false); // gaps_in_frame_num_value_allowed_flag
        w.ue(self.mbs.0 - 1);
        w.ue(self.mbs.1 - 1);
        w.bit(true); // frame_mbs_only_flag
        w.bit(true); // direct_8x8_inference_flag
        let (right, bottom) = self.crop();
        w.bit(right != 0 || bottom != 0); // frame_cropping_flag
        if right != 0 || bottom != 0 {
            w.ue(0);
            w.ue(right);
            w.ue(0);
            w.ue(bottom);
        }
        w.bit(true); // vui_parameters_present_flag
        w.bit(false); // aspect_ratio_info_present_flag
        w.bit(false); // overscan_info_present_flag
        w.bit(true); // video_signal_type_present_flag
        w.bits(5, 3); // video_format: unspecified
        w.bit(false); // video_full_range_flag: studio swing
        w.bit(true); // colour_description_present_flag
        w.bits(6, 8); // colour_primaries: SMPTE 170M
        w.bits(6, 8); // transfer_characteristics: SMPTE 170M
        w.bits(6, 8); // matrix_coefficients: BT.601
        w.bit(false); // chroma_loc_info_present_flag
        w.bit(false); // timing_info_present_flag
        w.bit(false); // nal_hrd_parameters_present_flag
        w.bit(false); // vcl_hrd_parameters_present_flag
        w.bit(false); // pic_struct_present_flag
        // No picture is held for reordering, so a decoder outputs each the
        // moment it is decoded rather than filling its buffer first.
        w.bit(true); // bitstream_restriction_flag
        w.bit(true); // motion_vectors_over_pic_boundaries_flag
        w.ue(0); // max_bytes_per_pic_denom
        w.ue(0); // max_bits_per_mb_denom
        w.ue(15); // log2_max_mv_length_horizontal
        w.ue(15); // log2_max_mv_length_vertical
        w.ue(0); // max_num_reorder_frames
        w.ue(1); // max_dec_frame_buffering
        w.trailing();
        w.finish()
    }
}

/// The picture parameter set, id 0 over sequence 0.
pub fn pps() -> Vec<u8> {
    let mut w = Writer::nal(3, NAL_PPS);
    w.ue(0); // pic_parameter_set_id
    w.ue(0); // seq_parameter_set_id
    w.bit(true); // entropy_coding_mode_flag: CABAC
    w.bit(false); // bottom_field_pic_order_in_frame_present_flag
    w.ue(0); // num_slice_groups_minus1
    w.ue(0); // num_ref_idx_l0_default_active_minus1
    w.ue(0); // num_ref_idx_l1_default_active_minus1
    w.bit(false); // weighted_pred_flag
    w.bits(0, 2); // weighted_bipred_idc
    w.se(i32::from(PIC_INIT_QP) - 26); // pic_init_qp_minus26
    w.se(0); // pic_init_qs_minus26
    w.se(0); // chroma_qp_index_offset
    w.bit(true); // deblocking_filter_control_present_flag
    w.bit(false); // constrained_intra_pred_flag
    w.bit(false); // redundant_pic_cnt_present_flag
    w.bit(true); // transform_8x8_mode_flag
    w.bit(false); // pic_scaling_matrix_present_flag
    w.se(0); // second_chroma_qp_index_offset
    w.trailing();
    w.finish()
}

impl Slice {
    /// The slice's `slice_type`: 2 for I, 0 for P.
    pub fn slice_type(&self) -> u8 {
        if self.idr { 2 } else { 0 }
    }

    /// The slice's header, the whole picture's one slice, up to its data.
    pub fn header(&self) -> Vec<u8> {
        let mut w = if self.idr { Writer::nal(3, NAL_IDR) } else { Writer::nal(2, NAL_SLICE) };
        w.ue(0); // first_mb_in_slice
        w.ue(u32::from(self.slice_type()));
        w.ue(0); // pic_parameter_set_id
        w.bits(self.frame_num, LOG2_MAX_FRAME_NUM);
        if self.idr {
            w.ue(self.idr_pic_id);
        }
        if !self.idr {
            w.bit(false); // num_ref_idx_active_override_flag
            w.bit(false); // ref_pic_list_modification_flag_l0
        }
        // dec_ref_pic_marking: a reference picture, marked by the sliding
        // window, which with one reference frame keeps the last alone.
        if self.idr {
            w.bit(false); // no_output_of_prior_pics_flag
            w.bit(false); // long_term_reference_flag
        } else {
            w.bit(false); // adaptive_ref_pic_marking_mode_flag
            w.ue(0); // cabac_init_idc
        }
        w.se(i32::from(self.qp) - i32::from(PIC_INIT_QP)); // slice_qp_delta
        w.ue(0); // disable_deblocking_filter_idc
        w.se(0); // slice_alpha_c0_offset_div2
        w.se(0); // slice_beta_offset_div2
        // cabac_alignment_one_bit, up to the byte the slice data starts on.
        while w.used != 0 {
            w.bit(true);
        }
        w.finish()
    }
}

/// The lowest level whose decoder holds a `mbs` picture and decodes it at
/// [`super::FPS`]: H.264's table A-1, its frame size and macroblock rate.
pub fn level_idc(mbs: (u32, u32)) -> u8 {
    const LEVELS: [(u8, u32, u32); 17] = [
        (10, 99, 1_485),
        (11, 396, 3_000),
        (12, 396, 6_000),
        (13, 396, 11_880),
        (20, 396, 11_880),
        (21, 792, 19_800),
        (22, 1_620, 20_250),
        (30, 1_620, 40_500),
        (31, 3_600, 108_000),
        (32, 5_120, 216_000),
        (40, 8_192, 245_760),
        (42, 8_704, 522_240),
        (50, 22_080, 589_824),
        (51, 36_864, 983_040),
        (52, 36_864, 2_073_600),
        (60, 139_264, 4_177_920),
        (62, 139_264, 16_711_680),
    ];
    let frame = mbs.0 * mbs.1;
    let rate = frame * super::FPS;
    // Also no side over the square root of eight times the frame size.
    let fits = |max_fs: u32| frame <= max_fs && u64::from(mbs.0.max(mbs.1)).pow(2) <= u64::from(max_fs) * 8;
    LEVELS.iter().find(|&&(_, max_fs, max_rate)| fits(max_fs) && rate <= max_rate).map_or(62, |&(level, ..)| level)
}

/// A NAL unit written a bit at a time, most significant first, its emulation
/// prevention bytes put in as each byte is.
struct Writer {
    out: Vec<u8>,
    /// The bits of the byte being written, and how many.
    byte: u8,
    used: u32,
    /// Zero bytes in a row at the end of the payload.
    zeros: u32,
}

impl Writer {
    /// A unit's start code and header.
    fn nal(ref_idc: u8, kind: u8) -> Self {
        Self { out: vec![0, 0, 0, 1, ref_idc << 5 | kind], byte: 0, used: 0, zeros: 0 }
    }

    fn bit(&mut self, bit: bool) {
        self.byte = self.byte << 1 | u8::from(bit);
        self.used += 1;
        if self.used == 8 {
            // Three bytes 00 00 0x, x up to 3, would read as a start code.
            if self.zeros >= 2 && self.byte <= 3 {
                self.out.push(3);
                self.zeros = 0;
            }
            self.out.push(self.byte);
            self.zeros = if self.byte == 0 { self.zeros + 1 } else { 0 };
            (self.byte, self.used) = (0, 0);
        }
    }

    fn bits(&mut self, value: u32, count: u32) {
        for at in (0..count).rev() {
            self.bit(value >> at & 1 == 1);
        }
    }

    /// Exp-Golomb, unsigned.
    fn ue(&mut self, value: u32) {
        let coded = u64::from(value) + 1;
        let len = 64 - coded.leading_zeros();
        self.bits(0, len - 1);
        for at in (0..len).rev() {
            self.bit(coded >> at & 1 == 1);
        }
    }

    /// Exp-Golomb, signed: 1, −1, 2, −2… as 1, 2, 3, 4….
    fn se(&mut self, value: i32) {
        self.ue(if value > 0 { (value as u32) * 2 - 1 } else { value.unsigned_abs() * 2 });
    }

    /// `rbsp_trailing_bits`: a one, then zeros to the byte.
    fn trailing(&mut self) {
        self.bit(true);
        while self.used != 0 {
            self.bit(false);
        }
    }

    fn finish(self) -> Vec<u8> {
        debug_assert_eq!(self.used, 0, "a header ends on a byte");
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(f: impl FnOnce(&mut Writer)) -> Vec<u8> {
        let mut w = Writer::nal(0, 0);
        f(&mut w);
        while w.used != 0 {
            w.bit(false);
        }
        w.out.split_off(5)
    }

    #[test]
    fn exp_golomb_codes_are_h264s() {
        // 0 → 1, 1 → 010, 2 → 011, 3 → 00100, 7 → 0001000.
        assert_eq!(written(|w| [0, 1, 2, 3].into_iter().for_each(|v| w.ue(v))), [0b1010_0110, 0b0100_0000]);
        assert_eq!(written(|w| w.ue(7)), [0b0001_0000]);
        // se: 1 → 010, −1 → 011, 0 → 1, −3 → 00111.
        assert_eq!(written(|w| [1, -1, 0, -3].into_iter().for_each(|v| w.se(v))), [0b0100_1110, 0b0111_0000]);
    }

    #[test]
    fn three_bytes_that_would_read_as_a_start_code_get_a_prevention_byte() {
        assert_eq!(written(|w| w.bits(0x0000_0102, 32)), [0, 0, 3, 1, 2]);
        assert_eq!(written(|w| w.bits(0x0000_0004, 24)), [0, 0, 4]);
        assert_eq!(written(|w| w.bits(0x0000_0000, 32)), [0, 0, 3, 0, 0]);
    }

    #[test]
    fn the_level_is_the_lowest_that_holds_the_picture_at_60_frames() {
        // 1920x1080: 120x68 macroblocks, 8160 a frame, 489600 a second.
        assert_eq!(level_idc((120, 68)), 42);
        // 3840x2160: 32400 a frame.
        assert_eq!(level_idc((240, 135)), 52);
        assert_eq!(level_idc((4, 4)), 10);
        // A frame of 8192 macroblocks at 60 is over level 4.1's rate, and one
        // of 8840 over level 4.2's frame.
        assert_eq!(level_idc((128, 64)), 42);
        assert_eq!(level_idc((130, 68)), 50);
    }

    /// ffmpeg's own reading of the headers, its `trace_headers` filter over a
    /// stream of them: the sequence, picture and slice headers, an IDR's and a
    /// P frame's, say what they were written to say. Skipped, with a word,
    /// where `ffmpeg` is not on the path.
    #[test]
    fn ffmpeg_reads_the_headers_as_written() {
        let sequence = Sequence { level_idc: 42, mbs: (120, 68), size: (1920, 1080) };
        let mut stream = sequence.sps();
        stream.extend(pps());
        for slice in [Slice { idr: true, frame_num: 0, idr_pic_id: 3, qp: 18 }, Slice { idr: false, frame_num: 37, idr_pic_id: 0, qp: 35 }] {
            stream.extend(slice.header());
            // Slice data for the filter to pass over, which it leaves alone.
            stream.extend([0x88, 0x84, 0x00, 0x33, 0xFF]);
        }
        let dir = std::env::temp_dir().join(format!("wlshare-h264-syntax-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("headers.h264");
        std::fs::write(&path, &stream).unwrap();
        // `-copyinkf`, or the P frame is dropped as coming before a keyframe
        // the copy can start from.
        let run = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "trace", "-f", "h264", "-i"])
            .arg(&path)
            .args(["-c", "copy", "-copyinkf", "-bsf:v", "trace_headers", "-f", "null", "-"])
            .output();
        std::fs::remove_dir_all(&dir).ok();
        let Ok(run) = run else {
            eprintln!("no ffmpeg on the path; the headers are not read back");
            return;
        };
        let trace = String::from_utf8_lossy(&run.stderr);
        // Every value the trace gives a field, in order.
        let said = |name: &str| -> Vec<i64> {
            trace
                .lines()
                .filter(|l| l.starts_with("[trace_headers") && l.split_whitespace().any(|token| token == name))
                .map(|l| l.rsplit('=').next().unwrap().trim().parse().unwrap())
                .collect()
        };
        // The parameter sets are traced twice, from the stream's extradata and
        // from its first packet.
        for (name, value) in [
            ("profile_idc", 100),
            ("level_idc", 42),
            ("chroma_format_idc", 1),
            ("pic_order_cnt_type", 2),
            ("max_num_ref_frames", 1),
            ("pic_width_in_mbs_minus1", 119),
            ("pic_height_in_map_units_minus1", 67),
            ("frame_crop_bottom_offset", 4),
            ("video_full_range_flag", 0),
            ("matrix_coefficients", 6),
            ("max_num_reorder_frames", 0),
            ("max_dec_frame_buffering", 1),
            ("entropy_coding_mode_flag", 1),
            ("transform_8x8_mode_flag", 1),
        ] {
            let values = said(name);
            assert!(!values.is_empty() && values.iter().all(|&v| v == value), "{name}: {values:?}, not {value}, in ffmpeg's trace:\n{trace}");
        }
        for (name, values) in [
            ("first_mb_in_slice", &[0, 0][..]),
            ("slice_type", &[2, 0]),
            ("frame_num", &[0, 37]),
            ("idr_pic_id", &[3]),
            ("slice_qp_delta", &[-8, 9]),
            ("disable_deblocking_filter_idc", &[0, 0]),
        ] {
            assert_eq!(said(name), values, "{name}, in ffmpeg's trace:\n{trace}");
        }
    }
}
