//! HEVC's headers read back, for the encoder's trial frames and its tests: the
//! NAL units of an access unit, and each slice's quantizer, which libavcodec's
//! HEVC decoder does not report as its H.264 one does.
//!
//! Written from H.265's syntax tables (7.3), independent of the encoder: the
//! sequence and picture parameter sets as far as a slice header needs them,
//! and each slice segment's header as far as `slice_qp_delta`, its QP being
//! `26 + init_qp_minus26 + slice_qp_delta`. What it does not read — a scaling
//! list, a reference list's modification, a weighted prediction table — it
//! refuses rather than guesses past, so a stream it reads is read right.

use anyhow::{Context as _, bail, ensure};

/// NAL unit types (7.4.2.2).
pub const IDR_W_RADL: u8 = 19;
pub const IDR_N_LP: u8 = 20;
pub const VPS: u8 = 32;
pub const SPS: u8 = 33;
pub const PPS: u8 = 34;

/// The NAL units of an Annex B access unit, as their types and payloads, the
/// two-byte header off and the emulation prevention bytes in.
pub fn nal_units(unit: &[u8]) -> Vec<(u8, &[u8])> {
    let mut starts = Vec::new();
    let mut at = 0;
    while at + 3 <= unit.len() {
        if unit[at] == 0 && unit[at + 1] == 0 && unit[at + 2] == 1 {
            starts.push(at + 3);
            at += 3;
        } else {
            at += 1;
        }
    }
    let mut units = Vec::new();
    for (n, &start) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(unit.len(), |&next| next - 3);
        // A four-byte start code's leading zero, and trailing zeros, are not
        // the unit's.
        while end > start && unit[end - 1] == 0 {
            end -= 1;
        }
        if end >= start + 2 {
            units.push(((unit[start] >> 1) & 0x3f, &unit[start + 2..end]));
        }
    }
    units
}

/// The NAL unit types of an access unit, in order.
pub fn nal_types(unit: &[u8]) -> Vec<u8> {
    nal_units(unit).into_iter().map(|(kind, _)| kind).collect()
}

/// What a sequence parameter set says that a slice header is read by, and
/// what the tests hold the stream to.
#[derive(Debug, Clone)]
pub struct Sps {
    pub profile_idc: u8,
    pub chroma_format_idc: u32,
    pub size: (u32, u32),
    /// The conformance window's crop, in luma samples: left, right, top, bottom.
    pub crop: [u32; 4],
    pub bit_depth: (u32, u32),
    separate_colour_plane: bool,
    log2_max_poc_lsb: u32,
    sao: bool,
    num_short_term_ref_pic_sets: u32,
    /// Each of the sequence's short-term reference sets' `NumDeltaPocs`.
    num_delta_pocs: Vec<u32>,
    long_term_refs: Option<u32>,
    temporal_mvp: bool,
    ctbs: u32,
    /// The VUI's colour description, where it has one: video_full_range_flag,
    /// colour_primaries, transfer_characteristics, matrix_coeffs.
    pub colour: Option<(bool, u8, u8, u8)>,
}

/// What a picture parameter set says that a slice header is read by.
#[derive(Debug, Clone)]
struct Pps {
    sps: u32,
    dependent_slice_segments: bool,
    output_flag_present: bool,
    num_extra_slice_header_bits: u32,
    cabac_init_present: bool,
    num_ref_idx_default: (u32, u32),
    init_qp: i32,
    cu_qp_delta: bool,
    weighted: (bool, bool),
    lists_modification_present: bool,
}

/// A slice segment as its header has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slice {
    pub nal_type: u8,
    /// 0 B, 1 P, 2 I.
    pub slice_type: u32,
    pub qp: i32,
    /// Whether a coding unit may stand off the slice's QP.
    pub cu_qp_delta: bool,
}

/// The parameter sets a stream has carried so far, which its slices are read
/// against.
#[derive(Debug, Default)]
pub struct Headers {
    sps: Vec<Option<Sps>>,
    pps: Vec<Option<Pps>>,
}

impl Headers {
    /// Read an access unit: its parameter sets kept, and its slices returned.
    pub fn read(&mut self, unit: &[u8]) -> anyhow::Result<Vec<Slice>> {
        let mut slices = Vec::new();
        for (kind, payload) in nal_units(unit) {
            let rbsp = rbsp(payload);
            let mut r = Bits::new(&rbsp);
            match kind {
                SPS => {
                    let (id, sps) = read_sps(&mut r).context("reading a sequence parameter set")?;
                    put(&mut self.sps, id, sps);
                }
                PPS => {
                    let (id, pps) = read_pps(&mut r).context("reading a picture parameter set")?;
                    put(&mut self.pps, id, pps);
                }
                0..=21 => slices.push(self.read_slice(kind, &mut r).with_context(|| format!("reading a slice of NAL type {kind}"))?),
                _ => {}
            }
        }
        Ok(slices)
    }

    /// The sequence parameter set of the given id, as last carried.
    pub fn sps(&self, id: u32) -> Option<&Sps> {
        self.sps.get(id as usize).and_then(Option::as_ref)
    }

    fn read_slice(&self, nal_type: u8, r: &mut Bits) -> anyhow::Result<Slice> {
        let first = r.flag()?;
        if (16..=23).contains(&nal_type) {
            r.flag()?; // no_output_of_prior_pics_flag
        }
        let pps_id = r.ue()?;
        let pps = self.pps.get(pps_id as usize).and_then(Option::as_ref).with_context(|| format!("no picture parameter set {pps_id}"))?;
        let sps = self.sps(pps.sps).with_context(|| format!("no sequence parameter set {}", pps.sps))?;
        if !first {
            if pps.dependent_slice_segments {
                ensure!(!r.flag()?, "a dependent slice segment, which carries no QP of its own");
            }
            r.bits(ceil_log2(sps.ctbs))?; // slice_segment_address
        }
        r.bits(pps.num_extra_slice_header_bits)?; // slice_reserved_flag
        let slice_type = r.ue()?;
        ensure!(slice_type <= 2, "slice type {slice_type}");
        if pps.output_flag_present {
            r.flag()?; // pic_output_flag
        }
        if sps.separate_colour_plane {
            r.bits(2)?; // colour_plane_id
        }
        let mut temporal_mvp = false;
        if nal_type != IDR_W_RADL && nal_type != IDR_N_LP {
            r.bits(sps.log2_max_poc_lsb)?; // slice_pic_order_cnt_lsb
            if !r.flag()? {
                // short_term_ref_pic_set_sps_flag clear: a set of the slice's own.
                st_ref_pic_set(r, sps.num_short_term_ref_pic_sets, sps.num_short_term_ref_pic_sets, &sps.num_delta_pocs)?;
            } else if sps.num_short_term_ref_pic_sets > 1 {
                r.bits(ceil_log2(sps.num_short_term_ref_pic_sets))?; // short_term_ref_pic_set_idx
            }
            if let Some(sps_lts) = sps.long_term_refs {
                let from_sps = if sps_lts > 0 { r.ue()? } else { 0 };
                let own = r.ue()?;
                for i in 0..from_sps + own {
                    if i < from_sps {
                        if sps_lts > 1 {
                            r.bits(ceil_log2(sps_lts))?; // lt_idx_sps
                        }
                    } else {
                        r.bits(sps.log2_max_poc_lsb)?; // poc_lsb_lt
                        r.flag()?; // used_by_curr_pic_lt_flag
                    }
                    if r.flag()? {
                        r.ue()?; // delta_poc_msb_cycle_lt
                    }
                }
            }
            if sps.temporal_mvp {
                temporal_mvp = r.flag()?;
            }
        }
        if sps.sao {
            r.flag()?; // slice_sao_luma_flag
            if !sps.separate_colour_plane && sps.chroma_format_idc != 0 {
                r.flag()?; // slice_sao_chroma_flag
            }
        }
        if slice_type != 2 {
            let b = slice_type == 0;
            let (mut l0, mut l1) = pps.num_ref_idx_default;
            if r.flag()? {
                l0 = r.ue()?;
                if b {
                    l1 = r.ue()?;
                }
            }
            ensure!(!pps.lists_modification_present, "a reference list's modification, which this does not read");
            if b {
                r.flag()?; // mvd_l1_zero_flag
            }
            if pps.cabac_init_present {
                r.flag()?; // cabac_init_flag
            }
            if temporal_mvp {
                let from_l0 = if b { r.flag()? } else { true };
                if (from_l0 && l0 > 0) || (!from_l0 && l1 > 0) {
                    r.ue()?; // collocated_ref_idx
                }
            }
            ensure!(!(pps.weighted.0 && !b) && !(pps.weighted.1 && b), "a weighted prediction table, which this does not read");
            r.ue()?; // five_minus_max_num_merge_cand
        }
        let delta = r.se()?;
        Ok(Slice { nal_type, slice_type, qp: 26 + pps.init_qp + delta, cu_qp_delta: pps.cu_qp_delta })
    }
}

fn put<T>(sets: &mut Vec<Option<T>>, id: u32, set: T) {
    let id = id as usize;
    if sets.len() <= id {
        sets.resize_with(id + 1, || None);
    }
    sets[id] = Some(set);
}

fn read_sps(r: &mut Bits) -> anyhow::Result<(u32, Sps)> {
    r.bits(4)?; // sps_video_parameter_set_id
    let max_sub_layers_minus1 = r.bits(3)?;
    r.flag()?; // sps_temporal_id_nesting_flag
    let profile_idc = profile_tier_level(r, max_sub_layers_minus1)?;
    let id = r.ue()?;
    let chroma_format_idc = r.ue()?;
    let separate_colour_plane = chroma_format_idc == 3 && r.flag()?;
    let size = (r.ue()?, r.ue()?);
    let mut crop = [0; 4];
    if r.flag()? {
        for side in &mut crop {
            *side = r.ue()?;
        }
        // In chroma samples (7.4.3.2.1): two luma samples a unit where the
        // chroma is halved that way.
        let (sub_w, sub_h) = match chroma_format_idc {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        };
        crop = [crop[0] * sub_w, crop[1] * sub_w, crop[2] * sub_h, crop[3] * sub_h];
    }
    let bit_depth = (r.ue()? + 8, r.ue()? + 8);
    let log2_max_poc_lsb = r.ue()? + 4;
    let ordering_all = r.flag()?;
    for _ in if ordering_all { 0 } else { max_sub_layers_minus1 }..=max_sub_layers_minus1 {
        r.ue()?;
        r.ue()?;
        r.ue()?;
    }
    let log2_min_cb = r.ue()? + 3;
    let log2_ctb = log2_min_cb + r.ue()?;
    r.ue()?; // log2_min_luma_transform_block_size_minus2
    r.ue()?; // log2_diff_max_min_luma_transform_block_size
    r.ue()?; // max_transform_hierarchy_depth_inter
    r.ue()?; // max_transform_hierarchy_depth_intra
    if r.flag()? {
        ensure!(!r.flag()?, "a scaling list, which this does not read");
    }
    r.flag()?; // amp_enabled_flag
    let sao = r.flag()?;
    if r.flag()? {
        // pcm_enabled_flag
        r.bits(8)?;
        r.ue()?;
        r.ue()?;
        r.flag()?;
    }
    let num_short_term_ref_pic_sets = r.ue()?;
    ensure!(num_short_term_ref_pic_sets <= 64, "{num_short_term_ref_pic_sets} short-term reference sets");
    let mut num_delta_pocs = Vec::new();
    for idx in 0..num_short_term_ref_pic_sets {
        let n = st_ref_pic_set(r, idx, num_short_term_ref_pic_sets, &num_delta_pocs)?;
        num_delta_pocs.push(n);
    }
    let long_term_refs = if r.flag()? {
        let n = r.ue()?;
        for _ in 0..n {
            r.bits(log2_max_poc_lsb)?;
            r.flag()?;
        }
        Some(n)
    } else {
        None
    };
    let temporal_mvp = r.flag()?;
    r.flag()?; // strong_intra_smoothing_enabled_flag
    let colour = if r.flag()? { vui(r)? } else { None };
    let ctb = 1 << log2_ctb;
    let ctbs = size.0.div_ceil(ctb) * size.1.div_ceil(ctb);
    Ok((
        id,
        Sps {
            profile_idc,
            chroma_format_idc,
            size,
            crop,
            bit_depth,
            separate_colour_plane,
            log2_max_poc_lsb,
            sao,
            num_short_term_ref_pic_sets,
            num_delta_pocs,
            long_term_refs,
            temporal_mvp,
            ctbs,
            colour,
        },
    ))
}

/// The VUI as far as its colour description (E.2.1).
fn vui(r: &mut Bits) -> anyhow::Result<Option<(bool, u8, u8, u8)>> {
    if r.flag()? {
        // aspect_ratio_info_present_flag
        if r.bits(8)? == 255 {
            r.bits(32)?;
        }
    }
    if r.flag()? {
        r.flag()?; // overscan_appropriate_flag
    }
    if !r.flag()? {
        return Ok(None);
    }
    r.bits(3)?; // video_format
    let full = r.flag()?;
    if !r.flag()? {
        return Ok(Some((full, 2, 2, 2)));
    }
    Ok(Some((full, r.bits(8)? as u8, r.bits(8)? as u8, r.bits(8)? as u8)))
}

/// `profile_tier_level(1, max_sub_layers_minus1)` (7.3.3): the general
/// profile's idc.
fn profile_tier_level(r: &mut Bits, max_sub_layers_minus1: u32) -> anyhow::Result<u8> {
    r.bits(2)?; // general_profile_space
    r.flag()?; // general_tier_flag
    let profile_idc = r.bits(5)? as u8;
    r.bits(32)?; // general_profile_compatibility_flag
    r.bits(4)?; // progressive, interlaced, non-packed, frame-only
    r.bits(32)?; // 43 bits of constraints, and one more
    r.bits(12)?;
    r.bits(8)?; // general_level_idc
    let mut present = Vec::new();
    for _ in 0..max_sub_layers_minus1 {
        present.push((r.flag()?, r.flag()?));
    }
    if max_sub_layers_minus1 > 0 {
        for _ in max_sub_layers_minus1..8 {
            r.bits(2)?;
        }
    }
    for (profile, level) in present {
        if profile {
            r.bits(32)?;
            r.bits(32)?;
            r.bits(24)?;
        }
        if level {
            r.bits(8)?;
        }
    }
    Ok(profile_idc)
}

/// `st_ref_pic_set(idx)` (7.3.7): its `NumDeltaPocs`, read against the sets
/// before it, `num_delta_pocs`.
fn st_ref_pic_set(r: &mut Bits, idx: u32, sets: u32, num_delta_pocs: &[u32]) -> anyhow::Result<u32> {
    if idx != 0 && r.flag()? {
        // inter_ref_pic_set_prediction_flag
        let delta_idx = if idx == sets { r.ue()? + 1 } else { 1 };
        ensure!(delta_idx <= idx, "a reference set predicted from one {delta_idx} before set {idx}");
        r.flag()?; // delta_rps_sign
        r.ue()?; // abs_delta_rps_minus1
        let reference = num_delta_pocs[(idx - delta_idx) as usize];
        let mut n = 0;
        for _ in 0..=reference {
            let used = r.flag()?;
            if used || r.flag()? {
                n += 1;
            }
        }
        return Ok(n);
    }
    let negative = r.ue()?;
    let positive = r.ue()?;
    ensure!(negative + positive <= 32, "a reference set of {negative} and {positive}");
    for _ in 0..negative + positive {
        r.ue()?;
        r.flag()?;
    }
    Ok(negative + positive)
}

fn read_pps(r: &mut Bits) -> anyhow::Result<(u32, Pps)> {
    let id = r.ue()?;
    let sps = r.ue()?;
    let dependent_slice_segments = r.flag()?;
    let output_flag_present = r.flag()?;
    let num_extra_slice_header_bits = r.bits(3)?;
    r.flag()?; // sign_data_hiding_enabled_flag
    let cabac_init_present = r.flag()?;
    let num_ref_idx_default = (r.ue()?, r.ue()?);
    let init_qp = 26 + r.se()?;
    r.flag()?; // constrained_intra_pred_flag
    r.flag()?; // transform_skip_enabled_flag
    let cu_qp_delta = r.flag()?;
    if cu_qp_delta {
        r.ue()?; // diff_cu_qp_delta_depth
    }
    r.se()?; // pps_cb_qp_offset
    r.se()?; // pps_cr_qp_offset
    r.flag()?; // pps_slice_chroma_qp_offsets_present_flag
    let weighted = (r.flag()?, r.flag()?);
    r.flag()?; // transquant_bypass_enabled_flag
    let tiles = r.flag()?;
    r.flag()?; // entropy_coding_sync_enabled_flag
    if tiles {
        let (columns, rows) = (r.ue()?, r.ue()?);
        if !r.flag()? {
            for _ in 0..columns + rows {
                r.ue()?;
            }
        }
        r.flag()?; // loop_filter_across_tiles_enabled_flag
    }
    r.flag()?; // pps_loop_filter_across_slices_enabled_flag
    if r.flag()? {
        // deblocking_filter_control_present_flag
        r.flag()?; // deblocking_filter_override_enabled_flag
        if !r.flag()? {
            r.se()?;
            r.se()?;
        }
    }
    ensure!(!r.flag()?, "a scaling list, which this does not read");
    let lists_modification_present = r.flag()?;
    Ok((
        id,
        Pps {
            sps,
            dependent_slice_segments,
            output_flag_present,
            num_extra_slice_header_bits,
            cabac_init_present,
            num_ref_idx_default: (num_ref_idx_default.0, num_ref_idx_default.1),
            init_qp: init_qp - 26,
            cu_qp_delta,
            weighted,
            lists_modification_present,
        },
    ))
}

fn ceil_log2(n: u32) -> u32 {
    if n <= 1 { 0 } else { 32 - (n - 1).leading_zeros() }
}

/// A NAL unit's payload with its emulation prevention bytes taken out.
fn rbsp(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zeros = 0;
    for &byte in payload {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        zeros = if byte == 0 { zeros + 1 } else { 0 };
        out.push(byte);
    }
    out
}

/// A big-endian bit reader over an RBSP.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn bits(&mut self, n: u32) -> anyhow::Result<u32> {
        ensure!(n <= 32, "{n} bits at once");
        let mut value = 0u64;
        for _ in 0..n {
            let byte = *self.data.get(self.at / 8).context("the header ends early")?;
            value = (value << 1) | u64::from((byte >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Ok(value as u32)
    }

    fn flag(&mut self) -> anyhow::Result<bool> {
        Ok(self.bits(1)? == 1)
    }

    fn ue(&mut self) -> anyhow::Result<u32> {
        let mut zeros = 0;
        while !self.flag()? {
            zeros += 1;
            if zeros > 31 {
                bail!("an Exp-Golomb code past 32 bits");
            }
        }
        Ok(((1u64 << zeros) - 1 + u64::from(self.bits(zeros)?)) as u32)
    }

    fn se(&mut self) -> anyhow::Result<i32> {
        let k = i64::from(self.ue()?);
        Ok(if k % 2 == 1 { (k + 1) / 2 } else { -k / 2 } as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_codes_read_as_the_spec_numbers_them() {
        // 1, 010, 011, 00100, 00101 → 0, 1, 2, 3, 4; then 011 as se → -1.
        let data = [0b1010_0110, 0b0100_0010, 0b1011_0000];
        let mut r = Bits::new(&data);
        assert_eq!([r.ue().unwrap(), r.ue().unwrap(), r.ue().unwrap(), r.ue().unwrap(), r.ue().unwrap()], [0, 1, 2, 3, 4]);
        assert_eq!(r.se().unwrap(), -1);
    }

    #[test]
    fn emulation_prevention_bytes_come_out() {
        assert_eq!(rbsp(&[0, 0, 3, 1, 0, 0, 3, 0, 5]), [0, 0, 1, 0, 0, 0, 5]);
    }

    #[test]
    fn nal_units_are_split_at_their_start_codes() {
        let unit = [0, 0, 0, 1, 0x40, 0x01, 0xAA, 0, 0, 1, 0x42, 0x01, 0xBB, 0xCC, 0, 0, 1, 0x26, 0x01, 0xDD];
        let units = nal_units(&unit);
        assert_eq!(units.iter().map(|u| u.0).collect::<Vec<_>>(), [VPS, SPS, IDR_W_RADL]);
        assert_eq!(units[1].1, [0xBB, 0xCC]);
    }

    #[test]
    fn ceil_log2_counts_the_bits_an_index_needs() {
        assert_eq!([ceil_log2(1), ceil_log2(2), ceil_log2(3), ceil_log2(4), ceil_log2(5)], [0, 1, 2, 2, 3]);
    }

    /// The headers of a stream libx265 coded, through libavcodec, at a
    /// constant QP with no adaptive quantization: every slice reads back at
    /// that QP, the first an IDR, the rest P frames, and the sequence as
    /// coded. An encoder that shares nothing with this reader or with NVENC.
    #[test]
    fn libx265s_slices_read_back_at_the_qp_they_were_coded_at() {
        let (w, h) = (128, 72);
        let units = crate::decode::tests::x265(w, h, 6, 31);
        let mut headers = Headers::default();
        for (n, unit) in units.iter().enumerate() {
            let slices = headers.read(unit).unwrap();
            assert!(!slices.is_empty(), "frame {n} has a slice");
            for slice in slices {
                assert_eq!(slice.qp, 31, "frame {n}'s slice QP");
                assert_eq!(slice.slice_type == 2, n == 0, "frame {n}: intra only first");
            }
        }
        let sps = headers.sps(0).expect("a sequence parameter set");
        assert_eq!((sps.chroma_format_idc, sps.profile_idc), (3, 4), "4:4:4, Range Extensions");
        assert_eq!((sps.size.0 - sps.crop[0] - sps.crop[1], sps.size.1 - sps.crop[2] - sps.crop[3]), (w as u32, h as u32));
    }
}
