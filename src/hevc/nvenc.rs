//! NVIDIA's encoder, NVENC, through its own API: the driver's
//! `libnvidia-encode.so.1` and `libcuda.so.1`, loaded at run time, so a host
//! without the NVIDIA driver runs the daemon as well and has no HEVC.
//!
//! **A session** is a CUDA context on the first GPU, one a session of the
//! daemon's ([`Context`]), and every NVENC and CUDA call is made with it
//! current, pushed and popped around the call, since a session's frames are
//! coded on whichever of the runtime's threads its worker is on.
//!
//! **A stream** ([`Stream`]) is an encoder over one picture size, HEVC Range
//! Extensions at 4:4:4, 8 bits: P frames only, one frame in and one out, at a
//! constant quantizer with no adaptive quantization, one slice, and its
//! parameter sets ahead of every IDR, the stream's VUI saying BT.601 at studio
//! swing. Its input is one pitched CUDA allocation of the three planes, one
//! below another, registered with the encoder once and mapped for each frame;
//! a frame uploads to it the rows the damage since the frame before spans, and
//! the rest of it is the picture before. The quantizer is the encoder's
//! constant one, which a reconfiguration moves between any two frames with no
//! IDR: libavcodec's `hevc_nvenc` fixes it when it opens. The bitstream is
//! locked synchronously, the frame on the wire the moment it is coded.

use std::ffi::{CStr, c_void};
use std::ptr;
use std::sync::{Arc, OnceLock};

use super::sys;
use super::sys::wl_nvenc_versions as ver;

/// HEVC, as NVENC names it.
const CODEC_HEVC: sys::GUID = sys::GUID { Data1: 0x790c_dc88, Data2: 0x4522, Data3: 0x4d7b, Data4: [0x94, 0x25, 0xbd, 0xa9, 0x97, 0x5f, 0x76, 0x03] };
/// HEVC's Range Extensions profile, the one that codes 4:4:4.
const PROFILE_FREXT: sys::GUID = sys::GUID { Data1: 0x51ec_32b5, Data2: 0x1b4c, Data3: 0x453c, Data4: [0x9c, 0xbd, 0xb6, 0x16, 0xbd, 0x62, 0x13, 0x41] };
/// The fastest preset, P1: the encoder's least search, as libx264's
/// `ultrafast` is, for a stream of frames each at the quantizer it is asked.
const PRESET_P1: sys::GUID = sys::GUID { Data1: 0xfc0a_8d3e, Data2: 0x45f8, Data3: 0x4cf8, Data4: [0x80, 0xc7, 0x29, 0x88, 0x71, 0x59, 0x0e, 0xbf] };

/// The frame rate the encoder is told, for its level: twice wlshare's
/// default `max_fps`, as the H.264 stream's.
const FPS: u32 = 60;

/// The driver's two libraries, loaded once for the process, and the entry
/// points of each the daemon calls.
struct Driver {
    nvenc: sys::NV_ENCODE_API_FUNCTION_LIST,
    cu_device_get: sys::tcuDeviceGet,
    cu_ctx_create: sys::tcuCtxCreate_v2,
    cu_ctx_destroy: sys::tcuCtxDestroy_v2,
    cu_ctx_push: sys::tcuCtxPushCurrent_v2,
    cu_ctx_pop: sys::tcuCtxPopCurrent_v2,
    cu_mem_alloc_pitch: sys::tcuMemAllocPitch_v2,
    cu_mem_free: sys::tcuMemFree_v2,
    cu_memcpy_2d: sys::tcuMemcpy2D_v2,
}

// SAFETY: function pointers into the driver's libraries, which stay loaded for
// the process and are called from any thread.
unsafe impl Send for Driver {}
unsafe impl Sync for Driver {}

/// The driver, loaded the first time it is asked for: an error, said once and
/// kept, where the host has no NVIDIA driver or one older than the headers.
fn driver() -> anyhow::Result<&'static Driver> {
    static DRIVER: OnceLock<Result<Driver, String>> = OnceLock::new();
    // SAFETY: `load` opens the libraries and reads their symbols by the types
    // the headers give them.
    DRIVER.get_or_init(|| unsafe { load() }.map_err(|e| format!("{e:#}"))).as_ref().map_err(|e| anyhow::anyhow!("{e}"))
}

/// Open a shared library, kept open for the process.
unsafe fn open(name: &CStr) -> anyhow::Result<*mut c_void> {
    // SAFETY: a NUL-terminated name; the handle is never closed.
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        // SAFETY: dlerror's message is NUL-terminated, read before the next call.
        let why = unsafe { libc::dlerror() };
        let why = if why.is_null() { "not found".into() } else { unsafe { CStr::from_ptr(why) }.to_string_lossy() };
        anyhow::bail!("{}: {why}", name.to_string_lossy());
    }
    Ok(handle)
}

/// A symbol of `library`, as the function pointer type `T` the header gives
/// it: an `Option` of a function, `Some` here since a missing one is an error.
unsafe fn symbol<T: Copy>(library: *mut c_void, name: &CStr) -> anyhow::Result<T> {
    const { assert!(size_of::<T>() == size_of::<*mut c_void>()) };
    // SAFETY: an open handle and a NUL-terminated name.
    let address = unsafe { libc::dlsym(library, name.as_ptr()) };
    anyhow::ensure!(!address.is_null(), "the NVIDIA driver has no {}", name.to_string_lossy());
    // SAFETY: `T` is an `Option` of the function's pointer type, the size of a
    // pointer, and the address is that function's.
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&address) })
}

unsafe fn load() -> anyhow::Result<Driver> {
    // SAFETY: each symbol is read by the type the headers declare it with, and
    // called as the headers say.
    unsafe {
        let cuda = open(c"libcuda.so.1")?;
        let encode = open(c"libnvidia-encode.so.1")?;
        let cu_init: sys::tcuInit = symbol(cuda, c"cuInit")?;
        cu(cu_init.expect("read")(0), "initializing CUDA")?;

        type MaxVersion = Option<unsafe extern "C" fn(*mut u32) -> sys::NVENCSTATUS>;
        type CreateInstance = Option<unsafe extern "C" fn(*mut sys::NV_ENCODE_API_FUNCTION_LIST) -> sys::NVENCSTATUS>;
        let max_version: MaxVersion = symbol(encode, c"NvEncodeAPIGetMaxSupportedVersion")?;
        let create: CreateInstance = symbol(encode, c"NvEncodeAPICreateInstance")?;
        let mut max = 0;
        nvenc(max_version.expect("read")(&mut max), "asking the driver its NVENC version")?;
        let built = (sys::NVENCAPI_MAJOR_VERSION << 4) | sys::NVENCAPI_MINOR_VERSION;
        anyhow::ensure!(
            max >= built,
            "the NVIDIA driver speaks NVENC {}.{}, older than the {}.{} this daemon is built against",
            max >> 4,
            max & 0xf,
            sys::NVENCAPI_MAJOR_VERSION,
            sys::NVENCAPI_MINOR_VERSION
        );
        let mut list = sys::NV_ENCODE_API_FUNCTION_LIST { version: ver::WL_NV_ENCODE_API_FUNCTION_LIST_VER, ..Default::default() };
        nvenc(create.expect("read")(&mut list), "loading NVENC's entry points")?;
        Ok(Driver {
            nvenc: list,
            cu_device_get: symbol(cuda, c"cuDeviceGet")?,
            cu_ctx_create: symbol(cuda, c"cuCtxCreate_v2")?,
            cu_ctx_destroy: symbol(cuda, c"cuCtxDestroy_v2")?,
            cu_ctx_push: symbol(cuda, c"cuCtxPushCurrent_v2")?,
            cu_ctx_pop: symbol(cuda, c"cuCtxPopCurrent_v2")?,
            cu_mem_alloc_pitch: symbol(cuda, c"cuMemAllocPitch_v2")?,
            cu_mem_free: symbol(cuda, c"cuMemFree_v2")?,
            cu_memcpy_2d: symbol(cuda, c"cuMemcpy2D_v2")?,
        })
    }
}

/// A CUDA result as a result.
fn cu(code: sys::CUresult, what: &str) -> anyhow::Result<()> {
    anyhow::ensure!(code == sys::cudaError_enum::CUDA_SUCCESS, "{what}: CUDA error {code}");
    Ok(())
}

/// An NVENC status as a result, where there is no encoder to say why.
fn nvenc(code: sys::NVENCSTATUS, what: &str) -> anyhow::Result<()> {
    anyhow::ensure!(code == sys::_NVENCSTATUS::NV_ENC_SUCCESS, "{what}: NVENC status {code}");
    Ok(())
}

/// Call an entry point of NVENC's function list.
macro_rules! call {
    ($driver:expr, $name:ident($($arg:expr),* $(,)?)) => {
        ($driver.nvenc.$name.expect(concat!("NVENC has ", stringify!($name))))($($arg),*)
    };
}

/// A CUDA context on the first GPU: one session's, which its streams'
/// encoders and allocations are made in.
pub struct Context {
    driver: &'static Driver,
    context: sys::CUcontext,
}

// SAFETY: a CUDA context is used from any thread it is made current on, and
// every use here pushes it first and pops it after.
unsafe impl Send for Context {}
unsafe impl Sync for Context {}

impl Context {
    pub fn open() -> anyhow::Result<Self> {
        let driver = driver()?;
        // SAFETY: the driver's own calls, each checked; the context made
        // current by its creation is popped off this thread.
        unsafe {
            let mut device = 0;
            cu(driver.cu_device_get.expect("read")(&mut device, 0), "finding the first NVIDIA GPU")?;
            let mut context = ptr::null_mut();
            cu(driver.cu_ctx_create.expect("read")(&mut context, 0, device), "creating a CUDA context")?;
            let mut popped = ptr::null_mut();
            let left = driver.cu_ctx_pop.expect("read")(&mut popped);
            let made = Self { driver, context };
            cu(left, "leaving the CUDA context")?;
            Ok(made)
        }
    }

    /// The context, current on this thread until the guard goes.
    fn current(&self) -> anyhow::Result<Current<'_>> {
        // SAFETY: a context this value owns.
        cu(unsafe { self.driver.cu_ctx_push.expect("read")(self.context) }, "entering the CUDA context")?;
        Ok(Current(self))
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the context is this value's, every stream in it gone first,
        // since each holds it.
        unsafe { self.driver.cu_ctx_destroy.expect("read")(self.context) };
    }
}

/// A context made current, popped off the thread when this goes.
struct Current<'a>(&'a Context);

impl Drop for Current<'_> {
    fn drop(&mut self) {
        let mut popped = ptr::null_mut();
        // SAFETY: the context this guard pushed.
        unsafe { self.0.driver.cu_ctx_pop.expect("read")(&mut popped) };
    }
}

/// The three planes of a picture as the stream uploads them: `Y`, `Cb`, `Cr`,
/// each `stride` bytes a row and the coded picture's height.
pub struct Upload<'a> {
    pub planes: [&'a [u8]; 3],
    pub stride: usize,
    /// The rows to upload, each `(top, bottom)` with nothing overlapping, or
    /// every row for `None`.
    pub rows: Option<&'a [(usize, usize)]>,
}

/// One encoder over one coded picture size.
pub struct Stream {
    context: Arc<Context>,
    encoder: *mut c_void,
    bitstream: sys::NV_ENC_OUTPUT_PTR,
    input: sys::CUdeviceptr,
    pitch: usize,
    registered: sys::NV_ENC_REGISTERED_PTR,
    size: (u32, u32),
    /// What the encoder was initialized with, and is reconfigured from: boxed,
    /// since the one points into the other.
    config: Box<sys::NV_ENC_CONFIG>,
    init: Box<sys::NV_ENC_INITIALIZE_PARAMS>,
    qp: u8,
    frames: u64,
}

// SAFETY: the encoder and allocations are this value's, used from one thread
// at a time, each call with the context current.
unsafe impl Send for Stream {}

/// The smallest and largest pictures the encoder codes, and whether it codes
/// 4:4:4 at all.
pub struct Caps {
    pub min: (u32, u32),
    pub max: (u32, u32),
}

impl Stream {
    /// What the GPU's HEVC encoder codes: an error where it has none at 4:4:4.
    pub fn caps(context: &Arc<Context>) -> anyhow::Result<Caps> {
        let _current = context.current()?;
        let encoder = open_session(context)?;
        let caps = (|| {
            let driver = context.driver;
            let query = |cap: sys::NV_ENC_CAPS, what: &str| -> anyhow::Result<u32> {
                let mut param = sys::NV_ENC_CAPS_PARAM { version: ver::WL_NV_ENC_CAPS_PARAM_VER, capsToQuery: cap, ..Default::default() };
                let mut value = 0;
                // SAFETY: an open session, and the GUID and arguments the call takes.
                status(driver, encoder, unsafe { call!(driver, nvEncGetEncodeCaps(encoder, CODEC_HEVC, &mut param, &mut value)) }, what)?;
                Ok(value as u32)
            };
            use sys::_NV_ENC_CAPS as c;
            anyhow::ensure!(query(c::NV_ENC_CAPS_SUPPORT_YUV444_ENCODE, "asking whether HEVC codes 4:4:4")? != 0, "the GPU's HEVC encoder codes no 4:4:4");
            Ok(Caps {
                min: (query(c::NV_ENC_CAPS_WIDTH_MIN, "asking the smallest width")?, query(c::NV_ENC_CAPS_HEIGHT_MIN, "asking the smallest height")?),
                max: (query(c::NV_ENC_CAPS_WIDTH_MAX, "asking the largest width")?, query(c::NV_ENC_CAPS_HEIGHT_MAX, "asking the largest height")?),
            })
        })();
        // SAFETY: the session opened above, nothing else on it.
        unsafe { call!(context.driver, nvEncDestroyEncoder(encoder)) };
        caps
    }

    /// An encoder over a `width`×`height` picture, its first frame at `qp`.
    pub fn open(context: Arc<Context>, (width, height): (usize, usize), qp: u8) -> anyhow::Result<Self> {
        let current = context.current()?;
        let driver = context.driver;
        let mut stream = Self {
            context: context.clone(),
            encoder: ptr::null_mut(),
            bitstream: ptr::null_mut(),
            input: 0,
            pitch: 0,
            registered: ptr::null_mut(),
            size: (u32::try_from(width)?, u32::try_from(height)?),
            config: Box::default(),
            init: Box::default(),
            qp,
            frames: 0,
        };
        stream.encoder = open_session(&context)?;
        let (encoder, (w, h)) = (stream.encoder, stream.size);

        let mut preset = sys::NV_ENC_PRESET_CONFIG {
            version: ver::WL_NV_ENC_PRESET_CONFIG_VER,
            presetCfg: sys::NV_ENC_CONFIG { version: ver::WL_NV_ENC_CONFIG_VER, ..Default::default() },
            ..Default::default()
        };
        let tuning = sys::NV_ENC_TUNING_INFO::NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;
        // SAFETY: an open session, and a preset structure of the version asked.
        stream.check(unsafe { call!(driver, nvEncGetEncodePresetConfigEx(encoder, CODEC_HEVC, PRESET_P1, tuning, &mut preset)) }, "reading the P1 preset")?;
        let config = &mut *stream.config;
        *config = preset.presetCfg;
        config.version = ver::WL_NV_ENC_CONFIG_VER;
        config.profileGUID = PROFILE_FREXT;
        config.gopLength = sys::NVENC_INFINITE_GOPLENGTH;
        config.frameIntervalP = 1;
        let rc = &mut config.rcParams;
        rc.rateControlMode = sys::_NV_ENC_PARAMS_RC_MODE::NV_ENC_PARAMS_RC_CONSTQP;
        rc.constQP = constant(qp);
        rc.set_enableAQ(0);
        rc.set_enableTemporalAQ(0);
        rc.set_enableLookahead(0);
        rc.qpMapMode = sys::_NV_ENC_QP_MAP_MODE::NV_ENC_QP_MAP_DISABLED;
        // SAFETY: the codec's half of the union, HEVC's, which the preset filled.
        let hevc = unsafe { &mut config.encodeCodecConfig.hevcConfig };
        hevc.set_chromaFormatIDC(3);
        hevc.set_repeatSPSPPS(1);
        hevc.set_disableSPSPPS(0);
        hevc.set_outputAUD(0);
        hevc.set_enableIntraRefresh(0);
        hevc.set_enableLTR(0);
        hevc.idrPeriod = sys::NVENC_INFINITE_GOPLENGTH;
        hevc.sliceMode = 0;
        hevc.sliceModeData = 0;
        let vui = &mut hevc.hevcVUIParameters;
        vui.videoSignalTypePresentFlag = 1;
        vui.videoFormat = sys::_NV_ENC_VUI_VIDEO_FORMAT::NV_ENC_VUI_VIDEO_FORMAT_UNSPECIFIED;
        vui.videoFullRangeFlag = 0;
        vui.colourDescriptionPresentFlag = 1;
        vui.colourPrimaries = sys::_NV_ENC_VUI_COLOR_PRIMARIES::NV_ENC_VUI_COLOR_PRIMARIES_SMPTE170M;
        vui.transferCharacteristics = sys::_NV_ENC_VUI_TRANSFER_CHARACTERISTIC::NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SMPTE170M;
        vui.colourMatrix = sys::_NV_ENC_VUI_MATRIX_COEFFS::NV_ENC_VUI_MATRIX_COEFFS_SMPTE170M;

        *stream.init = sys::NV_ENC_INITIALIZE_PARAMS {
            version: ver::WL_NV_ENC_INITIALIZE_PARAMS_VER,
            encodeGUID: CODEC_HEVC,
            presetGUID: PRESET_P1,
            encodeWidth: w,
            encodeHeight: h,
            darWidth: w,
            darHeight: h,
            frameRateNum: FPS,
            frameRateDen: 1,
            enablePTD: 1,
            encodeConfig: &mut *stream.config,
            maxEncodeWidth: w,
            maxEncodeHeight: h,
            tuningInfo: tuning,
            bufferFormat: sys::_NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_YUV444,
            ..Default::default()
        };
        // SAFETY: an open session, and parameters pointing at the config, which
        // the stream keeps.
        let initialized = unsafe { call!(driver, nvEncInitializeEncoder(encoder, &mut *stream.init)) };
        stream.check(initialized, "initializing the HEVC encoder")?;

        let mut buffer = sys::NV_ENC_CREATE_BITSTREAM_BUFFER { version: ver::WL_NV_ENC_CREATE_BITSTREAM_BUFFER_VER, ..Default::default() };
        // SAFETY: an initialized encoder.
        stream.check(unsafe { call!(driver, nvEncCreateBitstreamBuffer(encoder, &mut buffer)) }, "making the bitstream buffer")?;
        stream.bitstream = buffer.bitstreamBuffer;

        let (mut input, mut pitch) = (0, 0);
        // SAFETY: the context is current; the allocation is the stream's.
        cu(unsafe { driver.cu_mem_alloc_pitch.expect("read")(&mut input, &mut pitch, width, height * 3, 16) }, "allocating the input planes")?;
        stream.input = input;
        stream.pitch = pitch;
        let mut register = sys::NV_ENC_REGISTER_RESOURCE {
            version: ver::WL_NV_ENC_REGISTER_RESOURCE_VER,
            resourceType: sys::_NV_ENC_INPUT_RESOURCE_TYPE::NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR,
            width: w,
            height: h,
            pitch: u32::try_from(pitch)?,
            resourceToRegister: input as *mut c_void,
            bufferFormat: sys::_NV_ENC_BUFFER_FORMAT::NV_ENC_BUFFER_FORMAT_YUV444,
            bufferUsage: sys::_NV_ENC_BUFFER_USAGE::NV_ENC_INPUT_IMAGE,
            ..Default::default()
        };
        // SAFETY: the allocation above, of the size and pitch said.
        stream.check(unsafe { call!(driver, nvEncRegisterResource(encoder, &mut register)) }, "registering the input planes")?;
        stream.registered = register.registeredResource;
        drop(current);
        Ok(stream)
    }

    /// Upload what `upload` says changed, code the picture at `qp`, as an IDR
    /// where `keyframe` says so, and append the access unit to `out`. Whether
    /// the encoder made a frame.
    pub fn frame(&mut self, upload: &Upload, keyframe: bool, qp: u8, out: &mut Vec<u8>) -> anyhow::Result<bool> {
        let context = self.context.clone();
        let _current = context.current()?;
        let driver = context.driver;
        self.upload(upload)?;
        if qp != self.qp {
            self.config.rcParams.constQP = constant(qp);
            let mut params = sys::NV_ENC_RECONFIGURE_PARAMS {
                version: ver::WL_NV_ENC_RECONFIGURE_PARAMS_VER,
                reInitEncodeParams: *self.init,
                ..Default::default()
            };
            params.set_resetEncoder(0);
            params.set_forceIDR(0);
            // SAFETY: an initialized encoder, reconfigured from what it was
            // initialized with, the quantizer moved.
            self.check(unsafe { call!(driver, nvEncReconfigureEncoder(self.encoder, &mut params)) }, "moving the quantizer")?;
            self.qp = qp;
        }

        let mut map = sys::NV_ENC_MAP_INPUT_RESOURCE { version: ver::WL_NV_ENC_MAP_INPUT_RESOURCE_VER, registeredResource: self.registered, ..Default::default() };
        // SAFETY: the registered input.
        self.check(unsafe { call!(driver, nvEncMapInputResource(self.encoder, &mut map)) }, "mapping the input planes")?;
        let coded = self.encode(map.mappedResource, map.mappedBufferFmt, keyframe, out);
        // SAFETY: the resource mapped above, unmapped whatever the frame did.
        let unmapped = unsafe { call!(driver, nvEncUnmapInputResource(self.encoder, map.mappedResource)) };
        let coded = coded?;
        self.check(unmapped, "unmapping the input planes")?;
        Ok(coded)
    }

    fn encode(&mut self, input: sys::NV_ENC_INPUT_PTR, format: sys::NV_ENC_BUFFER_FORMAT, keyframe: bool, out: &mut Vec<u8>) -> anyhow::Result<bool> {
        let driver = self.context.driver;
        let flags = if keyframe { sys::_NV_ENC_PIC_FLAGS::NV_ENC_PIC_FLAG_FORCEIDR | sys::_NV_ENC_PIC_FLAGS::NV_ENC_PIC_FLAG_OUTPUT_SPSPPS } else { 0 };
        let mut picture = sys::NV_ENC_PIC_PARAMS {
            version: ver::WL_NV_ENC_PIC_PARAMS_VER,
            inputWidth: self.size.0,
            inputHeight: self.size.1,
            inputPitch: u32::try_from(self.pitch)?,
            encodePicFlags: flags,
            inputTimeStamp: self.frames,
            inputBuffer: input,
            outputBitstream: self.bitstream,
            bufferFmt: format,
            pictureStruct: sys::_NV_ENC_PIC_STRUCT::NV_ENC_PIC_STRUCT_FRAME,
            ..Default::default()
        };
        // SAFETY: the mapped input and the stream's bitstream buffer.
        self.check(unsafe { call!(driver, nvEncEncodePicture(self.encoder, &mut picture)) }, "coding a frame")?;
        self.frames += 1;
        let mut lock = sys::NV_ENC_LOCK_BITSTREAM { version: ver::WL_NV_ENC_LOCK_BITSTREAM_VER, outputBitstream: self.bitstream, ..Default::default() };
        lock.set_doNotWait(0);
        // SAFETY: the buffer the frame was just coded into, waited for.
        self.check(unsafe { call!(driver, nvEncLockBitstream(self.encoder, &mut lock)) }, "reading the coded frame")?;
        let len = lock.bitstreamSizeInBytes as usize;
        // SAFETY: the locked buffer holds `len` bytes of the frame until it is
        // unlocked.
        out.extend_from_slice(unsafe { std::slice::from_raw_parts(lock.bitstreamBufferPtr as *const u8, len) });
        // SAFETY: the buffer locked above.
        self.check(unsafe { call!(driver, nvEncUnlockBitstream(self.encoder, self.bitstream)) }, "letting go of the coded frame")?;
        Ok(len > 0)
    }

    /// Copy the rows `upload` names of each plane into the input planes.
    fn upload(&self, upload: &Upload) -> anyhow::Result<()> {
        let height = self.size.1 as usize;
        let whole = [(0, height)];
        let rows = upload.rows.unwrap_or(&whole);
        for (plane, data) in upload.planes.iter().enumerate() {
            anyhow::ensure!(data.len() >= upload.stride * height, "plane {plane} is short of the picture");
            for &(top, bottom) in rows {
                if top >= bottom {
                    continue;
                }
                anyhow::ensure!(bottom <= height, "rows {top}..{bottom} past a picture of {height}");
                let copy = sys::CUDA_MEMCPY2D {
                    srcMemoryType: sys::CUmemorytype_enum_CU_MEMORYTYPE_HOST,
                    srcHost: data[top * upload.stride..].as_ptr().cast(),
                    srcPitch: upload.stride,
                    dstMemoryType: sys::CUmemorytype_enum_CU_MEMORYTYPE_DEVICE,
                    dstDevice: self.input + ((plane * height + top) * self.pitch) as sys::CUdeviceptr,
                    dstPitch: self.pitch,
                    WidthInBytes: self.size.0 as usize,
                    Height: bottom - top,
                    ..Default::default()
                };
                // SAFETY: rows inside the host plane, checked above, onto rows of
                // the stream's allocation, with the context current.
                cu(unsafe { self.context.driver.cu_memcpy_2d.expect("read")(&copy) }, "uploading the picture")?;
            }
        }
        Ok(())
    }

    /// An NVENC status as a result, with the encoder's own word on why.
    fn check(&self, code: sys::NVENCSTATUS, what: &str) -> anyhow::Result<()> {
        status(self.context.driver, self.encoder, code, what)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let Ok(_current) = self.context.current() else { return };
        let driver = self.context.driver;
        // SAFETY: each is this stream's, released before the encoder it is on.
        unsafe {
            if !self.registered.is_null() {
                call!(driver, nvEncUnregisterResource(self.encoder, self.registered));
            }
            if !self.bitstream.is_null() {
                call!(driver, nvEncDestroyBitstreamBuffer(self.encoder, self.bitstream));
            }
            if !self.encoder.is_null() {
                call!(driver, nvEncDestroyEncoder(self.encoder));
            }
            if self.input != 0 {
                driver.cu_mem_free.expect("read")(self.input);
            }
        }
    }
}

/// An encode session in the context, which is current.
fn open_session(context: &Context) -> anyhow::Result<*mut c_void> {
    let mut params = sys::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS {
        version: ver::WL_NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER,
        deviceType: sys::_NV_ENC_DEVICE_TYPE::NV_ENC_DEVICE_TYPE_CUDA,
        device: context.context.cast(),
        apiVersion: ver::WL_NVENCAPI_VERSION,
        ..Default::default()
    };
    let mut encoder = ptr::null_mut();
    // SAFETY: the context, current, as the device.
    nvenc(unsafe { call!(context.driver, nvEncOpenEncodeSessionEx(&mut params, &mut encoder)) }, "opening an NVENC session")?;
    Ok(encoder)
}

/// An NVENC status as a result, with the encoder's last error where there is
/// one.
fn status(driver: &Driver, encoder: *mut c_void, code: sys::NVENCSTATUS, what: &str) -> anyhow::Result<()> {
    if code == sys::_NVENCSTATUS::NV_ENC_SUCCESS {
        return Ok(());
    }
    let why = match driver.nvenc.nvEncGetLastErrorString {
        // SAFETY: the encoder's own string, NUL-terminated and valid until its
        // next call.
        Some(last) if !encoder.is_null() => unsafe { CStr::from_ptr(last(encoder)) }.to_string_lossy().into_owned(),
        _ => String::new(),
    };
    anyhow::bail!("{what}: NVENC status {code} {why}")
}

/// One quantizer for every kind of frame.
fn constant(qp: u8) -> sys::NV_ENC_QP {
    let qp = u32::from(qp);
    sys::NV_ENC_QP { qpInterP: qp, qpInterB: qp, qpIntra: qp }
}
