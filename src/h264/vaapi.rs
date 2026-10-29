//! The host's GPU through libva itself: the encoder a VA-API driver offers
//! for H.264, handed every picture's parameters, its quantizer among them, and
//! the stream's headers as the daemon writes them ([`super::syntax`]).
//!
//! The quantizer is each slice's `slice_qp_delta` from the picture set's,
//! which a constant-quantizer encoder codes the slice at; libavcodec's
//! `h264_vaapi` fixes it when it opens, and its region-of-interest offset is
//! no substitute: on testdesktop2's shader encoder it left a few macroblocks
//! of every frame at the base, and on the low-power one it was ignored beyond
//! about eight either side. Through libva itself, every frame of a walk
//! jumping between 18 and 51 came out at the quantizer asked, on both, with no
//! keyframe but the first.
//!
//! **A stream.** One input surface, which each picture is copied into, two
//! reconstructed ones, the picture coded and the one it refers to, in turn,
//! and one buffer the frame is coded into; one frame in flight, synced before
//! it is read. The sequence parameter set goes ahead of every IDR, with the
//! picture one; frame numbers restart at each. A driver that takes packed
//! headers is handed the daemon's, and one that takes some writes the rest
//! from the same parameters.

use std::ffi::{CStr, c_char, c_void};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;

use cros_libva as va;
use log::{debug, info, warn};

use super::syntax::{self, Sequence, Slice};
use super::{Backend, Encoder, Planes};

/// Where render nodes are.
const DRI: &str = "/dev/dri";

/// The frames after an IDR at which the next is one whatever is asked: the
/// picture order count is twice the count, an `int` in every decoder.
const MAX_SINCE_IDR: u32 = 1 << 30;

/// Look for a working VA-API H.264 encoder: each render node in turn, the
/// low-power encoder ahead of the shader one, each tried on two frames.
/// `None` with nothing working, the reasons logged.
pub fn find() -> Option<Encoder> {
    let mut nodes: Vec<PathBuf> = match std::fs::read_dir(DRI) {
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
        for low_power in [true, false] {
            let encoder = Encoder::Vaapi { device: node.clone(), low_power };
            match encoder.try_frames() {
                Ok(()) => {
                    info!("hardware H.264: {} through VA-API, its {} encoder", node.display(), kind(low_power));
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

/// Which VA-API encoder: the fixed-function block or the shader one.
fn kind(low_power: bool) -> &'static str {
    if low_power { "low-power" } else { "shader" }
}

/// A VA display on a render node, initialized: one session's.
pub struct Display {
    display: va::VADisplay,
    /// The node, open for as long as the display is.
    _node: File,
}

// SAFETY: the display is libva's, which locks its own state, and its encoders
// are each one stream's, used from one thread at a time, the session's.
unsafe impl Send for Display {}
unsafe impl Sync for Display {}

impl Display {
    pub fn open(node: &Path) -> anyhow::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(node).map_err(|e| anyhow::anyhow!("opening {}: {e}", node.display()))?;
        // SAFETY: the descriptor stays open for the display's life, the
        // callbacks are plain functions, and a display whose initialization
        // fails is terminated, which frees it.
        unsafe {
            let display = va::vaGetDisplayDRM(file.as_raw_fd());
            anyhow::ensure!(!display.is_null(), "libva has no display on {}", node.display());
            va::vaSetInfoCallback(display, Some(libva_info), ptr::null_mut());
            va::vaSetErrorCallback(display, Some(libva_error), ptr::null_mut());
            let (mut major, mut minor) = (0, 0);
            if let Err(e) = status(va::vaInitialize(display, &mut major, &mut minor), "initializing VA-API") {
                va::vaTerminate(display);
                return Err(e.context(format!("VA-API on {}", node.display())));
            }
            Ok(Self { display, _node: file })
        }
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: the display is this value's, every stream on it gone first,
        // since each holds the display.
        unsafe { va::vaTerminate(self.display) };
    }
}

/// libva's messages, through the log: what it says it opened.
unsafe extern "C" fn libva_info(_: *mut c_void, message: *const c_char) {
    // SAFETY: libva hands a NUL-terminated message.
    debug!("libva: {}", unsafe { CStr::from_ptr(message) }.to_string_lossy().trim_end());
}

unsafe extern "C" fn libva_error(_: *mut c_void, message: *const c_char) {
    // SAFETY: as `libva_info`'s.
    warn!("libva: {}", unsafe { CStr::from_ptr(message) }.to_string_lossy().trim_end());
}

/// A libva status as a result.
fn status(code: va::VAStatus, what: &str) -> anyhow::Result<()> {
    if code == va::VA_STATUS_SUCCESS as va::VAStatus {
        return Ok(());
    }
    // SAFETY: libva's error strings are static and NUL-terminated.
    let message = unsafe { CStr::from_ptr(va::vaErrorStr(code)) }.to_string_lossy();
    anyhow::bail!("{what}: {message} ({code:#x})")
}

/// `VAEncPackedHeaderParameterBuffer`, which cros-libva's bindings leave out.
#[repr(C)]
struct PackedHeader {
    kind: u32,
    bit_length: u32,
    has_emulation_bytes: u8,
    va_reserved: [u32; 4],
}

/// How a picture reaches the input surface: the surface mapped itself, or,
/// where the driver will not map it, an image of it put onto it.
enum Upload {
    Derive,
    Put(va::VAImage),
}

/// The picture the next P frame refers to: the reconstructed surface it was
/// coded into, and its frame number and order count.
#[derive(Clone, Copy)]
struct Reference {
    surface: usize,
    frame_num: u32,
    poc: i32,
}

/// One encoder on a session's display, over a stream's coded picture.
pub struct Stream {
    display: Arc<Display>,
    config: va::VAConfigID,
    context: va::VAContextID,
    input: va::VASurfaceID,
    recon: [va::VASurfaceID; 2],
    coded: va::VABufferID,
    upload: Option<Upload>,
    sequence: Sequence,
    /// The headers the driver takes from the daemon: `VA_ENC_PACKED_HEADER_*`.
    packed: u32,
    reference: Option<Reference>,
    frame_num: u32,
    since_idr: u32,
    idr_pic_id: u32,
}

// SAFETY: as `Display`'s: the encoder is this value's, used from one thread at
// a time.
unsafe impl Send for Stream {}

impl Stream {
    /// An encoder over a `width`×`height` picture, both even: the low-power
    /// one where `low_power` says so.
    pub fn open(display: Arc<Display>, low_power: bool, (width, height): (usize, usize)) -> anyhow::Result<Self> {
        let mbs = (width.div_ceil(16) as u32, height.div_ceil(16) as u32);
        let mut stream = Self {
            display,
            config: va::VA_INVALID_ID,
            context: va::VA_INVALID_ID,
            input: va::VA_INVALID_SURFACE,
            recon: [va::VA_INVALID_SURFACE; 2],
            coded: va::VA_INVALID_ID,
            upload: None,
            sequence: Sequence { level_idc: syntax::level_idc(mbs), mbs, size: (width as u32, height as u32) },
            packed: 0,
            reference: None,
            frame_num: 0,
            since_idr: 0,
            idr_pic_id: 0,
        };
        let dpy = stream.display.display;
        let profile = va::VAProfile::VAProfileH264High;
        let entrypoint = if low_power { va::VAEntrypoint::VAEntrypointEncSliceLP } else { va::VAEntrypoint::VAEntrypointEncSlice };
        // SAFETY: every id made is the stream's, freed by `Drop` on any
        // failure after it; the attribute lists are their stated lengths.
        unsafe {
            let attrib = |type_| va::VAConfigAttrib { type_, value: 0 };
            let mut attribs = [
                attrib(va::VAConfigAttribType::VAConfigAttribRTFormat),
                attrib(va::VAConfigAttribType::VAConfigAttribRateControl),
                attrib(va::VAConfigAttribType::VAConfigAttribEncPackedHeaders),
                attrib(va::VAConfigAttribType::VAConfigAttribEncMaxRefFrames),
            ];
            status(va::vaGetConfigAttributes(dpy, profile, entrypoint, attribs.as_mut_ptr(), attribs.len() as i32), "asking for High profile's encoder")?;
            let [rt, rc, packed, refs] = attribs.map(|a| a.value);
            anyhow::ensure!(rt != va::VA_ATTRIB_NOT_SUPPORTED && rt & va::VA_RT_FORMAT_YUV420 != 0, "the encoder takes no 4:2:0");
            anyhow::ensure!(rc != va::VA_ATTRIB_NOT_SUPPORTED && rc & va::VA_RC_CQP != 0, "the encoder has no constant quantizer");
            anyhow::ensure!(refs == va::VA_ATTRIB_NOT_SUPPORTED || refs & 0xffff >= 1, "the encoder takes no reference picture");
            let mut config = vec![
                va::VAConfigAttrib { type_: va::VAConfigAttribType::VAConfigAttribRTFormat, value: va::VA_RT_FORMAT_YUV420 },
                va::VAConfigAttrib { type_: va::VAConfigAttribType::VAConfigAttribRateControl, value: va::VA_RC_CQP },
            ];
            if packed != va::VA_ATTRIB_NOT_SUPPORTED {
                stream.packed = packed & (va::VA_ENC_PACKED_HEADER_SEQUENCE | va::VA_ENC_PACKED_HEADER_PICTURE | va::VA_ENC_PACKED_HEADER_SLICE);
                config.push(va::VAConfigAttrib { type_: va::VAConfigAttribType::VAConfigAttribEncPackedHeaders, value: stream.packed });
            }
            status(va::vaCreateConfig(dpy, profile, entrypoint, config.as_mut_ptr(), config.len() as i32, &mut stream.config), "making the encoder's configuration")?;
            status(va::vaCreateSurfaces(dpy, va::VA_RT_FORMAT_YUV420, width as u32, height as u32, &mut stream.input, 1, ptr::null_mut(), 0), "making the input surface")?;
            let (w, h) = (mbs.0 * 16, mbs.1 * 16);
            status(va::vaCreateSurfaces(dpy, va::VA_RT_FORMAT_YUV420, w, h, stream.recon.as_mut_ptr(), 2, ptr::null_mut(), 0), "making the reconstructed surfaces")?;
            status(va::vaCreateContext(dpy, stream.config, w as i32, h as i32, va::VA_PROGRESSIVE as i32, stream.recon.as_mut_ptr(), 2, &mut stream.context), "making the encoder")?;
            // Raw 4:2:0 is 384 bytes a macroblock; the rest is headroom for
            // the headers and a coarse estimate.
            let size = mbs.0 * mbs.1 * 400 + 64 * 1024;
            status(va::vaCreateBuffer(dpy, stream.context, va::VABufferType::VAEncCodedBufferType, size, 1, ptr::null_mut(), &mut stream.coded), "making the coded buffer")?;
            stream.upload = Some(stream.upload_kind()?);
        }
        Ok(stream)
    }

    /// Whether the input surface maps as NV12 itself, or wants an image of it
    /// put onto it.
    unsafe fn upload_kind(&self) -> anyhow::Result<Upload> {
        let dpy = self.display.display;
        // SAFETY: the image made is destroyed here, or kept as the stream's.
        unsafe {
            let mut image: va::VAImage = std::mem::zeroed();
            if va::vaDeriveImage(dpy, self.input, &mut image) == va::VA_STATUS_SUCCESS as va::VAStatus {
                let nv12 = image.format.fourcc == va::VA_FOURCC_NV12;
                va::vaDestroyImage(dpy, image.image_id);
                if nv12 {
                    return Ok(Upload::Derive);
                }
            }
            let mut format: va::VAImageFormat = std::mem::zeroed();
            format.fourcc = va::VA_FOURCC_NV12;
            format.byte_order = va::VA_LSB_FIRST;
            format.bits_per_pixel = 12;
            let (w, h) = self.sequence.size;
            status(va::vaCreateImage(dpy, &mut format, w as i32, h as i32, &mut image), "making an NV12 image of the input surface")?;
            Ok(Upload::Put(image))
        }
    }

    /// Copy the planes onto the input surface.
    fn copy_in(&mut self, planes: &Planes) -> anyhow::Result<()> {
        let dpy = self.display.display;
        let (w, h) = planes.coded;
        // SAFETY: the image is mapped for the copy alone, which stays inside
        // its planes' pitches and the coded picture's rows; a derived image is
        // destroyed once copied into.
        unsafe {
            let mut derived: va::VAImage = std::mem::zeroed();
            let image = match self.upload.as_ref().expect("made with the stream") {
                Upload::Derive => {
                    status(va::vaDeriveImage(dpy, self.input, &mut derived), "mapping the input surface")?;
                    derived
                }
                Upload::Put(image) => *image,
            };
            let mut mapped: *mut c_void = ptr::null_mut();
            let copied = status(va::vaMapBuffer(dpy, image.buf, &mut mapped), "mapping the input picture").map(|()| {
                let base = mapped.cast::<u8>();
                for (plane, rows, source) in [(0, h, &planes.y), (1, h / 2, &planes.uv)] {
                    let (offset, pitch) = (image.offsets[plane] as usize, image.pitches[plane] as usize);
                    for row in 0..rows {
                        ptr::copy_nonoverlapping(source.as_ptr().add(row * planes.stride), base.add(offset + row * pitch), w);
                    }
                }
                va::vaUnmapBuffer(dpy, image.buf);
            });
            let put = match self.upload.as_ref().expect("made with the stream") {
                Upload::Derive => {
                    va::vaDestroyImage(dpy, derived.image_id);
                    Ok(())
                }
                Upload::Put(image) => copied.as_ref().map_or(Ok(()), |()| status(va::vaPutImage(dpy, self.input, image.image_id, 0, 0, w as u32, h as u32, 0, 0, w as u32, h as u32), "putting the picture onto the input surface")),
            };
            copied.and(put)
        }
    }
}

/// The parameter buffers of one picture, destroyed once it is coded.
struct Buffers {
    display: va::VADisplay,
    context: va::VAContextID,
    ids: Vec<va::VABufferID>,
}

impl Buffers {
    /// A buffer of `kind` holding `value`'s bytes.
    fn add<T>(&mut self, kind: va::VABufferType::Type, value: &T, what: &str) -> anyhow::Result<()> {
        self.bytes(kind, (value as *const T).cast(), size_of::<T>(), what)
    }

    fn bytes(&mut self, kind: va::VABufferType::Type, data: *const c_void, len: usize, what: &str) -> anyhow::Result<()> {
        let mut id = va::VA_INVALID_ID;
        // SAFETY: libva copies `len` bytes from `data`, which holds them.
        status(unsafe { va::vaCreateBuffer(self.display, self.context, kind, len as u32, 1, data.cast_mut(), &mut id) }, what)?;
        self.ids.push(id);
        Ok(())
    }

    /// A packed header, the daemon's bytes with their emulation prevention.
    fn packed(&mut self, kind: u32, header: &[u8], what: &str) -> anyhow::Result<()> {
        let parameters = PackedHeader { kind, bit_length: (header.len() * 8) as u32, has_emulation_bytes: 1, va_reserved: [0; 4] };
        self.add(va::VABufferType::VAEncPackedHeaderParameterBufferType, &parameters, what)?;
        self.bytes(va::VABufferType::VAEncPackedHeaderDataBufferType, header.as_ptr().cast(), header.len(), what)
    }
}

impl Drop for Buffers {
    fn drop(&mut self) {
        for &id in &self.ids {
            // SAFETY: each id is a buffer made here, on this context.
            unsafe { va::vaDestroyBuffer(self.display, id) };
        }
    }
}

/// A picture no reference is: every list entry not in use.
fn no_picture() -> va::VAPictureH264 {
    va::VAPictureH264 { picture_id: va::VA_INVALID_SURFACE, frame_idx: 0, flags: va::VA_PICTURE_H264_INVALID, TopFieldOrderCnt: 0, BottomFieldOrderCnt: 0, va_reserved: [0; 4] }
}

fn picture(surface: va::VASurfaceID, frame_num: u32, poc: i32, flags: u32) -> va::VAPictureH264 {
    va::VAPictureH264 { picture_id: surface, frame_idx: frame_num, flags, TopFieldOrderCnt: poc, BottomFieldOrderCnt: poc, va_reserved: [0; 4] }
}

impl Backend for Stream {
    fn frame(&mut self, planes: &Planes, keyframe: bool, qp: u8, out: &mut Vec<u8>) -> anyhow::Result<bool> {
        let idr = keyframe || self.reference.is_none() || self.since_idr >= MAX_SINCE_IDR;
        if idr {
            (self.frame_num, self.since_idr) = (0, 0);
        }
        self.copy_in(planes)?;
        let dpy = self.display.display;
        let current = self.reference.map_or(0, |r| 1 - r.surface);
        let poc = (2 * self.since_idr) as i32;
        let reference = self.reference.filter(|_| !idr);
        let slice = Slice { idr, frame_num: self.frame_num, idr_pic_id: self.idr_pic_id, qp };
        let sequence = self.sequence;
        let mut buffers = Buffers { display: dpy, context: self.context, ids: Vec::new() };
        // SAFETY: the parameter structures are plain data, zeroed and filled
        // here, and copied by libva into its buffers; the picture is begun,
        // rendered and ended on the stream's own context and surfaces, synced
        // before the coded buffer is read, and the coded buffer's segments
        // read within the sizes they say.
        unsafe {
            if idr {
                let mut seq: va::VAEncSequenceParameterBufferH264 = std::mem::zeroed();
                seq.level_idc = sequence.level_idc;
                seq.intra_period = MAX_SINCE_IDR;
                seq.intra_idr_period = MAX_SINCE_IDR;
                seq.ip_period = 1;
                seq.max_num_ref_frames = 1;
                (seq.picture_width_in_mbs, seq.picture_height_in_mbs) = (sequence.mbs.0 as u16, sequence.mbs.1 as u16);
                let fields = &mut seq.seq_fields.bits;
                fields.set_chroma_format_idc(1);
                fields.set_frame_mbs_only_flag(1);
                fields.set_direct_8x8_inference_flag(1);
                fields.set_log2_max_frame_num_minus4(syntax::LOG2_MAX_FRAME_NUM - 4);
                fields.set_pic_order_cnt_type(2);
                let (right, bottom) = sequence.crop();
                seq.frame_cropping_flag = u8::from(right != 0 || bottom != 0);
                (seq.frame_crop_right_offset, seq.frame_crop_bottom_offset) = (right, bottom);
                seq.vui_parameters_present_flag = 1;
                let vui = &mut seq.vui_fields.bits;
                vui.set_bitstream_restriction_flag(1);
                vui.set_motion_vectors_over_pic_boundaries_flag(1);
                vui.set_log2_max_mv_length_horizontal(15);
                vui.set_log2_max_mv_length_vertical(15);
                buffers.add(va::VABufferType::VAEncSequenceParameterBufferType, &seq, "making the sequence's parameters")?;
                if self.packed & va::VA_ENC_PACKED_HEADER_SEQUENCE != 0 {
                    buffers.packed(va::VAEncPackedHeaderTypeH264::VAEncPackedHeaderH264_SPS, &sequence.sps(), "making the sequence parameter set")?;
                }
            }

            let mut pic: va::VAEncPictureParameterBufferH264 = std::mem::zeroed();
            pic.CurrPic = picture(self.recon[current], self.frame_num, poc, 0);
            pic.ReferenceFrames = [no_picture(); 16];
            if let Some(r) = reference {
                pic.ReferenceFrames[0] = picture(self.recon[r.surface], r.frame_num, r.poc, va::VA_PICTURE_H264_SHORT_TERM_REFERENCE);
            }
            pic.coded_buf = self.coded;
            pic.frame_num = self.frame_num as u16;
            pic.pic_init_qp = syntax::PIC_INIT_QP;
            let fields = &mut pic.pic_fields.bits;
            fields.set_idr_pic_flag(u32::from(idr));
            fields.set_reference_pic_flag(1);
            fields.set_entropy_coding_mode_flag(1);
            fields.set_transform_8x8_mode_flag(1);
            fields.set_deblocking_filter_control_present_flag(1);
            buffers.add(va::VABufferType::VAEncPictureParameterBufferType, &pic, "making the picture's parameters")?;
            if idr && self.packed & va::VA_ENC_PACKED_HEADER_PICTURE != 0 {
                buffers.packed(va::VAEncPackedHeaderTypeH264::VAEncPackedHeaderH264_PPS, &syntax::pps(), "making the picture parameter set")?;
            }

            if self.packed & va::VA_ENC_PACKED_HEADER_SLICE != 0 {
                buffers.packed(va::VAEncPackedHeaderTypeH264::VAEncPackedHeaderH264_Slice, &slice.header(), "making the slice header")?;
            }
            let mut sp: va::VAEncSliceParameterBufferH264 = std::mem::zeroed();
            sp.num_macroblocks = sequence.mbs.0 * sequence.mbs.1;
            sp.macroblock_info = va::VA_INVALID_ID;
            sp.slice_type = slice.slice_type();
            sp.idr_pic_id = self.idr_pic_id as u16;
            sp.direct_spatial_mv_pred_flag = 1;
            sp.RefPicList0 = [no_picture(); 32];
            sp.RefPicList1 = [no_picture(); 32];
            if let Some(r) = reference {
                sp.RefPicList0[0] = pic.ReferenceFrames[0];
                debug_assert_eq!(r.surface, 1 - current);
            }
            sp.slice_qp_delta = (i32::from(qp) - i32::from(syntax::PIC_INIT_QP)) as i8;
            buffers.add(va::VABufferType::VAEncSliceParameterBufferType, &sp, "making the slice's parameters")?;

            status(va::vaBeginPicture(dpy, self.context, self.input), "beginning a picture")?;
            status(va::vaRenderPicture(dpy, self.context, buffers.ids.as_mut_ptr(), buffers.ids.len() as i32), "handing the encoder a picture")?;
            status(va::vaEndPicture(dpy, self.context), "coding a picture")?;
            drop(buffers);
            status(va::vaSyncSurface(dpy, self.input), "waiting for a picture to be coded")?;

            let mut mapped: *mut c_void = ptr::null_mut();
            status(va::vaMapBuffer(dpy, self.coded, &mut mapped), "mapping the coded frame")?;
            let mut segment = mapped.cast::<va::VACodedBufferSegment>().cast_const();
            let mut overflowed = false;
            while !segment.is_null() {
                let s = &*segment;
                overflowed |= s.status & va::VA_CODED_BUF_STATUS_SLICE_OVERFLOW_MASK != 0;
                out.extend_from_slice(std::slice::from_raw_parts(s.buf.cast::<u8>(), s.size as usize));
                segment = s.next.cast::<va::VACodedBufferSegment>().cast_const();
            }
            va::vaUnmapBuffer(dpy, self.coded);
            anyhow::ensure!(!overflowed, "the coded frame overflowed its buffer");
        }
        self.reference = Some(Reference { surface: current, frame_num: self.frame_num, poc });
        self.frame_num = (self.frame_num + 1) % (1 << syntax::LOG2_MAX_FRAME_NUM);
        self.since_idr += 1;
        if idr {
            self.idr_pic_id = (self.idr_pic_id + 1) % 65_536;
        }
        Ok(true)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let dpy = self.display.display;
        // SAFETY: each id is this stream's, made on its display, and the
        // invalid ones were never made.
        unsafe {
            if let Some(Upload::Put(image)) = self.upload.take() {
                va::vaDestroyImage(dpy, image.image_id);
            }
            if self.coded != va::VA_INVALID_ID {
                va::vaDestroyBuffer(dpy, self.coded);
            }
            if self.context != va::VA_INVALID_ID {
                va::vaDestroyContext(dpy, self.context);
            }
            for surface in self.recon.iter_mut().chain([&mut self.input]).filter(|s| **s != va::VA_INVALID_SURFACE) {
                va::vaDestroySurfaces(dpy, surface, 1);
            }
            if self.config != va::VA_INVALID_ID {
                va::vaDestroyConfig(dpy, self.config);
            }
        }
    }
}
