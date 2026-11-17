//! The host's GPU through libva itself: the encoder a VA-API driver offers
//! for H.264, handed every picture's parameters, its quantizer among them, and
//! the stream's headers as this crate writes them (`syntax`, beside this).
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
//! **Which encoder, and how hard it looks**
//! ([measured](../../../../docs/video-performance.md#the-hardware-beside-libx264)).
//! Where a driver has both of Intel's, the shader encoder is tried ahead of
//! the low-power, fixed-function one, and either is asked for the highest of
//! its quality levels where it has several: left at the driver's own level,
//! the low-power encoder's search stops 160 pixels from where a macroblock
//! was, short of a page scrolled, and at the highest it finds a slide of 256.
//! On a UHD 630 the level takes a third off the low-power encoder's bytes
//! over the desktop samples and the shader encoder at it takes over half,
//! the page as close or closer, for a few milliseconds more of the GPU a
//! frame.
//!
//! **A stream.** One input surface, which each picture is copied into, two
//! reconstructed ones, the picture coded and the one it refers to, in turn,
//! and one buffer the frame is coded into; one frame in flight, synced before
//! it is read. The sequence parameter set goes ahead of every IDR, with the
//! picture one; frame numbers restart at each. The driver takes the daemon's
//! sequence parameter set packed, or is no encoder, since VA's parameters
//! carry no colour description; of the picture and slice headers, it writes
//! from the same parameters any it does not take packed. A frame the driver
//! flags, overflowed or a bad bitstream, is an error and not the reference;
//! and the frame after one that failed, however it did, is an IDR. A picture
//! the driver was begun on and did not code leaves in the driver whatever of
//! it the driver took, which no call says or clears: the encoder's context is
//! made again before the next picture, on the same surfaces.

use std::ffi::{CStr, c_char, c_void};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use log::{debug, info, warn};

use super::libva as va;
use super::syntax::{self, Sequence, Slice};
use super::{Backend, Planes};

/// Where render nodes are.
const DRI: &str = "/dev/dri";

/// The quality level asked of a driver that has several: the highest, which
/// VA-API numbers 1, the higher numbers trading the picture and its bytes for
/// the encoder's speed and power.
const QUALITY_LEVEL: u32 = 1;

/// The frames after an IDR at which the next is one whatever is asked: the
/// picture order count is twice the count, an `int` in every decoder.
const MAX_SINCE_IDR: u32 = 1 << 30;

/// Look for a working VA-API H.264 encoder: each render node in turn, the
/// shader encoder ahead of the low-power one, each tried by `works`, which is
/// handed the node and whether the low-power encoder is the one to try. The
/// first that works, or `None` with nothing working, the reasons logged.
pub fn find(works: impl Fn(&Path, bool) -> anyhow::Result<()>) -> Option<(PathBuf, bool)> {
    let mut nodes: Vec<PathBuf> = match std::fs::read_dir(DRI) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("renderD"))
            })
            .collect(),
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
        for low_power in [false, true] {
            match works(&node, low_power) {
                Ok(()) => {
                    info!(
                        "hardware H.264: {} through VA-API, its {} encoder",
                        node.display(),
                        kind(low_power)
                    );
                    return Some((node, low_power));
                }
                Err(e) => debug!(
                    "the {} encoder on {}: {e:#}",
                    kind(low_power),
                    node.display()
                ),
            }
        }
        info!(
            "{}: VA-API opens no H.264 encoder there (run with RUST_LOG=debug for each attempt)",
            node.display()
        );
    }
    info!("no hardware H.264 encoder");
    None
}

/// Whether the encoder on `node` codes a stream whose quantizer moves frame
/// by frame, by what its frames weigh: three frames of noise on a small
/// picture, an IDR and two P frames, the first of those at a fine quantizer
/// and the second at a coarse one, which is to be under half the first's
/// bytes. What an agent tries its encoder by, having no decoder to read each
/// macroblock's quantizer back with as a daemon has; the daemon it serves
/// reads them back from the frames it is sent.
pub fn moves_its_quantizer(node: &Path, low_power: bool) -> anyhow::Result<()> {
    const SIDE: u16 = 96;
    let mut stream = Stream::open(
        Arc::new(Display::open(node)?),
        low_power,
        (SIDE.into(), SIDE.into()),
    )?;
    let mut planes = Planes::new(SIDE, SIDE);
    let mut state = 0x9E37_79B9u32;
    let mut sizes = Vec::new();
    for (n, (qp, nal)) in [(28, 5), (16, 1), (40, 1)].into_iter().enumerate() {
        // New noise each frame, so no frame is free.
        for sample in planes.y.iter_mut().chain(&mut planes.uv) {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *sample = state as u8;
        }
        let mut unit = Vec::new();
        anyhow::ensure!(
            stream.frame(&planes, false, qp, &mut unit)?,
            "the encoder made no frame {n}"
        );
        anyhow::ensure!(
            super::nal_types(&unit).contains(&nal),
            "frame {n} is not the kind asked"
        );
        sizes.push(unit.len());
    }
    anyhow::ensure!(
        sizes[2] * 2 < sizes[1],
        "a P frame at QP 40 weighs {} bytes and one at QP 16 {}: the quantizer does not move",
        sizes[2],
        sizes[1]
    );
    Ok(())
}

/// Whether this machine is said to have an encoder that works, by
/// `WLSHARE_REQUIRE_VAAPI` set in the environment. The tests of the
/// hardware, here and in the daemon, pass with a word on a machine [`find`]
/// finds no encoder on, which is most that build this; with it set they fail
/// there, saying what each encoder tried came to, so that on a machine with
/// an encoder one that stopped working is not passed over as one never
/// there.
pub fn required() -> bool {
    std::env::var_os("WLSHARE_REQUIRE_VAAPI").is_some_and(|set| !set.is_empty())
}

/// Which VA-API encoder: the fixed-function block or the shader one.
fn kind(low_power: bool) -> &'static str {
    if low_power { "low-power" } else { "shader" }
}

/// A VA display on a render node, initialized: one session's, or at an agent
/// one daemon's, whose streams are each a thread's.
pub struct Display {
    display: va::VADisplay,
    /// The node, open for as long as the display is.
    _node: File,
    /// Held for as long as anything of this display's is in libva's hands: a
    /// stream being opened, a frame being coded, a stream being closed. A
    /// session's streams are coded one after another by its own thread and
    /// never wait on it; an agent's are each a thread's, and a driver is not
    /// trusted to be called on one display from several at once.
    lock: Mutex<()>,
}

// SAFETY: the display is libva's, and every call made on it or on anything
// of its is made holding its lock, so from one thread at a time, whichever
// thread that is.
unsafe impl Send for Display {}
unsafe impl Sync for Display {}

impl Display {
    pub fn open(node: &Path) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(node)
            .map_err(|e| anyhow::anyhow!("opening {}: {e}", node.display()))?;
        // SAFETY: the descriptor stays open for the display's life, the
        // callbacks are plain functions, and a display whose initialization
        // fails is terminated, which frees it.
        unsafe {
            let display = va::vaGetDisplayDRM(file.as_raw_fd());
            anyhow::ensure!(
                !display.is_null(),
                "libva has no display on {}",
                node.display()
            );
            va::vaSetInfoCallback(display, Some(libva_info), ptr::null_mut());
            va::vaSetErrorCallback(display, Some(libva_error), ptr::null_mut());
            let (mut major, mut minor) = (0, 0);
            if let Err(e) = status(
                va::vaInitialize(display, &mut major, &mut minor),
                "initializing VA-API",
            ) {
                va::vaTerminate(display);
                return Err(e.context(format!("VA-API on {}", node.display())));
            }
            Ok(Self {
                display,
                _node: file,
                lock: Mutex::new(()),
            })
        }
    }

    /// The display's lock, held: libva is this thread's to call.
    fn held(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: the display is this value's, every stream on it gone first,
        // since each holds the display, and so nothing holds its lock.
        unsafe { va::vaTerminate(self.display) };
    }
}

/// libva's messages, through the log: what it says it opened.
unsafe extern "C" fn libva_info(_: *mut c_void, message: *const c_char) {
    // SAFETY: libva hands a NUL-terminated message.
    debug!(
        "libva: {}",
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .trim_end()
    );
}

unsafe extern "C" fn libva_error(_: *mut c_void, message: *const c_char) {
    // SAFETY: as `libva_info`'s.
    warn!(
        "libva: {}",
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .trim_end()
    );
}

/// A libva status as a result.
fn status(code: va::VAStatus, what: &str) -> anyhow::Result<()> {
    if code == va::VA_STATUS_SUCCESS {
        return Ok(());
    }
    // SAFETY: libva's error strings are static and NUL-terminated.
    let message = unsafe { CStr::from_ptr(va::vaErrorStr(code)) }.to_string_lossy();
    anyhow::bail!("{what}: {message} ({code:#x})")
}

/// How many bytes `rows` rows of `width` samples reach to, `apart` bytes
/// apart and the first `offset` in; `None` where that is past any count, as
/// numbers that are not a picture's can be.
fn reach(offset: usize, rows: usize, apart: usize, width: usize) -> Option<usize> {
    rows.checked_sub(1)?
        .checked_mul(apart)?
        .checked_add(offset)?
        .checked_add(width)
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
    /// The driver has quality levels, and is asked for [`QUALITY_LEVEL`].
    levels: bool,
    reference: Option<Reference>,
    frame_num: u32,
    since_idr: u32,
    idr_pic_id: u32,
    /// A picture was begun on the context and not seen through to its coded
    /// frame: the context is made again before another is begun.
    unfinished: bool,
    /// The next picture is begun and left there, as a driver that refuses
    /// what it is handed leaves one.
    #[cfg(test)]
    leave_begun: bool,
}

// SAFETY: as `Display`'s: the encoder is this value's, used from one thread at
// a time, which holds the display's lock for as long as it calls libva.
unsafe impl Send for Stream {}

impl Stream {
    /// An encoder over a `width`×`height` picture, both even: the low-power
    /// one where `low_power` says so.
    pub fn open(
        display: Arc<Display>,
        low_power: bool,
        (width, height): (usize, usize),
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            width > 0 && height > 0 && width % 2 == 0 && height % 2 == 0,
            "a {width}x{height} picture is not one of even sides"
        );
        let mbs = (width.div_ceil(16) as u32, height.div_ceil(16) as u32);
        let mut stream = Self {
            display,
            config: va::VA_INVALID_ID,
            context: va::VA_INVALID_ID,
            input: va::VA_INVALID_SURFACE,
            recon: [va::VA_INVALID_SURFACE; 2],
            coded: va::VA_INVALID_ID,
            upload: None,
            sequence: Sequence {
                level_idc: syntax::level_idc(mbs),
                mbs,
                size: (width as u32, height as u32),
            },
            packed: 0,
            levels: false,
            reference: None,
            frame_num: 0,
            since_idr: 0,
            idr_pic_id: 0,
            unfinished: false,
            #[cfg(test)]
            leave_begun: false,
        };
        // Held past whatever of the stream a failure here drops, which locks
        // for itself: the stream is declared first and so dropped last.
        let display = stream.display.clone();
        let _held = display.held();
        let dpy = stream.display.display;
        let profile = va::VAProfileH264High;
        let entrypoint = if low_power {
            va::VAEntrypointEncSliceLP
        } else {
            va::VAEntrypointEncSlice
        };
        // SAFETY: every id made is the stream's, freed by `Drop` on any
        // failure after it; the attribute lists are their stated lengths.
        unsafe {
            let attrib = |type_| va::VAConfigAttrib { type_, value: 0 };
            let mut attribs = [
                attrib(va::VAConfigAttribRTFormat),
                attrib(va::VAConfigAttribRateControl),
                attrib(va::VAConfigAttribEncPackedHeaders),
                attrib(va::VAConfigAttribEncMaxRefFrames),
                attrib(va::VAConfigAttribEncQualityRange),
            ];
            status(
                va::vaGetConfigAttributes(
                    dpy,
                    profile,
                    entrypoint,
                    attribs.as_mut_ptr(),
                    attribs.len() as i32,
                ),
                "asking for High profile's encoder",
            )?;
            let [rt, rc, packed, refs, levels] = attribs.map(|a| a.value);
            anyhow::ensure!(
                rt != va::VA_ATTRIB_NOT_SUPPORTED && rt & va::VA_RT_FORMAT_YUV420 != 0,
                "the encoder takes no 4:2:0"
            );
            anyhow::ensure!(
                rc != va::VA_ATTRIB_NOT_SUPPORTED && rc & va::VA_RC_CQP != 0,
                "the encoder has no constant quantizer"
            );
            anyhow::ensure!(
                refs == va::VA_ATTRIB_NOT_SUPPORTED || refs & 0xffff >= 1,
                "the encoder takes no reference picture"
            );
            // One level or none says the encoder has the one setting.
            stream.levels = levels != va::VA_ATTRIB_NOT_SUPPORTED && levels > 1;
            let mut config = vec![
                va::VAConfigAttrib {
                    type_: va::VAConfigAttribRTFormat,
                    value: va::VA_RT_FORMAT_YUV420,
                },
                va::VAConfigAttrib {
                    type_: va::VAConfigAttribRateControl,
                    value: va::VA_RC_CQP,
                },
            ];
            // The sequence parameter set is the daemon's own: VA's sequence
            // parameters have no colour description, so a driver's would
            // leave the decoder to guess BT.709 at HD for BT.601's pixels.
            anyhow::ensure!(
                packed != va::VA_ATTRIB_NOT_SUPPORTED
                    && packed & va::VA_ENC_PACKED_HEADER_SEQUENCE != 0,
                "the encoder takes no packed sequence parameter set"
            );
            stream.packed = packed
                & (va::VA_ENC_PACKED_HEADER_SEQUENCE
                    | va::VA_ENC_PACKED_HEADER_PICTURE
                    | va::VA_ENC_PACKED_HEADER_SLICE);
            config.push(va::VAConfigAttrib {
                type_: va::VAConfigAttribEncPackedHeaders,
                value: stream.packed,
            });
            status(
                va::vaCreateConfig(
                    dpy,
                    profile,
                    entrypoint,
                    config.as_mut_ptr(),
                    config.len() as i32,
                    &mut stream.config,
                ),
                "making the encoder's configuration",
            )?;
            status(
                va::vaCreateSurfaces(
                    dpy,
                    va::VA_RT_FORMAT_YUV420,
                    width as u32,
                    height as u32,
                    &mut stream.input,
                    1,
                    ptr::null_mut(),
                    0,
                ),
                "making the input surface",
            )?;
            let (w, h) = (mbs.0 * 16, mbs.1 * 16);
            status(
                va::vaCreateSurfaces(
                    dpy,
                    va::VA_RT_FORMAT_YUV420,
                    w,
                    h,
                    stream.recon.as_mut_ptr(),
                    2,
                    ptr::null_mut(),
                    0,
                ),
                "making the reconstructed surfaces",
            )?;
            stream.make_context()?;
            stream.upload = Some(stream.upload_kind()?);
        }
        Ok(stream)
    }

    /// The encoder's context, on the stream's configuration and its
    /// reconstructed surfaces, and the buffer its frames are coded into:
    /// made with the stream, and again after a picture that was begun and
    /// not coded.
    unsafe fn make_context(&mut self) -> anyhow::Result<()> {
        let dpy = self.display.display;
        let mbs = self.sequence.mbs;
        // SAFETY: the configuration and the surfaces are the stream's, and
        // each id made is the stream's, freed by `drop_context`.
        unsafe {
            status(
                va::vaCreateContext(
                    dpy,
                    self.config,
                    (mbs.0 * 16) as i32,
                    (mbs.1 * 16) as i32,
                    va::VA_PROGRESSIVE,
                    self.recon.as_mut_ptr(),
                    2,
                    &mut self.context,
                ),
                "making the encoder",
            )?;
            let size = super::most_coded((mbs.0 as usize * 16, mbs.1 as usize * 16)) as u32;
            status(
                va::vaCreateBuffer(
                    dpy,
                    self.context,
                    va::VAEncCodedBufferType,
                    size,
                    1,
                    ptr::null_mut(),
                    &mut self.coded,
                ),
                "making the coded buffer",
            )
        }
    }

    /// The coded buffer and the context, destroyed where they were made.
    unsafe fn drop_context(&mut self) {
        let dpy = self.display.display;
        // SAFETY: each id is this stream's, made on its display, and the
        // invalid ones were never made.
        unsafe {
            if self.coded != va::VA_INVALID_ID {
                va::vaDestroyBuffer(dpy, self.coded);
                self.coded = va::VA_INVALID_ID;
            }
            if self.context != va::VA_INVALID_ID {
                va::vaDestroyContext(dpy, self.context);
                self.context = va::VA_INVALID_ID;
            }
        }
    }

    /// Whether the input surface maps as NV12 itself, or wants an image of it
    /// put onto it.
    unsafe fn upload_kind(&self) -> anyhow::Result<Upload> {
        let dpy = self.display.display;
        // SAFETY: the image made is destroyed here, or kept as the stream's.
        unsafe {
            let mut image: va::VAImage = std::mem::zeroed();
            if va::vaDeriveImage(dpy, self.input, &mut image) == va::VA_STATUS_SUCCESS {
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
            status(
                va::vaCreateImage(dpy, &mut format, w as i32, h as i32, &mut image),
                "making an NV12 image of the input surface",
            )?;
            Ok(Upload::Put(image))
        }
    }

    /// Copy the planes onto the input surface: planes that are the stream's
    /// picture and hold its rows, or an error and nothing copied.
    fn copy_in(&mut self, planes: &Planes) -> anyhow::Result<()> {
        let dpy = self.display.display;
        let (w, h) = planes.coded;
        // The planes are whoever calls's, their every field open to it: held
        // to the stream's picture, and to holding the rows it has, before a
        // sample of them is read.
        let size = (self.sequence.size.0 as usize, self.sequence.size.1 as usize);
        anyhow::ensure!(
            planes.coded == size,
            "planes of a {w}x{h} picture were handed to a stream of {}x{}",
            size.0,
            size.1
        );
        let stride = planes.stride;
        let holds =
            |plane: &[u8], rows| reach(0, rows, stride, w).is_some_and(|end| end <= plane.len());
        anyhow::ensure!(
            stride >= w && holds(&planes.y, h) && holds(&planes.uv, h / 2),
            "planes of {} and {} bytes, their rows {stride} apart, do not hold a {w}x{h} picture",
            planes.y.len(),
            planes.uv.len()
        );
        // SAFETY: the image is mapped for the copy alone, which reads rows
        // the planes were just held to and writes within the bytes the image
        // says it has, each row held to them before the copy; a derived
        // image is destroyed once copied into.
        unsafe {
            let mut derived: va::VAImage = std::mem::zeroed();
            let image = match self.upload.as_ref().expect("made with the stream") {
                Upload::Derive => {
                    status(
                        va::vaDeriveImage(dpy, self.input, &mut derived),
                        "mapping the input surface",
                    )?;
                    derived
                }
                Upload::Put(image) => *image,
            };
            // Where each plane's rows go in the image, and that they end
            // inside it.
            let layout =
                [(0, h, &planes.y), (1, h / 2, &planes.uv)].map(|(plane, rows, source)| {
                    let (offset, pitch) =
                        (image.offsets[plane] as usize, image.pitches[plane] as usize);
                    let within = image.num_planes as usize > plane
                        && pitch >= w
                        && reach(offset, rows, pitch, w)
                            .is_some_and(|end| end <= image.data_size as usize);
                    (within, offset, pitch, rows, source)
                });
            let copied = (|| {
                anyhow::ensure!(
                    layout.iter().all(|(within, ..)| *within),
                    "the input surface's image, {} planes in {} bytes, does not hold a {w}x{h} picture",
                    image.num_planes,
                    image.data_size
                );
                let mut mapped: *mut c_void = ptr::null_mut();
                status(
                    va::vaMapBuffer(dpy, image.buf, &mut mapped),
                    "mapping the input picture",
                )?;
                let base = mapped.cast::<u8>();
                for (_, offset, pitch, rows, source) in layout {
                    for row in 0..rows {
                        ptr::copy_nonoverlapping(
                            source.as_ptr().add(row * stride),
                            base.add(offset + row * pitch),
                            w,
                        );
                    }
                }
                status(
                    va::vaUnmapBuffer(dpy, image.buf),
                    "unmapping the input picture",
                )
            })();
            let put = match self.upload.as_ref().expect("made with the stream") {
                Upload::Derive => {
                    va::vaDestroyImage(dpy, derived.image_id);
                    Ok(())
                }
                Upload::Put(image) => copied.as_ref().map_or(Ok(()), |()| {
                    status(
                        va::vaPutImage(
                            dpy,
                            self.input,
                            image.image_id,
                            0,
                            0,
                            w as u32,
                            h as u32,
                            0,
                            0,
                            w as u32,
                            h as u32,
                        ),
                        "putting the picture onto the input surface",
                    )
                }),
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
    fn add<T>(&mut self, kind: va::VABufferType, value: &T, what: &str) -> anyhow::Result<()> {
        self.bytes(kind, (value as *const T).cast(), size_of::<T>(), what)
    }

    fn bytes(
        &mut self,
        kind: va::VABufferType,
        data: *const c_void,
        len: usize,
        what: &str,
    ) -> anyhow::Result<()> {
        let mut id = va::VA_INVALID_ID;
        // SAFETY: libva copies `len` bytes from `data`, which holds them.
        status(
            unsafe {
                va::vaCreateBuffer(
                    self.display,
                    self.context,
                    kind,
                    len as u32,
                    1,
                    data.cast_mut(),
                    &mut id,
                )
            },
            what,
        )?;
        self.ids.push(id);
        Ok(())
    }

    /// A packed header, the daemon's bytes with their emulation prevention.
    fn packed(&mut self, kind: u32, header: &[u8], what: &str) -> anyhow::Result<()> {
        let parameters = va::VAEncPackedHeaderParameterBuffer {
            type_: kind,
            bit_length: (header.len() * 8) as u32,
            has_emulation_bytes: 1,
            ..Default::default()
        };
        self.add(va::VAEncPackedHeaderParameterBufferType, &parameters, what)?;
        self.bytes(
            va::VAEncPackedHeaderDataBufferType,
            header.as_ptr().cast(),
            header.len(),
            what,
        )
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
    va::VAPictureH264 {
        picture_id: va::VA_INVALID_SURFACE,
        frame_idx: 0,
        flags: va::VA_PICTURE_H264_INVALID,
        TopFieldOrderCnt: 0,
        BottomFieldOrderCnt: 0,
        va_reserved: [0; 4],
    }
}

fn picture(surface: va::VASurfaceID, frame_num: u32, poc: i32, flags: u32) -> va::VAPictureH264 {
    va::VAPictureH264 {
        picture_id: surface,
        frame_idx: frame_num,
        flags,
        TopFieldOrderCnt: poc,
        BottomFieldOrderCnt: poc,
        va_reserved: [0; 4],
    }
}

impl Backend for Stream {
    fn frame(
        &mut self,
        planes: &Planes,
        keyframe: bool,
        qp: u8,
        out: &mut Vec<u8>,
    ) -> anyhow::Result<bool> {
        let display = self.display.clone();
        let _held = display.held();
        // The reference is the stream's again once this frame is coded: a
        // frame that fails, at any step, leaves the stream none, so the
        // frame after it is an IDR whatever it is asked as. A keyframe that
        // failed is owed until one is coded, and no P frame is ever coded
        // from counters a failed one had started again.
        let held = self.reference.take();
        // And a picture the driver was begun on and did not code left its
        // context holding whatever of that picture it took, the packed
        // headers among it, for the next to be coded with: nothing of it is
        // kept, the context made again, on the same surfaces, which the IDR
        // that follows writes anew.
        if self.unfinished {
            // SAFETY: the context and its buffer are this stream's, and
            // nothing of theirs is in use: the picture's own buffers went
            // with the frame that failed.
            unsafe {
                self.drop_context();
                self.make_context()?;
            }
            self.unfinished = false;
        }
        let idr = keyframe || held.is_none() || self.since_idr >= MAX_SINCE_IDR;
        if idr {
            (self.frame_num, self.since_idr) = (0, 0);
        }
        self.copy_in(planes)?;
        let dpy = self.display.display;
        let current = held.map_or(0, |r| 1 - r.surface);
        let poc = (2 * self.since_idr) as i32;
        let reference = held.filter(|_| !idr);
        let slice = Slice {
            idr,
            frame_num: self.frame_num,
            idr_pic_id: self.idr_pic_id,
            qp,
        };
        let sequence = self.sequence;
        let mut buffers = Buffers {
            display: dpy,
            context: self.context,
            ids: Vec::new(),
        };
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
                (seq.picture_width_in_mbs, seq.picture_height_in_mbs) =
                    (sequence.mbs.0 as u16, sequence.mbs.1 as u16);
                let fields = &mut seq.seq_fields;
                fields.set_chroma_format_idc(1);
                fields.set_frame_mbs_only_flag(1);
                fields.set_direct_8x8_inference_flag(1);
                fields.set_log2_max_frame_num_minus4(syntax::LOG2_MAX_FRAME_NUM - 4);
                fields.set_pic_order_cnt_type(2);
                let (right, bottom) = sequence.crop();
                seq.frame_cropping_flag = u8::from(right != 0 || bottom != 0);
                (seq.frame_crop_right_offset, seq.frame_crop_bottom_offset) = (right, bottom);
                seq.vui_parameters_present_flag = 1;
                let vui = &mut seq.vui_fields;
                vui.set_bitstream_restriction_flag(1);
                vui.set_motion_vectors_over_pic_boundaries_flag(1);
                vui.set_log2_max_mv_length_horizontal(15);
                vui.set_log2_max_mv_length_vertical(15);
                buffers.add(
                    va::VAEncSequenceParameterBufferType,
                    &seq,
                    "making the sequence's parameters",
                )?;
                buffers.packed(
                    va::VAEncPackedHeaderH264_SPS,
                    &sequence.sps(),
                    "making the sequence parameter set",
                )?;
                // The level holds for the sequence it is sent with.
                if self.levels {
                    buffers.add(
                        va::VAEncMiscParameterBufferType,
                        &va::VAEncMiscParameterQualityLevel {
                            type_: va::VAEncMiscParameterTypeQualityLevel,
                            quality_level: QUALITY_LEVEL,
                            ..Default::default()
                        },
                        "asking for the quality level",
                    )?;
                }
            }

            let mut pic: va::VAEncPictureParameterBufferH264 = std::mem::zeroed();
            pic.CurrPic = picture(self.recon[current], self.frame_num, poc, 0);
            pic.ReferenceFrames = [no_picture(); 16];
            if let Some(r) = reference {
                pic.ReferenceFrames[0] = picture(
                    self.recon[r.surface],
                    r.frame_num,
                    r.poc,
                    va::VA_PICTURE_H264_SHORT_TERM_REFERENCE,
                );
            }
            pic.coded_buf = self.coded;
            pic.frame_num = self.frame_num as u16;
            pic.pic_init_qp = syntax::PIC_INIT_QP;
            let fields = &mut pic.pic_fields;
            fields.set_idr_pic_flag(u32::from(idr));
            fields.set_reference_pic_flag(1);
            fields.set_entropy_coding_mode_flag(1);
            fields.set_transform_8x8_mode_flag(1);
            fields.set_deblocking_filter_control_present_flag(1);
            buffers.add(
                va::VAEncPictureParameterBufferType,
                &pic,
                "making the picture's parameters",
            )?;
            if idr && self.packed & va::VA_ENC_PACKED_HEADER_PICTURE != 0 {
                buffers.packed(
                    va::VAEncPackedHeaderH264_PPS,
                    &syntax::pps(),
                    "making the picture parameter set",
                )?;
            }

            if self.packed & va::VA_ENC_PACKED_HEADER_SLICE != 0 {
                buffers.packed(
                    va::VAEncPackedHeaderH264_Slice,
                    &slice.header(),
                    "making the slice header",
                )?;
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
            buffers.add(
                va::VAEncSliceParameterBufferType,
                &sp,
                "making the slice's parameters",
            )?;

            // Begun, as far as is known, from the call on, and until the
            // coded frame has been read.
            self.unfinished = true;
            status(
                va::vaBeginPicture(dpy, self.context, self.input),
                "beginning a picture",
            )?;
            #[cfg(test)]
            anyhow::ensure!(
                !std::mem::take(&mut self.leave_begun),
                "a picture begun and left, as the test asked"
            );
            status(
                va::vaRenderPicture(
                    dpy,
                    self.context,
                    buffers.ids.as_mut_ptr(),
                    buffers.ids.len() as i32,
                ),
                "handing the encoder a picture",
            )?;
            status(va::vaEndPicture(dpy, self.context), "coding a picture")?;
            drop(buffers);
            status(
                va::vaSyncSurface(dpy, self.input),
                "waiting for a picture to be coded",
            )?;

            let mut mapped: *mut c_void = ptr::null_mut();
            status(
                va::vaMapBuffer(dpy, self.coded, &mut mapped),
                "mapping the coded frame",
            )?;
            let mut segment = mapped.cast::<va::VACodedBufferSegment>().cast_const();
            let (start, mut flags) = (out.len(), 0);
            while !segment.is_null() {
                let s = &*segment;
                flags |= s.status
                    & (va::VA_CODED_BUF_STATUS_SLICE_OVERFLOW_MASK
                        | va::VA_CODED_BUF_STATUS_BAD_BITSTREAM);
                out.extend_from_slice(std::slice::from_raw_parts(
                    s.buf.cast::<u8>(),
                    s.size as usize,
                ));
                segment = s.next.cast::<va::VACodedBufferSegment>().cast_const();
            }
            let unmapped = status(
                va::vaUnmapBuffer(dpy, self.coded),
                "unmapping the coded frame",
            );
            // A flagged frame, or one whose buffer the driver did not take
            // back, is neither sent nor made the reference, and the frame
            // after it is an IDR.
            if flags != 0 || unmapped.is_err() {
                out.truncate(start);
            }
            unmapped?;
            // Coded, whatever the driver says of the frame it made.
            self.unfinished = false;
            anyhow::ensure!(
                flags & va::VA_CODED_BUF_STATUS_SLICE_OVERFLOW_MASK == 0,
                "the coded frame overflowed its buffer"
            );
            anyhow::ensure!(
                flags & va::VA_CODED_BUF_STATUS_BAD_BITSTREAM == 0,
                "the encoder flagged the coded frame a bad bitstream"
            );
        }
        self.reference = Some(Reference {
            surface: current,
            frame_num: self.frame_num,
            poc,
        });
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
        let display = self.display.clone();
        let _held = display.held();
        let dpy = self.display.display;
        // SAFETY: each id is this stream's, made on its display, and the
        // invalid ones were never made.
        unsafe {
            if let Some(Upload::Put(image)) = self.upload.take() {
                va::vaDestroyImage(dpy, image.image_id);
            }
            self.drop_context();
            for surface in self
                .recon
                .iter_mut()
                .chain([&mut self.input])
                .filter(|s| **s != va::VA_INVALID_SURFACE)
            {
                va::vaDestroySurfaces(dpy, surface, 1);
            }
            if self.config != va::VA_INVALID_ID {
                va::vaDestroyConfig(dpy, self.config);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows are held to a plane by where their last sample is, and rows whose
    /// place is past any count are in no plane.
    #[test]
    fn rows_past_any_count_reach_nowhere() {
        assert_eq!(reach(0, 64, 128, 96), Some(63 * 128 + 96));
        assert_eq!(reach(4096, 1, 128, 96), Some(4096 + 96));
        assert_eq!(reach(0, 0, 128, 96), None);
        assert_eq!(reach(0, 64, usize::MAX / 32, 96), None);
        assert_eq!(reach(usize::MAX - 100, 2, 64, 96), None);
        assert_eq!(reach(0, 2, usize::MAX - 95, 96), None);
    }

    /// The encoder these tests run on: `None`, with a word, on a machine
    /// with none that works, which `unexercised` says the cost of; and a
    /// failure there instead where the machine is said to have one
    /// ([`required`]), with what each encoder tried came to.
    fn encoder(unexercised: &str) -> Option<(PathBuf, bool)> {
        let tried = std::cell::RefCell::new(Vec::new());
        let found = find(|node, low_power| {
            moves_its_quantizer(node, low_power).inspect_err(|e| {
                tried.borrow_mut().push(format!(
                    "the {} encoder on {}: {e:#}",
                    kind(low_power),
                    node.display()
                ));
            })
        });
        if found.is_none() {
            assert!(
                !required(),
                "WLSHARE_REQUIRE_VAAPI is set, and no VA-API H.264 encoder works on this machine: {:?}",
                tried.borrow()
            );
            eprintln!("no VA-API H.264 encoder on this machine; {unexercised}");
        }
        found
    }

    /// One display's streams, each opened, coded and closed by a thread of
    /// its own, as an agent's are for the daemon it codes for: every frame
    /// of every one is made, an IDR first and P frames after it. Passes
    /// with a word where this machine has no encoder.
    #[test]
    fn a_displays_streams_are_each_a_threads() {
        let Some((node, low_power)) = encoder("its display is not shared") else {
            return;
        };
        let display = Arc::new(Display::open(&node).unwrap());
        let threads: Vec<_> = (0..4u16)
            .map(|n| {
                let display = display.clone();
                std::thread::spawn(move || {
                    let (w, h) = (96 + 32 * n, 64 + 16 * n);
                    let mut stream =
                        Stream::open(display, low_power, (w.into(), h.into())).unwrap();
                    let mut planes = Planes::new(w, h);
                    let mut state = 0x9E37_79B9u32 + u32::from(n);
                    for frame in 0..40 {
                        for sample in planes.y.iter_mut().chain(&mut planes.uv) {
                            state ^= state << 13;
                            state ^= state >> 17;
                            state ^= state << 5;
                            *sample = state as u8;
                        }
                        let mut unit = Vec::new();
                        assert!(stream.frame(&planes, false, 28, &mut unit).unwrap());
                        assert_eq!(
                            crate::h264::nal_types(&unit).contains(&5),
                            frame == 0,
                            "stream {n}, frame {frame}"
                        );
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
    }

    /// Planes that are not the stream's picture, or do not hold the rows
    /// they say, fail the frame with nothing copied or coded; and the frame
    /// after a failed one is an IDR whatever it is asked as, so a keyframe
    /// that failed is still had. Passes with a word where this machine has
    /// no encoder.
    #[test]
    fn planes_of_another_picture_fail_the_frame_and_the_next_is_an_idr() {
        let Some((node, low_power)) = encoder("its stream is not exercised") else {
            return;
        };
        let display = Arc::new(Display::open(&node).unwrap());
        assert!(Stream::open(display.clone(), low_power, (96, 0)).is_err());
        assert!(Stream::open(display.clone(), low_power, (95, 64)).is_err());
        let mut stream = Stream::open(display, low_power, (96, 64)).unwrap();
        let planes = Planes::new(96, 64);
        let mut frame = |planes: &Planes, keyframe| {
            let mut unit = vec![0xAA];
            let made = stream.frame(planes, keyframe, 28, &mut unit);
            if made.is_err() {
                assert_eq!(unit, [0xAA], "nothing of a failed frame is kept");
            }
            made.map(|_| crate::h264::nal_types(&unit).contains(&5))
        };
        assert!(frame(&planes, false).unwrap(), "the first frame is an IDR");
        assert!(!frame(&planes, false).unwrap(), "and the next a P frame");

        let mut narrow = Planes::new(96, 64);
        narrow.stride = 32;
        let mut short = Planes::new(96, 64);
        short.uv.truncate(short.stride * 8);
        // Rows so far apart that where the last is wraps round to a place
        // the plane has.
        let mut apart = Planes::new(96, 64);
        apart.stride = usize::MAX / 63 + 2;
        for (wrong, keyframe) in [
            (Planes::new(64, 64), true),
            (Planes::new(96, 128), false),
            (narrow, false),
            (short, false),
            (apart, false),
        ] {
            assert!(frame(&wrong, keyframe).is_err());
            assert!(
                frame(&planes, false).unwrap(),
                "the frame after a failed one is an IDR"
            );
            assert!(!frame(&planes, false).unwrap(), "and the next a P frame");
        }
    }

    /// A picture begun and left, as a driver that refuses what it is handed
    /// leaves one, fails its frame with nothing of it kept; the frame after
    /// it is coded by a context made again, an IDR whatever it is asked as,
    /// and the stream goes on from that. Passes with a word where this
    /// machine has no encoder.
    #[test]
    fn a_picture_begun_and_left_has_its_context_made_again() {
        let Some((node, low_power)) = encoder("no context is made again") else {
            return;
        };
        let display = Arc::new(Display::open(&node).unwrap());
        let mut stream = Stream::open(display, low_power, (96, 64)).unwrap();
        let mut planes = Planes::new(96, 64);
        let mut state = 0x9E37_79B9u32;
        // New noise each frame, and whether the frame made of it is an IDR.
        let mut frame = |stream: &mut Stream| {
            for sample in planes.y.iter_mut().chain(&mut planes.uv) {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *sample = state as u8;
            }
            let mut unit = vec![0xAA];
            let made = stream.frame(&planes, false, 28, &mut unit);
            if made.is_err() {
                assert_eq!(unit, [0xAA], "nothing of a failed frame is kept");
            }
            made.map(|_| crate::h264::nal_types(&unit).contains(&5))
        };
        assert!(frame(&mut stream).unwrap(), "the first frame is an IDR");
        assert!(!frame(&mut stream).unwrap(), "and the next a P frame");
        for _ in 0..3 {
            stream.leave_begun = true;
            assert!(frame(&mut stream).is_err());
            assert!(stream.unfinished);
            assert!(
                frame(&mut stream).unwrap(),
                "the frame after a picture left begun is an IDR"
            );
            assert!(!stream.unfinished);
            for _ in 0..3 {
                assert!(!frame(&mut stream).unwrap(), "and P frames after it");
            }
        }
    }
}
