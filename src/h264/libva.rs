//! libva's C API, written out by hand: what `vaapi.rs` calls of the general
//! API, `va/va.h`, and of `va/va_drm.h`, and H.264's encode parameters,
//! `va/va_enc_h264.h`; libva and libva-drm linked dynamically, `libva.so.2`
//! and `libva-drm.so.2` from the system.
//!
//! Written here rather than generated from the headers at build time, so that
//! the build reads no header and runs no libclang for them: libva's ABI is
//! `libva.so.2`'s since 2.0, each structure keeping its layout (a field added
//! takes from its `va_reserved` padding) and each enum its values (one added
//! is appended). Every release's headers from 2.1.0, the first to declare
//! `VA_CODED_BUF_STATUS_BAD_BITSTREAM` and `vui_fields`'
//! `motion_vectors_over_pic_boundaries_flag`, to 2.24.1 give these structures
//! the same sizes, offsets and bitfield words and these constants the same
//! values, so the package depends on libva 2.1.0 onward (`Cargo.toml`), the
//! `libva2` package being `libva.so.2` alone. Each structure's size and the
//! offsets after its padding are asserted below as the C compiler lays out
//! those headers, and each bitfield word as it packs them.
//!
//! Enums are their underlying integers with a constant for each value used.

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

use std::ffi::{c_char, c_int, c_uint, c_void};
use std::mem::{offset_of, size_of};

pub type VADisplay = *mut c_void;
pub type VAStatus = c_int;
pub type VAGenericID = c_uint;
pub type VAConfigID = VAGenericID;
pub type VAContextID = VAGenericID;
pub type VASurfaceID = VAGenericID;
pub type VABufferID = VAGenericID;
pub type VAImageID = VAGenericID;
pub type VAMessageCallback =
    Option<unsafe extern "C" fn(user_context: *mut c_void, message: *const c_char)>;

pub const VA_STATUS_SUCCESS: VAStatus = 0;
pub const VA_INVALID_ID: VAGenericID = 0xffff_ffff;
pub const VA_INVALID_SURFACE: VASurfaceID = VA_INVALID_ID;

pub type VAProfile = c_int;
pub const VAProfileH264High: VAProfile = 7;

pub type VAEntrypoint = c_uint;
pub const VAEntrypointEncSlice: VAEntrypoint = 6;
pub const VAEntrypointEncSliceLP: VAEntrypoint = 8;

pub type VAConfigAttribType = c_uint;
pub const VAConfigAttribRTFormat: VAConfigAttribType = 0;
pub const VAConfigAttribRateControl: VAConfigAttribType = 5;
pub const VAConfigAttribEncPackedHeaders: VAConfigAttribType = 10;
pub const VAConfigAttribEncMaxRefFrames: VAConfigAttribType = 13;

/// An attribute's value where the driver does not support it.
pub const VA_ATTRIB_NOT_SUPPORTED: u32 = 0x8000_0000;
/// `VAConfigAttribRTFormat`'s.
pub const VA_RT_FORMAT_YUV420: u32 = 0x1;
/// `VAConfigAttribRateControl`'s.
pub const VA_RC_CQP: u32 = 0x10;
/// `VAConfigAttribEncPackedHeaders`'s.
pub const VA_ENC_PACKED_HEADER_SEQUENCE: u32 = 0x1;
pub const VA_ENC_PACKED_HEADER_PICTURE: u32 = 0x2;
pub const VA_ENC_PACKED_HEADER_SLICE: u32 = 0x4;

/// `vaCreateContext`'s flag.
pub const VA_PROGRESSIVE: c_int = 0x1;

pub type VABufferType = c_uint;
pub const VAEncCodedBufferType: VABufferType = 21;
pub const VAEncSequenceParameterBufferType: VABufferType = 22;
pub const VAEncPictureParameterBufferType: VABufferType = 23;
pub const VAEncSliceParameterBufferType: VABufferType = 24;
pub const VAEncPackedHeaderParameterBufferType: VABufferType = 25;
pub const VAEncPackedHeaderDataBufferType: VABufferType = 26;

/// `VAEncPackedHeaderTypeH264`, as `VAEncPackedHeaderParameterBuffer::type_`
/// takes it: `VAEncPackedHeaderSequence`, `Picture` and `Slice`.
pub const VAEncPackedHeaderH264_SPS: u32 = 1;
pub const VAEncPackedHeaderH264_PPS: u32 = 2;
pub const VAEncPackedHeaderH264_Slice: u32 = 3;

/// `VACodedBufferSegment::status`'s flags.
pub const VA_CODED_BUF_STATUS_SLICE_OVERFLOW_MASK: u32 = 0x200;
pub const VA_CODED_BUF_STATUS_BAD_BITSTREAM: u32 = 0x8000;

/// `VAPictureH264::flags`'.
pub const VA_PICTURE_H264_INVALID: u32 = 0x1;
pub const VA_PICTURE_H264_SHORT_TERM_REFERENCE: u32 = 0x8;

/// `VA_FOURCC('N', 'V', '1', '2')`.
pub const VA_FOURCC_NV12: u32 = u32::from_le_bytes(*b"NV12");
pub const VA_LSB_FIRST: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VAConfigAttrib {
    pub type_: VAConfigAttribType,
    pub value: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VAPictureH264 {
    pub picture_id: VASurfaceID,
    pub frame_idx: u32,
    pub flags: u32,
    pub TopFieldOrderCnt: i32,
    pub BottomFieldOrderCnt: i32,
    pub va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VAImageFormat {
    pub fourcc: u32,
    pub byte_order: u32,
    pub bits_per_pixel: u32,
    pub depth: u32,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub alpha_mask: u32,
    pub va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VAImage {
    pub image_id: VAImageID,
    pub format: VAImageFormat,
    pub buf: VABufferID,
    pub width: u16,
    pub height: u16,
    pub data_size: u32,
    pub num_planes: u32,
    pub pitches: [u32; 3],
    pub offsets: [u32; 3],
    pub num_palette_entries: i32,
    pub entry_bytes: i32,
    pub component_order: [i8; 4],
    pub va_reserved: [u32; 4],
}

/// One segment of a mapped coded buffer, `next` the following one or null.
#[repr(C)]
pub struct VACodedBufferSegment {
    pub size: u32,
    pub bit_offset: u32,
    pub status: u32,
    pub reserved: u32,
    pub buf: *mut c_void,
    pub next: *mut c_void,
    pub va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Default)]
pub struct VAEncPackedHeaderParameterBuffer {
    pub type_: u32,
    pub bit_length: u32,
    pub has_emulation_bytes: u8,
    pub va_reserved: [u32; 4],
}

/// A C union of a `uint32_t` bitfield struct and its `value`: the word, with a
/// setter for each field written. The fields are allotted from the least
/// significant bit, as GCC and Clang lay them out on a little-endian target.
macro_rules! bitfields {
    ($(#[$doc:meta])* $name:ident { $($setter:ident: $shift:literal, $width:literal;)* }) => {
        $(#[$doc])*
        #[repr(transparent)]
        #[derive(Clone, Copy, Default)]
        pub struct $name(pub u32);

        impl $name {
            $(
                pub fn $setter(&mut self, value: u32) {
                    let mask = ((1u32 << $width) - 1) << $shift;
                    self.0 = (self.0 & !mask) | ((value << $shift) & mask);
                }
            )*
        }
    };
}

const _: () = assert!(
    cfg!(target_endian = "little"),
    "the bitfield words are laid out for a little-endian target"
);

bitfields! {
    /// `seq_fields`.
    SeqFields {
        set_chroma_format_idc: 0, 2;
        set_frame_mbs_only_flag: 2, 1;
        set_direct_8x8_inference_flag: 5, 1;
        set_log2_max_frame_num_minus4: 6, 4;
        set_pic_order_cnt_type: 10, 2;
    }
}

bitfields! {
    /// `vui_fields`.
    VuiFields {
        set_bitstream_restriction_flag: 2, 1;
        set_log2_max_mv_length_horizontal: 3, 5;
        set_log2_max_mv_length_vertical: 8, 5;
        set_motion_vectors_over_pic_boundaries_flag: 15, 1;
    }
}

bitfields! {
    /// `pic_fields`.
    PicFields {
        set_idr_pic_flag: 0, 1;
        set_reference_pic_flag: 1, 2;
        set_entropy_coding_mode_flag: 3, 1;
        set_transform_8x8_mode_flag: 8, 1;
        set_deblocking_filter_control_present_flag: 9, 1;
    }
}

#[repr(C)]
pub struct VAEncSequenceParameterBufferH264 {
    pub seq_parameter_set_id: u8,
    pub level_idc: u8,
    pub intra_period: u32,
    pub intra_idr_period: u32,
    pub ip_period: u32,
    pub bits_per_second: u32,
    pub max_num_ref_frames: u32,
    pub picture_width_in_mbs: u16,
    pub picture_height_in_mbs: u16,
    pub seq_fields: SeqFields,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    pub num_ref_frames_in_pic_order_cnt_cycle: u8,
    pub offset_for_non_ref_pic: i32,
    pub offset_for_top_to_bottom_field: i32,
    pub offset_for_ref_frame: [i32; 256],
    pub frame_cropping_flag: u8,
    pub frame_crop_left_offset: u32,
    pub frame_crop_right_offset: u32,
    pub frame_crop_top_offset: u32,
    pub frame_crop_bottom_offset: u32,
    pub vui_parameters_present_flag: u8,
    pub vui_fields: VuiFields,
    pub aspect_ratio_idc: u8,
    pub sar_width: u32,
    pub sar_height: u32,
    pub num_units_in_tick: u32,
    pub time_scale: u32,
    pub va_reserved: [u32; 4],
}

#[repr(C)]
pub struct VAEncPictureParameterBufferH264 {
    pub CurrPic: VAPictureH264,
    pub ReferenceFrames: [VAPictureH264; 16],
    pub coded_buf: VABufferID,
    pub pic_parameter_set_id: u8,
    pub seq_parameter_set_id: u8,
    pub last_picture: u8,
    pub frame_num: u16,
    pub pic_init_qp: u8,
    pub num_ref_idx_l0_active_minus1: u8,
    pub num_ref_idx_l1_active_minus1: u8,
    pub chroma_qp_index_offset: i8,
    pub second_chroma_qp_index_offset: i8,
    pub pic_fields: PicFields,
    pub va_reserved: [u32; 4],
}

#[repr(C)]
pub struct VAEncSliceParameterBufferH264 {
    pub macroblock_address: u32,
    pub num_macroblocks: u32,
    pub macroblock_info: VABufferID,
    pub slice_type: u8,
    pub pic_parameter_set_id: u8,
    pub idr_pic_id: u16,
    pub pic_order_cnt_lsb: u16,
    pub delta_pic_order_cnt_bottom: i32,
    pub delta_pic_order_cnt: [i32; 2],
    pub direct_spatial_mv_pred_flag: u8,
    pub num_ref_idx_active_override_flag: u8,
    pub num_ref_idx_l0_active_minus1: u8,
    pub num_ref_idx_l1_active_minus1: u8,
    pub RefPicList0: [VAPictureH264; 32],
    pub RefPicList1: [VAPictureH264; 32],
    pub luma_log2_weight_denom: u8,
    pub chroma_log2_weight_denom: u8,
    pub luma_weight_l0_flag: u8,
    pub luma_weight_l0: [i16; 32],
    pub luma_offset_l0: [i16; 32],
    pub chroma_weight_l0_flag: u8,
    pub chroma_weight_l0: [[i16; 2]; 32],
    pub chroma_offset_l0: [[i16; 2]; 32],
    pub luma_weight_l1_flag: u8,
    pub luma_weight_l1: [i16; 32],
    pub luma_offset_l1: [i16; 32],
    pub chroma_weight_l1_flag: u8,
    pub chroma_weight_l1: [[i16; 2]; 32],
    pub chroma_offset_l1: [[i16; 2]; 32],
    pub cabac_init_idc: u8,
    pub slice_qp_delta: i8,
    pub disable_deblocking_filter_idc: u8,
    pub slice_alpha_c0_offset_div2: i8,
    pub slice_beta_offset_div2: i8,
    pub va_reserved: [u32; 4],
}

// The layouts, as `sizeof` and `offsetof` give them over libva 2.22's
// headers: every size, and the offsets that padding moves.
const _: () = {
    assert!(size_of::<VAConfigAttrib>() == 8);
    assert!(size_of::<VAPictureH264>() == 36);
    assert!(size_of::<VAImageFormat>() == 48);
    assert!(size_of::<VAImage>() == 120);
    assert!(offset_of!(VAImage, buf) == 52);
    assert!(offset_of!(VAImage, width) == 56);
    assert!(offset_of!(VAImage, pitches) == 68);
    assert!(offset_of!(VAImage, offsets) == 80);
    assert!(offset_of!(VAImage, component_order) == 100);
    assert!(offset_of!(VACodedBufferSegment, status) == 8);
    assert!(offset_of!(VACodedBufferSegment, buf) == 16);
    assert!(offset_of!(VACodedBufferSegment, next) == 16 + size_of::<usize>());
    assert!(size_of::<VACodedBufferSegment>() == 32 + 2 * size_of::<usize>());
    assert!(size_of::<VAEncPackedHeaderParameterBuffer>() == 28);
    assert!(offset_of!(VAEncPackedHeaderParameterBuffer, has_emulation_bytes) == 8);
    assert!(offset_of!(VAEncPackedHeaderParameterBuffer, va_reserved) == 12);
    assert!(size_of::<VAEncSequenceParameterBufferH264>() == 1132);
    assert!(offset_of!(VAEncSequenceParameterBufferH264, intra_period) == 4);
    assert!(offset_of!(VAEncSequenceParameterBufferH264, picture_width_in_mbs) == 24);
    assert!(offset_of!(VAEncSequenceParameterBufferH264, seq_fields) == 28);
    assert!(
        offset_of!(
            VAEncSequenceParameterBufferH264,
            num_ref_frames_in_pic_order_cnt_cycle
        ) == 34
    );
    assert!(offset_of!(VAEncSequenceParameterBufferH264, offset_for_non_ref_pic) == 36);
    assert!(offset_of!(VAEncSequenceParameterBufferH264, frame_cropping_flag) == 1068);
    assert!(offset_of!(VAEncSequenceParameterBufferH264, frame_crop_right_offset) == 1076);
    assert!(
        offset_of!(
            VAEncSequenceParameterBufferH264,
            vui_parameters_present_flag
        ) == 1088
    );
    assert!(offset_of!(VAEncSequenceParameterBufferH264, vui_fields) == 1092);
    assert!(offset_of!(VAEncSequenceParameterBufferH264, aspect_ratio_idc) == 1096);
    assert!(offset_of!(VAEncSequenceParameterBufferH264, sar_width) == 1100);
    assert!(offset_of!(VAEncSequenceParameterBufferH264, va_reserved) == 1116);
    assert!(size_of::<VAEncPictureParameterBufferH264>() == 648);
    assert!(offset_of!(VAEncPictureParameterBufferH264, coded_buf) == 612);
    assert!(offset_of!(VAEncPictureParameterBufferH264, frame_num) == 620);
    assert!(offset_of!(VAEncPictureParameterBufferH264, pic_init_qp) == 622);
    assert!(offset_of!(VAEncPictureParameterBufferH264, pic_fields) == 628);
    assert!(offset_of!(VAEncPictureParameterBufferH264, va_reserved) == 632);
    assert!(size_of::<VAEncSliceParameterBufferH264>() == 3140);
    assert!(offset_of!(VAEncSliceParameterBufferH264, slice_type) == 12);
    assert!(offset_of!(VAEncSliceParameterBufferH264, idr_pic_id) == 14);
    assert!(offset_of!(VAEncSliceParameterBufferH264, delta_pic_order_cnt_bottom) == 20);
    assert!(offset_of!(VAEncSliceParameterBufferH264, direct_spatial_mv_pred_flag) == 32);
    assert!(offset_of!(VAEncSliceParameterBufferH264, RefPicList0) == 36);
    assert!(offset_of!(VAEncSliceParameterBufferH264, RefPicList1) == 1188);
    assert!(offset_of!(VAEncSliceParameterBufferH264, luma_log2_weight_denom) == 2340);
    assert!(offset_of!(VAEncSliceParameterBufferH264, luma_weight_l0) == 2344);
    assert!(offset_of!(VAEncSliceParameterBufferH264, chroma_weight_l0_flag) == 2472);
    assert!(offset_of!(VAEncSliceParameterBufferH264, chroma_weight_l0) == 2474);
    assert!(offset_of!(VAEncSliceParameterBufferH264, luma_weight_l1_flag) == 2730);
    assert!(offset_of!(VAEncSliceParameterBufferH264, chroma_offset_l1) == 2990);
    assert!(offset_of!(VAEncSliceParameterBufferH264, cabac_init_idc) == 3118);
    assert!(offset_of!(VAEncSliceParameterBufferH264, slice_qp_delta) == 3119);
    assert!(offset_of!(VAEncSliceParameterBufferH264, va_reserved) == 3124);
};

#[link(name = "va")]
unsafe extern "C" {
    pub fn vaErrorStr(error_status: VAStatus) -> *const c_char;
    pub fn vaSetErrorCallback(
        dpy: VADisplay,
        callback: VAMessageCallback,
        user_context: *mut c_void,
    ) -> VAMessageCallback;
    pub fn vaSetInfoCallback(
        dpy: VADisplay,
        callback: VAMessageCallback,
        user_context: *mut c_void,
    ) -> VAMessageCallback;
    pub fn vaInitialize(
        dpy: VADisplay,
        major_version: *mut c_int,
        minor_version: *mut c_int,
    ) -> VAStatus;
    pub fn vaTerminate(dpy: VADisplay) -> VAStatus;
    pub fn vaGetConfigAttributes(
        dpy: VADisplay,
        profile: VAProfile,
        entrypoint: VAEntrypoint,
        attrib_list: *mut VAConfigAttrib,
        num_attribs: c_int,
    ) -> VAStatus;
    pub fn vaCreateConfig(
        dpy: VADisplay,
        profile: VAProfile,
        entrypoint: VAEntrypoint,
        attrib_list: *mut VAConfigAttrib,
        num_attribs: c_int,
        config_id: *mut VAConfigID,
    ) -> VAStatus;
    pub fn vaDestroyConfig(dpy: VADisplay, config_id: VAConfigID) -> VAStatus;
    /// `attrib_list` is a `VASurfaceAttrib *`, which the daemon passes null.
    pub fn vaCreateSurfaces(
        dpy: VADisplay,
        format: c_uint,
        width: c_uint,
        height: c_uint,
        surfaces: *mut VASurfaceID,
        num_surfaces: c_uint,
        attrib_list: *mut c_void,
        num_attribs: c_uint,
    ) -> VAStatus;
    pub fn vaDestroySurfaces(
        dpy: VADisplay,
        surfaces: *mut VASurfaceID,
        num_surfaces: c_int,
    ) -> VAStatus;
    pub fn vaCreateContext(
        dpy: VADisplay,
        config_id: VAConfigID,
        picture_width: c_int,
        picture_height: c_int,
        flag: c_int,
        render_targets: *mut VASurfaceID,
        num_render_targets: c_int,
        context: *mut VAContextID,
    ) -> VAStatus;
    pub fn vaDestroyContext(dpy: VADisplay, context: VAContextID) -> VAStatus;
    pub fn vaCreateBuffer(
        dpy: VADisplay,
        context: VAContextID,
        type_: VABufferType,
        size: c_uint,
        num_elements: c_uint,
        data: *mut c_void,
        buf_id: *mut VABufferID,
    ) -> VAStatus;
    pub fn vaMapBuffer(dpy: VADisplay, buf_id: VABufferID, pbuf: *mut *mut c_void) -> VAStatus;
    pub fn vaUnmapBuffer(dpy: VADisplay, buf_id: VABufferID) -> VAStatus;
    pub fn vaDestroyBuffer(dpy: VADisplay, buffer_id: VABufferID) -> VAStatus;
    pub fn vaBeginPicture(
        dpy: VADisplay,
        context: VAContextID,
        render_target: VASurfaceID,
    ) -> VAStatus;
    pub fn vaRenderPicture(
        dpy: VADisplay,
        context: VAContextID,
        buffers: *mut VABufferID,
        num_buffers: c_int,
    ) -> VAStatus;
    pub fn vaEndPicture(dpy: VADisplay, context: VAContextID) -> VAStatus;
    pub fn vaSyncSurface(dpy: VADisplay, render_target: VASurfaceID) -> VAStatus;
    pub fn vaCreateImage(
        dpy: VADisplay,
        format: *mut VAImageFormat,
        width: c_int,
        height: c_int,
        image: *mut VAImage,
    ) -> VAStatus;
    pub fn vaDestroyImage(dpy: VADisplay, image: VAImageID) -> VAStatus;
    pub fn vaPutImage(
        dpy: VADisplay,
        surface: VASurfaceID,
        image: VAImageID,
        src_x: c_int,
        src_y: c_int,
        src_width: c_uint,
        src_height: c_uint,
        dest_x: c_int,
        dest_y: c_int,
        dest_width: c_uint,
        dest_height: c_uint,
    ) -> VAStatus;
    pub fn vaDeriveImage(dpy: VADisplay, surface: VASurfaceID, image: *mut VAImage) -> VAStatus;
}

#[link(name = "va-drm")]
unsafe extern "C" {
    pub fn vaGetDisplayDRM(fd: c_int) -> VADisplay;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each word as the C compiler packs the same fields set to the same
    /// values over libva 2.22's headers.
    #[test]
    fn bitfields_pack_as_c_does() {
        let mut seq = SeqFields::default();
        seq.set_chroma_format_idc(1);
        seq.set_frame_mbs_only_flag(1);
        seq.set_direct_8x8_inference_flag(1);
        seq.set_log2_max_frame_num_minus4(15);
        seq.set_pic_order_cnt_type(2);
        assert_eq!(seq.0, 0xbe5);
        let mut vui = VuiFields::default();
        vui.set_bitstream_restriction_flag(1);
        vui.set_motion_vectors_over_pic_boundaries_flag(1);
        vui.set_log2_max_mv_length_horizontal(15);
        vui.set_log2_max_mv_length_vertical(15);
        assert_eq!(vui.0, 0x8f7c);
        let mut pic = PicFields::default();
        pic.set_idr_pic_flag(1);
        pic.set_reference_pic_flag(1);
        pic.set_entropy_coding_mode_flag(1);
        pic.set_transform_8x8_mode_flag(1);
        pic.set_deblocking_filter_control_present_flag(1);
        assert_eq!(pic.0, 0x30b);
    }

    #[test]
    fn nv12_fourcc() {
        assert_eq!(VA_FOURCC_NV12, 0x3231_564e);
    }
}
