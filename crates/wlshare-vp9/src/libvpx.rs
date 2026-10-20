//! libvpx: its encoder as a desktop is coded with it ([`Encoder`]), and its
//! decoder ([`Decoder`]), which gives the planes back for every test here and
//! in `wlshare-rfb`. The one place libvpx is spoken to.

use std::os::raw::{c_int, c_uint, c_ulong};

use vpx_sys as vpx;

use crate::{Chroma, Error, MAX_THREADS, Picture, QUALITY_MAX, QUALITY_MIN, Rect, Speed, fits, loop_filtered, quality_to_q, tile_columns_log2};

/// libvpx's image format for a picture at `chroma`.
fn image_format(chroma: Chroma) -> vpx::vpx_img_fmt_t {
    match chroma {
        Chroma::Yuv444 => vpx::vpx_img_fmt_VPX_IMG_FMT_I444,
        Chroma::Yuv420 => vpx::vpx_img_fmt_VPX_IMG_FMT_I420,
    }
}

/// libvpx's `cpu_used` for a speed, its speed setting, 0–9, higher being
/// faster and worse. 7 is inside the 5–8 band libvpx's own live-encoding
/// guidance names, and where the one other real-time desktop encoder to
/// consult sits, measured rather than inherited; 9 is the fastest its
/// real-time VP9 has.
fn cpu_used(speed: Speed) -> c_int {
    match speed {
        Speed::Usual => 7,
        Speed::Fastest => 9,
    }
}

/// The side of a block of libvpx's active map, in pixels.
const ACTIVE_BLOCK: usize = 16;

/// Turn a libvpx return code into an [`Error`] naming the call.
fn check(err: vpx::vpx_codec_err_t, call: &'static str) -> Result<(), Error> {
    if err == vpx::vpx_codec_err_t_VPX_CODEC_OK {
        return Ok(());
    }
    // SAFETY: `vpx_codec_err_to_string` takes a code, any code, and returns a
    // pointer to a static string compiled into the archive.
    let detail = unsafe { std::ffi::CStr::from_ptr(vpx::vpx_codec_err_to_string(err)) };
    Err(Error::Codec { call, detail: detail.to_string_lossy().into_owned() })
}

/// One VP9 stream at one picture size: a libvpx encoder whose quantizer is
/// pinned to the dial. A picture of another size needs
/// another encoder, whose first frame is a keyframe by construction. Within one
/// encoder, every inter frame is expressed as a change from earlier frames,
/// which is what makes an inter-frame stream mean anything.
pub struct Encoder {
    /// Boxed so that its address never changes: libvpx is handed a pointer to
    /// it at init and every call after.
    ctx: Box<vpx::vpx_codec_ctx_t>,
    /// The configuration libvpx is running on, kept so [`Self::follow`]
    /// can hand back the same struct with two fields changed rather than
    /// rebuild one and risk disagreeing with the encoder about a field it never
    /// meant to touch.
    cfg: vpx::vpx_codec_enc_cfg_t,
    /// An image that *borrows* a picture's planes. Built once, its pointers
    /// replaced on every frame. Never freed: `vpx_img_wrap` allocated nothing.
    img: vpx::vpx_image_t,
    size: (u16, u16),
    chroma: Chroma,
    /// The dial the next frame is coded at, which [`Self::set_quality`] moves.
    quality: u8,
    /// The speed the next frame is searched at, which `set_speed` moves.
    speed: Speed,
    /// The dial and the speed libvpx is running at, which a frame brings to
    /// the two above before it is coded ([`Self::follow`]).
    running: (u8, Speed),
    /// Where timestamps are measured from. Real elapsed time rather than a
    /// frame counter, so a pts is a millisecond on the timebase set below: the
    /// desktop decides when a frame happens, and a counter would tell the
    /// encoder they all arrived on schedule.
    started: std::time::Instant,
    /// The last timestamp given, which the next must pass.
    pts: i64,
    /// libvpx's active map, a byte per 16×16 block in rows: which blocks the
    /// frame being encoded may code, the others being skipped as they stand.
    /// Kept so a frame does not allocate one.
    active: Vec<u8>,
    /// The encoder is holding a map, which it keeps until told otherwise.
    mapped: bool,
    /// The dial each of the map's blocks was last coded at.
    coded_at: Vec<u8>,
    /// A frame has come out, so the next is not the stream's first.
    begun: bool,
}

impl Encoder {
    /// An encoder for a `width`×`height` picture at `chroma`, starting at
    /// `quality` (1–100, clamped), coded by `threads` threads. The thread count
    /// is the caller's: how many cores a machine can spare is a question about
    /// what else it runs. It also bounds the tile columns, and so decides with
    /// the width whether the stream is loop filtered: from 2048 wide on four
    /// threads or more it is not.
    pub fn new(width: u16, height: u16, quality: u8, chroma: Chroma, threads: usize) -> Result<Self, Error> {
        if width == 0 || height == 0 {
            return Err(Error::Empty(width, height));
        }
        if !(1..=MAX_THREADS).contains(&threads) {
            return Err(Error::Threads(threads));
        }
        let quality = quality.clamp(QUALITY_MIN, QUALITY_MAX);
        let q = quality_to_q(quality);

        // SAFETY: takes nothing and returns a pointer to a static interface
        // descriptor compiled into the archive.
        let iface = unsafe { vpx::vpx_codec_vp9_cx() };
        if iface.is_null() {
            return Err(Error::Codec { call: "vp9_cx", detail: "this libvpx has no VP9 encoder".to_owned() });
        }
        // SAFETY: zeroed before libvpx is given a pointer to it, and
        // `config_default` writes through that pointer rather than reading it. A
        // failure is returned before anything reads a half-initialised config.
        let mut cfg: vpx::vpx_codec_enc_cfg_t = unsafe { std::mem::zeroed() };
        check(unsafe { vpx::vpx_codec_enc_config_default(iface, &mut cfg, 0) }, "config_default")?;

        cfg.g_w = u32::from(width);
        cfg.g_h = u32::from(height);
        // The chroma's profile at eight bits. It has to match the image format
        // wrapped below, and `codec_string` tells a decoder the same number.
        cfg.g_profile = u32::from(chroma.profile());
        cfg.g_threads = threads as u32;
        // Milliseconds, so a pts is elapsed wall-clock rather than a frame index.
        cfg.g_timebase.num = 1;
        cfg.g_timebase.den = 1000;
        // **0, not libvpx's default of 25.** The default holds 25 frames inside
        // the encoder before emitting anything, which on a desktop is most of a
        // second of latency. RustDesk works around the default by flushing after
        // every frame; zero is the same thing without the second code path, and
        // it is what makes one encode mean one packet.
        cfg.g_lag_in_frames = 0;
        cfg.g_pass = vpx::vpx_enc_pass_VPX_RC_ONE_PASS;
        // No error resilience. Nothing here is ever lost in transit — the link
        // is TCP — so it would cost compression to protect against loss that
        // cannot happen. The same argument removes the periodic keyframe below.
        cfg.g_error_resilient = 0;
        // No fixed keyframe interval; every keyframe is one somebody asked for —
        // a repaint, a resize, a client coming back.
        cfg.kf_mode = vpx::vpx_kf_mode_VPX_KF_DISABLED;
        // Disabling them is not enough: libvpx 1.16's one-pass rate control still
        // counts down `kf_max_dist`, 128 by default, and codes a keyframe when it
        // runs out. Measured, at frames 128 and 256 of a changing picture. Out of
        // reach instead.
        cfg.kf_max_dist = i32::MAX as u32;
        // Constant quality with the quantizer pinned top and bottom. `VPX_Q` plus
        // the `CQ_LEVEL` control below is what decides it; min == max is what
        // makes that a guarantee rather than a preference, and it is why nothing
        // here sets a bitrate — the quantizer *is* the dial and the bytes land
        // wherever the picture puts them.
        cfg.rc_end_usage = vpx::vpx_rc_mode_VPX_Q;
        cfg.rc_min_quantizer = q;
        cfg.rc_max_quantizer = q;
        // Zero, explicitly, and load-bearing rather than tidy: a session records
        // source pixels as delivered the moment they are captured, so a frame
        // libvpx decided to drop would be permanently wrong pixels that nothing
        // re-sends. It is also libvpx's default, and a default is a thing that
        // can change.
        cfg.rc_dropframe_thresh = 0;
        // Likewise: an encoder that resized itself would change the picture size
        // mid-stream, while a stream has one picture size for its whole life.
        cfg.rc_resize_allowed = 0;

        // SAFETY: `ctx` is zeroed and written through by `enc_init_ver`, `cfg` is
        // the struct libvpx just filled in and this function then edited by
        // name, and the ABI version is the one the linked archive's headers
        // declared — which is what makes a Rust-side layout disagreement a named
        // error here rather than memory corruption later.
        let mut ctx: Box<vpx::vpx_codec_ctx_t> = Box::new(unsafe { std::mem::zeroed() });
        check(
            unsafe { vpx::vpx_codec_enc_init_ver(&mut *ctx, iface, &cfg, 0, vpx::VPX_ENCODER_ABI_VERSION as c_int) },
            "enc_init_ver",
        )?;

        // Everything from here on has a context to destroy, so the error paths
        // go through a constructed `Self` rather than returning: `Drop` is what
        // releases the encoder, and an early `?` before the struct would leak it.
        let mut encoder = Self {
            ctx,
            cfg,
            // SAFETY: zeroed, then filled in by `vpx_img_wrap` below.
            img: unsafe { std::mem::zeroed() },
            size: (width, height),
            chroma,
            quality,
            speed: Speed::default(),
            running: (quality, Speed::default()),
            started: std::time::Instant::now(),
            pts: -1,
            active: vec![0; usize::from(width).div_ceil(ACTIVE_BLOCK) * usize::from(height).div_ceil(ACTIVE_BLOCK)],
            mapped: false,
            coded_at: vec![quality; usize::from(width).div_ceil(ACTIVE_BLOCK) * usize::from(height).div_ceil(ACTIVE_BLOCK)],
            begun: false,
        };

        // SAFETY: the context is live and each control's argument really is an
        // `int` — the one thing `vpx_codec_control_`'s variadic signature cannot
        // check.
        unsafe {
            // In `VPX_Q` mode this is the value that actually decides the
            // quantizer. A *control*, not a config field, which is the easiest
            // thing about libvpx's rate control to get wrong: setting only
            // `rc_min/max_quantizer` leaves `cq_level` at its default and the
            // dial half-connected.
            encoder.control(vpx::vp8e_enc_control_id_VP8E_SET_CQ_LEVEL, q as c_int, "cq_level")?;
            // What this encoder is actually looking at. Screen content is mostly
            // flat colour, hard edges and text, none of which a camera preset
            // expects.
            encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_TUNE_CONTENT, vpx::vp9e_tune_content_VP9E_CONTENT_SCREEN as c_int, "tune_content")?;
            encoder.control(vpx::vp8e_enc_control_id_VP8E_SET_CPUUSED, cpu_used(encoder.speed), "cpuused")?;
            // Say in the bitstream what the conversion did: BT.601 matrix,
            // studio swing. libvpx writes *unknown* unless told, and a decoder
            // given unknown guesses — Chromium picks BT.709 for anything HD — so
            // without these two controls a 1080p desktop is converted with one
            // matrix and displayed with another, and every saturated colour
            // lands a little off. The decoder reads this off the keyframe
            // header; nothing on the wire has to carry it.
            encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_COLOR_SPACE, vpx::vpx_color_space_VPX_CS_BT_601 as c_int, "color_space")?;
            encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_COLOR_RANGE, vpx::vpx_color_range_VPX_CR_STUDIO_RANGE as c_int, "color_range")?;
            // Off: adaptive quantization would move the quantizer off the dial
            // that was just pinned.
            encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_AQ_MODE, 0, "aq_mode")?;
            // Tile columns for the threads to have separate work — as many as
            // the width is wide enough for and the threads can fill. Set whatever
            // the thread count: libvpx's default is 6, every column the width
            // allows, which one thread would code one after another for the
            // bytes and nothing else.
            encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_TILE_COLUMNS, tile_columns_log2(width, threads) as c_int, "tile_columns")?;
            // libvpx's 2 is no loop filter on any frame, 0 its own choice of level
            // on every one.
            encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_DISABLE_LOOPFILTER, if loop_filtered(tile_columns_log2(width, threads)) { 0 } else { 2 }, "disable_loopfilter")?;
            if threads > 1 {
                // What turns `g_threads` into actual parallelism inside a tile:
                // row-based multithreading. Inert at one thread.
                encoder.control(vpx::vp8e_enc_control_id_VP9E_SET_ROW_MT, 1, "row_mt")?;
            }
            // An image that *wraps* a picture's planes rather than owning any. A
            // non-null pointer that is never dereferenced is libvpx's own idiom
            // for "compute the layout, allocate nothing" — ffmpeg passes a
            // literal `1` — and it is why `vpx_img_free` must never be called on
            // this image.
            let wrapped = vpx::vpx_img_wrap(&mut encoder.img, image_format(chroma), cfg.g_w, cfg.g_h, 1, std::ptr::dangling_mut::<u8>());
            if wrapped.is_null() {
                return Err(Error::Codec { call: "img_wrap", detail: format!("refused a {width}x{height} picture") });
            }
        }
        Ok(encoder)
    }

    /// The picture size this encoder codes.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// How this encoder's stream samples colour.
    pub fn chroma(&self) -> Chroma {
        self.chroma
    }

    /// The dial this encoder is coding at.
    pub fn quality(&self) -> u8 {
        self.quality
    }

    /// Move the dial, without a keyframe: the next frame is
    /// coded at the new quantizer against the frames before it. This is how a
    /// link that is behind gives quality up, and "without a keyframe" is the
    /// whole reason it is written this way rather than by rebuilding the encoder,
    /// which would force a keyframe — the most bytes a frame can be — at the
    /// exact moment the link has run out of room.
    ///
    /// Nothing is asked of libvpx here: the frame that is coded at the dial
    /// takes libvpx to it first, and a libvpx that refuses is that frame's
    /// error ([`Self::encode`]).
    pub fn set_quality(&mut self, quality: u8) {
        self.quality = quality.clamp(QUALITY_MIN, QUALITY_MAX);
    }

    /// Bring libvpx to the dial and the speed the next frame is to be coded
    /// at, where it is not there already.
    ///
    /// `vpx_codec_enc_config_set` is libvpx's own mechanism for the dial, and
    /// both halves are needed: the config carries the quantizer bounds and the
    /// control carries the value `VPX_Q` mode actually reads. A refusal leaves
    /// libvpx where it was, to be asked again by the next frame.
    fn follow(&mut self) -> Result<(), Error> {
        if self.speed != self.running.1 {
            // SAFETY: the context is live and the control's argument is an `int`.
            unsafe { self.control(vpx::vp8e_enc_control_id_VP8E_SET_CPUUSED, cpu_used(self.speed), "cpuused")? };
            self.running.1 = self.speed;
        }
        let q = quality_to_q(self.quality);
        if q == quality_to_q(self.running.0) {
            self.running.0 = self.quality;
            return Ok(());
        }
        // Edited on a copy and kept only once libvpx has accepted it, so a
        // refusal leaves `cfg` describing the encoder as it still is.
        let mut cfg = self.cfg;
        cfg.rc_min_quantizer = q;
        cfg.rc_max_quantizer = q;
        // SAFETY: `cfg` is the struct libvpx validated at init with two fields
        // changed, and the context is live. libvpx re-validates it and returns a
        // code rather than accepting nonsense.
        unsafe {
            check(vpx::vpx_codec_enc_config_set(&mut *self.ctx, &cfg), "enc_config_set")?;
            if let Err(refused) = self.control(vpx::vp8e_enc_control_id_VP8E_SET_CQ_LEVEL, q as c_int, "cq_level") {
                // The bounds go back, so a refusal leaves libvpx wholly where
                // it was. Should it refuse the config it was running on a
                // moment ago, the bounds pinned at `q` are what it codes at,
                // whatever `cq_level` says: it is at the new dial.
                if check(vpx::vpx_codec_enc_config_set(&mut *self.ctx, &self.cfg), "enc_config_set").is_err() {
                    self.cfg = cfg;
                    self.running.0 = self.quality;
                }
                return Err(refused);
            }
            self.cfg = cfg;
        }
        self.running.0 = self.quality;
        Ok(())
    }

    /// The coarsest dial any block of the picture a decoder now holds was last
    /// coded at: [`Self::quality`] as each frame was encoded, for the blocks
    /// that frame coded. A frame told where its picture changed leaves the
    /// rest at the dial they had, so a picture the dial was walked down for is
    /// coarse here until every block of it has been coded finer, which a whole
    /// frame does at once — what a caller that settles a quiet picture asks
    /// before deciding it owes one. Before the first frame, the dial.
    pub fn coarsest(&self) -> u8 {
        if !self.begun {
            return self.quality;
        }
        self.coded_at.iter().copied().min().unwrap_or(self.quality)
    }

    /// The speed this encoder is coding at.
    pub fn speed(&self) -> Speed {
        self.speed
    }

    /// Move the speed, without a keyframe, as [`Self::set_quality`] moves
    /// the dial: the next frame is searched at the new speed against the
    /// frames before it.
    pub fn set_speed(&mut self, speed: Speed) {
        self.speed = speed;
    }

    /// Settle `picture` at `quality`: encode it whole and at that dial, and
    /// leave the dial where it was, for the frames after it. Returns whether
    /// the frame is a keyframe, as [`Self::encode`] does.
    pub fn settle(&mut self, picture: &Picture, quality: u8, keyframe: bool, out: &mut Vec<u8>) -> Result<bool, Error> {
        let walked = self.quality;
        self.set_quality(quality);
        let encoded = self.encode(picture, keyframe, None, out);
        self.quality = walked;
        encoded
    }

    /// Encode `picture` and append the frame to `out`. `keyframe` makes it one a
    /// decoder can start from; the first frame is one either way.
    ///
    /// `changed` is where the picture differs from the one encoded before it,
    /// or `None` for a picture that may differ anywhere. Blocks outside every
    /// rectangle are skipped — not searched, and left in the decoder's picture
    /// as they are — which is most of what a frame costs to encode when little
    /// moved: a 4K frame with one small change took 26 ms whole and 11 told
    /// where. It is the caller's word and not checked: a change outside the
    /// rectangles is not coded, by this frame or by a later one that does not
    /// name it. A skipped block also keeps the quantizer it was last coded at,
    /// so the frame that is to sharpen an unchanged picture after
    /// [`Self::set_quality`] passes `None`. A keyframe codes everything
    /// whatever `changed` says.
    ///
    /// Returns whether the frame is a keyframe. A libvpx that would not be
    /// brought to the dial or the speed is an error before anything is coded,
    /// and one that made no frame of the picture is [`Error::NoFrame`]:
    /// nothing is appended either way.
    pub fn encode(&mut self, picture: &Picture, keyframe: bool, changed: Option<&[Rect]>, out: &mut Vec<u8>) -> Result<bool, Error> {
        if picture.size() != self.size {
            let (w, h) = self.size;
            let (pw, ph) = picture.size();
            return Err(Error::Mismatch(w, h, pw, ph));
        }
        if picture.chroma() != self.chroma {
            return Err(Error::Sampling(self.chroma, picture.chroma()));
        }
        self.follow()?;
        // Strictly increasing, which two encodes inside one millisecond would
        // otherwise not be.
        self.pts = (self.started.elapsed().as_millis() as i64).max(self.pts + 1);
        // `vpx_enc_frame_flags_t` is a C `long`: 64 bits on Linux and macOS, 32 on
        // Windows. A cast rather than `From`, because no `From<u32>` exists for
        // the 32-bit case.
        let flags: vpx::vpx_enc_frame_flags_t = if keyframe { vpx::VPX_EFLAG_FORCE_KF as vpx::vpx_enc_frame_flags_t } else { 0 };
        let (planes, strides) = (picture.planes(), picture.strides());
        // An encoder's first frame is a keyframe whether asked or not, and a
        // keyframe has no blocks to skip.
        self.map(changed.filter(|_| !keyframe && self.begun))?;

        // SAFETY: the three planes outlive this call — they belong to `picture`,
        // which is borrowed for it — and the strides are the ones the picture
        // reports for exactly this size and chroma. Casting away const is what the C API
        // requires; libvpx does not write to an input image. The packets drained
        // below point into the encoder and are copied out before it is called
        // again, which is the lifetime libvpx documents for them.
        unsafe {
            for (to, plane) in self.img.planes.iter_mut().zip(planes) {
                *to = plane.as_ptr().cast_mut();
            }
            for (to, stride) in self.img.stride.iter_mut().zip(strides) {
                *to = stride as c_int;
            }
            check(
                vpx::vpx_codec_encode(
                    &mut *self.ctx,
                    &self.img,
                    self.pts,
                    // Duration: one tick of the timebase, deliberately a constant
                    // rather than the gap since the last frame. libvpx reads it
                    // for rate control, and there is no rate control here to read
                    // it: the quantizer is pinned, no bitrate is set, and no frame
                    // may be dropped. What must stay true is `pts`, which is real
                    // elapsed time — a decoder's timestamps read that, not this.
                    1,
                    flags,
                    vpx::VPX_DL_REALTIME as c_ulong,
                ),
                "encode",
            )?;
            let from = out.len();
            let mut keyframe = false;
            let mut packets = 0usize;
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
                keyframe |= frame.flags & vpx::VPX_FRAME_IS_KEY != 0;
                packets += 1;
            }
            // More than one packet is a superframe, which is a legal thing to
            // concatenate and hand to a decoder as one frame — but it also means
            // no lag is not doing what this module assumes, so it is said rather
            // than silently absorbed.
            if packets > 1 {
                log::warn!("vp9: one encode produced {packets} packets; expected one");
            }
            if out.len() == from {
                return Err(Error::NoFrame);
            }
            // What this frame coded is at this dial now: the blocks its map
            // names, or all of them.
            if self.mapped && !keyframe {
                for (at, _) in self.coded_at.iter_mut().zip(&self.active).filter(|(_, active)| **active != 0) {
                    *at = self.quality;
                }
            } else {
                self.coded_at.fill(self.quality);
            }
            self.begun = true;
            Ok(keyframe)
        }
    }

    /// Tell libvpx which blocks the next frame may code: those `changed`
    /// touches, or with `None` all of them. libvpx keeps a map until it is
    /// given another, so one is handed over for every frame that has
    /// rectangles and taken back by the first that has none.
    fn map(&mut self, changed: Option<&[Rect]>) -> Result<(), Error> {
        let (cols, rows) = (usize::from(self.size.0).div_ceil(ACTIVE_BLOCK), usize::from(self.size.1).div_ceil(ACTIVE_BLOCK));
        let active_map = match changed {
            Some(rects) => {
                self.active.fill(0);
                for rect in rects {
                    let (x, y) = (usize::from(rect.x), usize::from(rect.y));
                    let right = (x + usize::from(rect.width)).min(usize::from(self.size.0));
                    let bottom = (y + usize::from(rect.height)).min(usize::from(self.size.1));
                    if x >= right || y >= bottom {
                        continue;
                    }
                    let (first, last) = (x / ACTIVE_BLOCK, (right - 1) / ACTIVE_BLOCK);
                    for row in y / ACTIVE_BLOCK..=(bottom - 1) / ACTIVE_BLOCK {
                        self.active[row * cols + first..=row * cols + last].fill(1);
                    }
                }
                self.active.as_mut_ptr()
            }
            None if self.mapped => std::ptr::null_mut(),
            None => return Ok(()),
        };
        let mut map = vpx::vpx_active_map { active_map, rows: rows as u32, cols: cols as u32 };
        // SAFETY: the context is live, the control's argument is a pointer to
        // a `vpx_active_map`, and the map it names is a byte for each of the
        // `rows`×`cols` blocks libvpx counts for this size — it refuses any
        // other count. libvpx copies the map out before the call returns, and
        // a null one is how a map is taken back.
        check(unsafe { vpx::vpx_codec_control_(&mut *self.ctx, vpx::vp8e_enc_control_id_VP8E_SET_ACTIVEMAP as c_int, &mut map as *mut vpx::vpx_active_map) }, "active_map")?;
        self.mapped = changed.is_some();
        Ok(())
    }

    /// One `vpx_codec_control_` call with an `int` argument, checked.
    ///
    /// # Safety
    ///
    /// `id` must be a control whose argument really is an `int`. The call is
    /// variadic, so passing the wrong type compiles and corrupts the stack.
    unsafe fn control(&mut self, id: vpx::vp8e_enc_control_id, value: c_int, call: &'static str) -> Result<(), Error> {
        check(unsafe { vpx::vpx_codec_control_(&mut *self.ctx, id as c_int, value) }, call)
    }
}

// SAFETY: an `Encoder` owns its context exclusively — it is not `Clone`, and
// `encode` and `set_quality` take `&mut self` — and libvpx keeps no thread-local
// state for an encoder instance, so moving one between threads is sound. A
// session's task carries one from one of the runtime's threads to another.
//
// Deliberately **not** `Sync`. Two threads calling `vpx_codec_encode` on one
// context concurrently is undefined behaviour, and nothing needs to.
//
// `img`'s plane pointers borrow whatever picture was last encoded; they are
// overwritten at the top of every `encode` before libvpx reads them, so a stale
// pointer is never dereferenced.
unsafe impl Send for Encoder {}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: destroyed exactly once — `Encoder` is not `Clone` and holds the
        // only handle. The wrapped image is deliberately *not* freed:
        // `vpx_img_wrap` allocated nothing.
        unsafe {
            vpx::vpx_codec_destroy(&mut *self.ctx);
        }
    }
}

/// One VP9 stream's decoder: every frame through the same one, in the order
/// they came, since each is coded against the ones before it.
pub struct Decoder {
    /// Boxed for the same reason as the encoder's.
    ctx: Box<vpx::vpx_codec_ctx_t>,
}

impl Decoder {
    /// A decoder on `threads` threads. A decode gains nothing past the stream's
    /// tile columns, and the caller knows what else its machine is doing.
    pub fn new(threads: usize) -> Result<Self, Error> {
        if !(1..=MAX_THREADS).contains(&threads) {
            return Err(Error::Threads(threads));
        }
        // SAFETY: a static interface, a zeroed context written through by
        // `dec_init_ver`, and the ABI version of the linked archive's headers.
        unsafe {
            let iface = vpx::vpx_codec_vp9_dx();
            if iface.is_null() {
                return Err(Error::Codec { call: "vp9_dx", detail: "this libvpx has no VP9 decoder".to_owned() });
            }
            let cfg = vpx::vpx_codec_dec_cfg_t { threads: threads as c_uint, w: 0, h: 0 };
            let mut ctx: Box<vpx::vpx_codec_ctx_t> = Box::new(std::mem::zeroed());
            check(vpx::vpx_codec_dec_init_ver(&mut *ctx, iface, &cfg, 0, vpx::VPX_DECODER_ABI_VERSION as c_int), "dec_init_ver")?;
            Ok(Self { ctx })
        }
    }

    /// Decode one `frame` and return the picture it leaves on screen, which
    /// borrows the decoder until the next call.
    pub fn decode(&mut self, frame: &[u8]) -> Result<Decoded<'_>, Error> {
        // SAFETY: `frame` outlives the call. The image libvpx hands back belongs
        // to the decoder and is valid until it is called again, which the
        // returned borrow forbids.
        unsafe {
            check(vpx::vpx_codec_decode(&mut *self.ctx, frame.as_ptr(), frame.len() as c_uint, std::ptr::null_mut(), 0), "decode")?;
            let mut iter: vpx::vpx_codec_iter_t = std::ptr::null();
            let img = vpx::vpx_codec_get_frame(&mut *self.ctx, &mut iter);
            if img.is_null() {
                return Err(Error::NoPicture);
            }
            let img = &*img;
            let chroma = [Chroma::Yuv444, Chroma::Yuv420].into_iter().find(|&chroma| img.fmt == image_format(chroma));
            match chroma {
                Some(chroma) if img.bit_depth == 8 => Ok(Decoded { img, chroma }),
                _ => Err(Error::Format(img.fmt as u32, img.bit_depth)),
            }
        }
    }
}

// SAFETY: as the encoder's — one owner, `&mut self` for every call.
unsafe impl Send for Decoder {}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the only handle, destroyed once.
        unsafe {
            vpx::vpx_codec_destroy(&mut *self.ctx);
        }
    }
}

/// The picture a decoded frame leaves on screen: libvpx's own image, borrowed
/// from the decoder.
pub struct Decoded<'a> {
    /// Eight bits, at one of the two chromas, which the decode held it to.
    img: &'a vpx::vpx_image_t,
    chroma: Chroma,
}

impl Decoded<'_> {
    /// The picture's size.
    pub fn size(&self) -> (u32, u32) {
        (self.img.d_w, self.img.d_h)
    }

    /// How the picture's colour is sampled.
    pub fn chroma(&self) -> Chroma {
        self.chroma
    }

    /// Whether the stream declares its pixels BT.601 at studio swing, which is
    /// what every encoder here writes and what [`Self::write_bgrx`] assumes.
    pub fn declares_bt601_studio_swing(&self) -> bool {
        self.img.cs == vpx::vpx_color_space_VPX_CS_BT_601 && self.img.range == vpx::vpx_color_range_VPX_CR_STUDIO_RANGE
    }

    /// Y, U and V, each at its own stride from [`Self::strides`], the chroma
    /// planes at the chroma's size.
    pub fn planes(&self) -> [&[u8]; 3] {
        let plane = |i: usize| {
            let (mut w, mut h) = (self.img.d_w as usize, self.img.d_h as usize);
            if i > 0 {
                (w, h) = (w.div_ceil(1 << self.img.x_chroma_shift), h.div_ceil(1 << self.img.y_chroma_shift));
            }
            let stride = self.img.stride[i] as usize;
            // SAFETY: the plane is the decoder's, laid out at the stride it
            // reports, and read only inside the size it reports, shifted for
            // a chroma plane by what it reports of its sampling.
            unsafe { std::slice::from_raw_parts(self.img.planes[i], (h - 1) * stride + w) }
        };
        [plane(0), plane(1), plane(2)]
    }

    /// The width of a row of each plane, in bytes.
    pub fn strides(&self) -> [usize; 3] {
        [self.img.stride[0] as usize, self.img.stride[1] as usize, self.img.stride[2] as usize]
    }

    /// Write the picture into `out` as `B, G, R, X`, rows `stride` bytes apart,
    /// the X byte zero. BT.601 at studio swing, as the encoders here declare.
    pub fn write_bgrx(&self, out: &mut [u8], stride: usize) -> Result<(), Error> {
        use yuv::{YuvPlanarImage, YuvRange, YuvStandardMatrix};
        let (w, h) = (self.img.d_w as usize, self.img.d_h as usize);
        if !fits(w, h, stride, out.len()) {
            return Err(Error::Buffer { width: w, height: h, stride, bytes: out.len() });
        }
        let [y_plane, u_plane, v_plane] = self.planes();
        let [ys, us, vs] = self.strides();
        let image = YuvPlanarImage {
            y_plane,
            y_stride: ys as u32,
            u_plane,
            u_stride: us as u32,
            v_plane,
            v_stride: vs as u32,
            width: w as u32,
            height: h as u32,
        };
        match self.chroma {
            Chroma::Yuv444 => yuv::yuv444_to_bgra(&image, out, stride as u32, YuvRange::Limited, YuvStandardMatrix::Bt601),
            Chroma::Yuv420 => yuv::yuv420_to_bgra(&image, out, stride as u32, YuvRange::Limited, YuvStandardMatrix::Bt601),
        }
        .expect("the picture fits its buffer, which was checked");
        // It writes an opaque alpha where the X byte goes.
        for row in 0..h {
            for pixel in out[row * stride..row * stride + w * 4].as_chunks_mut::<4>().0 {
                pixel[3] = 0;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // libvpx's encoder itself, and the decoder: what is neither's — the
    // planes, the dial, the frame header — is tested once, in the crate's.
    use super::{Encoder, *};
    use crate::*;

    /// `w`×`h` of one colour, packed RGB.
    fn flat(w: u16, h: u16, colour: [u8; 3]) -> Vec<u8> {
        colour.iter().copied().cycle().take(usize::from(w) * usize::from(h) * 3).collect()
    }

    /// A picture read from `rgb`.
    fn picture(w: u16, h: u16, rgb: &[u8]) -> Picture {
        let mut picture = Picture::new(w, h, Chroma::Yuv444, 1).expect("a picture");
        picture.read_rgb(rgb).expect("its own picture");
        picture
    }

    /// Encode `picture` and return the frame with its keyframe bit.
    fn encode(encoder: &mut Encoder, picture: &Picture, keyframe: bool) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        let key = encoder.encode(picture, keyframe, None, &mut out).expect("an encode");
        (out, key)
    }

    /// A chain of frames through one decoder, keyframe first, and the picture
    /// the last one leaves as `B, G, R, X` rows of `w` pixels.
    fn decode_chain(frames: &[Vec<u8>], w: usize, h: usize) -> Vec<u8> {
        let mut decoder = Decoder::new(1).expect("a decoder");
        let mut out = vec![0; w * h * 4];
        for frame in frames {
            let decoded = decoder.decode(frame).expect("a decode");
            assert_eq!(decoded.size(), (w as u32, h as u32));
            decoded.write_bgrx(&mut out, w * 4).expect("a picture that fits");
        }
        out
    }

    /// The pixel at `(x, y)` of a decoded picture, as RGB.
    fn rgb_at(bgrx: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let p = &bgrx[(y * w + x) * 4..][..3];
        [p[2], p[1], p[0]]
    }

    /// A dark terminal with one-pixel coloured glyph stems: the picture 4:2:0
    /// would not carry. Every stem is at an odd column, and would share a 2×2
    /// chroma sample with three pixels of background.
    fn stems(w: u16, h: u16) -> (Vec<u8>, Vec<(usize, usize)>) {
        let (w, h) = (usize::from(w), usize::from(h));
        let mut rgb = flat(w as u16, h as u16, [30, 30, 30]);
        let mut at = Vec::new();
        for y in (0..h).filter(|y| y % 2 == 0) {
            for x in (1..w).step_by(4) {
                rgb[(y * w + x) * 3..][..3].copy_from_slice(&[255, 121, 198]);
                at.push((x, y));
            }
        }
        (rgb, at)
    }

    /// A 320×240 picture with a block moved by `step`, so there is something for
    /// the quantizer to be coarse about — a stream of identical frames costs
    /// nearly nothing at any quality and would compare two zeroes.
    fn moving(step: u16) -> Vec<u8> {
        let mut rgb = flat(320, 240, [30, 60, 90]);
        let stride = 320 * 3;
        for row in 0..80 {
            let at = (usize::from(step) * 3 + row) * stride + usize::from(step) * 9;
            rgb[at..at + 300].fill(230);
        }
        rgb
    }

    /// A frame told where its picture changed codes those blocks and no
    /// others: a change it was not told of stays out of the decoder's picture
    /// until a frame names it, and a block left out of one frame is coded by
    /// the next that names it. A frame told nothing codes the whole picture
    /// again, and a rectangle past the picture's edge is no error.
    #[test]
    fn a_frame_told_where_it_changed_codes_that_and_nothing_else() {
        // Neither side a multiple of the map's 16-pixel block.
        let (w, h) = (333u16, 250u16);
        let (wu, hu) = (usize::from(w), usize::from(h));
        const GREY: [u8; 3] = [200, 200, 200];
        let square = |x: u16, y: u16| Rect { x, y, width: 32, height: 32 };
        let (first, second, third) = (square(32, 32), square(200, 150), square(96, 200));
        let paint = |rgb: &mut [u8], rect: Rect, colour: [u8; 3]| {
            for y in usize::from(rect.y)..usize::from(rect.y + rect.height) {
                for x in usize::from(rect.x)..usize::from(rect.x + rect.width) {
                    rgb[(y * wu + x) * 3..][..3].copy_from_slice(&colour);
                }
            }
        };
        let mut encoder = Encoder::new(w, h, 90, Chroma::Yuv444, 2).expect("an encoder");
        let mut rgb = flat(w, h, GREY);
        let mut frames = vec![encode(&mut encoder, &picture(w, h, &rgb), false).0];
        let mut step = |rgb: &[u8], changed: Option<&[Rect]>, want: [(Rect, [u8; 3]); 3]| {
            let mut out = Vec::new();
            let key = encoder.encode(&picture(w, h, rgb), false, changed, &mut out).expect("an encode");
            assert!(!key, "a frame told where it changed cost a keyframe");
            frames.push(out);
            let back = decode_chain(&frames, wu, hu);
            for (rect, colour) in want {
                let got = rgb_at(&back, wu, usize::from(rect.x) + 16, usize::from(rect.y) + 16);
                assert!((0..3).all(|c| got[c].abs_diff(colour[c]) <= 40), "told {changed:?}: {rect:?} is {got:?}, not {colour:?}");
            }
        };

        // Two squares change and the frame is told of one.
        paint(&mut rgb, first, [220, 40, 40]);
        paint(&mut rgb, second, [40, 40, 220]);
        step(&rgb, Some(&[first]), [(first, [220, 40, 40]), (second, GREY), (third, GREY)]);
        // The other, which the frame before skipped, is coded when named.
        step(&rgb, Some(&[second]), [(first, [220, 40, 40]), (second, [40, 40, 220]), (third, GREY)]);
        // Told nothing, a frame codes whatever changed.
        paint(&mut rgb, third, [40, 180, 40]);
        step(&rgb, None, [(first, [220, 40, 40]), (second, [40, 40, 220]), (third, [40, 180, 40])]);
        // And a map after that one holds again, its rectangles clipped to
        // the picture.
        paint(&mut rgb, first, GREY);
        paint(&mut rgb, third, GREY);
        let edge = Rect { x: 320, y: 240, width: 100, height: 100 };
        let outside = Rect { x: 400, y: 300, width: 16, height: 16 };
        step(&rgb, Some(&[first, edge, outside]), [(first, GREY), (second, [40, 40, 220]), (third, [40, 180, 40])]);
        // Nothing named is nothing coded.
        step(&rgb, Some(&[]), [(first, GREY), (second, [40, 40, 220]), (third, [40, 180, 40])]);
        step(&rgb, Some(&[third]), [(first, GREY), (second, [40, 40, 220]), (third, GREY)]);
    }

    /// A block a frame skips keeps the quantizer it was last coded at, so a
    /// frame at a finer one that is told where the picture changed sharpens
    /// that and leaves the rest coarse: what a caller that settles a quiet
    /// picture must send its settle whole for.
    #[test]
    fn a_skipped_block_keeps_its_quantizer_until_a_whole_frame() {
        let (w, h) = (320u16, 240u16);
        let mut rgb = flat(w, h, [240, 240, 240]);
        let mut seed = 12_345u32;
        for px in rgb.chunks_mut(3) {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if (seed >> 16).is_multiple_of(5) {
                px.copy_from_slice(&[20, 20, 20]);
            }
        }
        let source = picture(w, h, &rgb);
        let (wu, hu) = (usize::from(w), usize::from(h));
        let named = Rect { x: 0, y: 0, width: 64, height: 64 };
        // The error of everything the rectangle leaves out.
        let error = |bgrx: &[u8]| {
            let rest = (0..hu).flat_map(|y| (0..wu).map(move |x| (x, y))).filter(|&(x, y)| x >= 64 || y >= 64);
            let sum: u64 = rest.clone().map(|(x, y)| u64::from(rgb_at(bgrx, wu, x, y)[1].abs_diff(rgb[(y * wu + x) * 3 + 1]))).sum();
            sum as f64 / rest.count() as f64
        };

        let mut encoder = Encoder::new(w, h, QUALITY_MIN, Chroma::Yuv444, 2).expect("an encoder");
        // Before a frame nothing is coded at any dial, wherever it has been.
        let mut unused = Encoder::new(w, h, 20, Chroma::Yuv444, 2).expect("an encoder");
        unused.set_quality(90);
        assert_eq!(unused.coarsest(), 90, "before a frame, the dial as it stands");
        assert_eq!(encoder.coarsest(), QUALITY_MIN);
        let mut frames = vec![encode(&mut encoder, &source, false).0];
        let coarse = error(&decode_chain(&frames, wu, hu));
        assert_eq!(encoder.coarsest(), QUALITY_MIN);

        encoder.set_quality(90);
        assert_eq!(encoder.coarsest(), QUALITY_MIN, "a dial that moved has coded nothing yet");
        let mut partial = Vec::new();
        encoder.encode(&source, false, Some(&[named]), &mut partial).expect("an encode");
        frames.push(partial);
        let skipped = error(&decode_chain(&frames, wu, hu));
        assert!(skipped > coarse * 0.9, "a frame told of one corner left the rest at error {skipped:.2}, from {coarse:.2}: it coded blocks it was to skip");
        assert_eq!(encoder.coarsest(), QUALITY_MIN, "the blocks a frame skipped are as coarse as they were");

        frames.push(encode(&mut encoder, &source, false).0);
        let settled = error(&decode_chain(&frames, wu, hu));
        assert!(settled < coarse / 4.0, "a whole frame at quality 90 left the picture at error {settled:.2}, from {coarse:.2}");
        assert_eq!(encoder.coarsest(), 90);

        // Coarse where a frame coded and nowhere else, until frames told where
        // have between them coded every block finer.
        encoder.set_quality(40);
        encoder.encode(&source, false, Some(&[named]), &mut Vec::new()).expect("an encode");
        assert_eq!(encoder.coarsest(), 40);
        encoder.set_quality(90);
        encoder.encode(&source, false, Some(&[Rect { x: 64, y: 0, width: 256, height: 240 }]), &mut Vec::new()).expect("an encode");
        assert_eq!(encoder.coarsest(), 40, "the corner coded at 40 was not in that frame");
        encoder.encode(&source, false, Some(&[Rect { x: 10, y: 10, width: 40, height: 40 }]), &mut Vec::new()).expect("an encode");
        assert_eq!(encoder.coarsest(), 90, "every block has been coded at 90 since");
    }

    /// A stream reads the rectangles a frame's changes are and nothing else of
    /// the buffer, and leaves the rest of the decoder's picture as it was. Its
    /// first read has no picture to be changes to, and is the whole buffer
    /// whatever it is told. A frame codes what was read since the one before
    /// it, in however many reads, and a keyframe all of the planes.
    #[test]
    fn a_stream_reads_and_codes_only_what_a_frame_names() {
        let (w, h) = (96u16, 80u16);
        let (wu, hu) = (usize::from(w), usize::from(h));
        const BACK: [u8; 3] = [20, 40, 80];
        const LIT: [u8; 3] = [240, 240, 240];
        const JUNK: [u8; 3] = [170, 85, 170];
        // On an odd row and column.
        let named = Rect { x: 33, y: 17, width: 31, height: 15 };
        // `rgb` in the byte order a stream is fed: packed RGB, or `B, G, R, X`.
        let packed = |rgb: &[u8], bgrx: bool| -> Vec<u8> { if bgrx { rgb.chunks(3).flat_map(|px| [px[2], px[1], px[0], 0]).collect() } else { rgb.to_vec() } };
        let paint = |rgb: &mut [u8], rect: Rect, colour: [u8; 3]| {
            for y in usize::from(rect.y)..usize::from(rect.y + rect.height) {
                for x in usize::from(rect.x)..usize::from(rect.x + rect.width) {
                    rgb[(y * wu + x) * 3..][..3].copy_from_slice(&colour);
                }
            }
        };
        let near = |got: [u8; 3], want: [u8; 3]| (0..3).all(|c| got[c].abs_diff(want[c]) <= 40);
        for bgrx in [true, false] {
            let mut stream = Stream::new(w, h, 90, Chroma::Yuv444, 2).expect("a stream");
            assert_eq!((stream.size(), stream.quality(), stream.coarsest()), ((w, h), 90, 90));
            let mut frames = Vec::new();
            let mut step = |stream: &mut Stream, rgb: &[u8], changed: Option<&[Rect]>, keyframe: bool| {
                let pixels = packed(rgb, bgrx);
                let mut out = Vec::new();
                if bgrx { stream.read_bgrx(&pixels, wu * 4, changed) } else { stream.read_rgb(&pixels, changed) }.expect("a read");
                let key = stream.encode(keyframe, &mut out);
                frames.push(out);
                (key.expect("an encode"), decode_chain(&frames, wu, hu))
            };
            let whole = flat(w, h, BACK);

            // Nothing was read, so there is nothing to code or to settle.
            assert!(matches!(stream.encode(false, &mut Vec::new()), Err(Error::Unread)));
            assert!(matches!(stream.settle(90, false, &mut Vec::new()), Err(Error::Unread)));

            // The first frame is told of changes it cannot be changes to.
            let (key, back) = step(&mut stream, &whole, Some(&[named]), false);
            assert!(key && near(rgb_at(&back, wu, 4, 70), BACK) && near(rgb_at(&back, wu, 48, 24), BACK), "bgrx {bgrx}: the first frame was not the whole picture");

            // A change, in a buffer all but the rectangle of which is not the
            // picture.
            let mut rows = flat(w, h, JUNK);
            paint(&mut rows, named, LIT);
            let (key, back) = step(&mut stream, &rows, Some(&[named]), false);
            assert!(!key && near(rgb_at(&back, wu, 48, 24), LIT), "bgrx {bgrx}: the change came back {:?}", rgb_at(&back, wu, 48, 24));
            for (x, y) in [(4, 4), (4, 24), (90, 24), (32, 24), (48, 4), (48, 15), (48, 16), (48, 32), (48, 70)] {
                assert!(near(rgb_at(&back, wu, x, y), BACK), "bgrx {bgrx}: ({x}, {y}), outside the change, came back {:?}", rgb_at(&back, wu, x, y));
            }

            // Two reads before a frame, which carries both: the rectangle put
            // back, and a band lit under it. Read from buffers that hold
            // nothing else, and are gone by the time the frame is coded.
            let band = Rect { x: 0, y: 64, width: w, height: 16 };
            for (rect, colour) in [(named, BACK), (band, LIT)] {
                let mut rows = flat(w, h, JUNK);
                paint(&mut rows, rect, colour);
                let pixels = packed(&rows, bgrx);
                if bgrx { stream.read_bgrx(&pixels, wu * 4, Some(&[rect])) } else { stream.read_rgb(&pixels, Some(&[rect])) }.expect("a read");
            }
            let mut out = Vec::new();
            assert!(!stream.encode(false, &mut out).expect("an encode"));
            frames.push(out);
            let back = decode_chain(&frames, wu, hu);
            assert!(near(rgb_at(&back, wu, 48, 24), BACK) && near(rgb_at(&back, wu, 48, 70), LIT) && near(rgb_at(&back, wu, 4, 4), BACK), "bgrx {bgrx}: a frame did not carry both of its reads");

            // A frame of nothing read is a frame, and changes nothing.
            let mut out = Vec::new();
            assert!(!stream.encode(false, &mut out).expect("an encode"));
            frames.push(out);
            assert_eq!(decode_chain(&frames, wu, hu), back, "bgrx {bgrx}: a frame of nothing read changed the picture");

            // A settle carries what was read and no frame has: the rectangle
            // lit again, with the rest as the planes hold it.
            let mut rows = flat(w, h, JUNK);
            paint(&mut rows, named, LIT);
            let pixels = packed(&rows, bgrx);
            if bgrx { stream.read_bgrx(&pixels, wu * 4, Some(&[named])) } else { stream.read_rgb(&pixels, Some(&[named])) }.expect("a read");
            let mut out = Vec::new();
            assert!(!stream.settle(90, false, &mut out).expect("a settle"));
            frames.push(out);
            let back = decode_chain(&frames, wu, hu);
            assert!(near(rgb_at(&back, wu, 48, 24), LIT) && near(rgb_at(&back, wu, 48, 70), LIT) && near(rgb_at(&back, wu, 4, 4), BACK), "bgrx {bgrx}: a settle did not carry what was read for it");

            // A keyframe is the whole picture, of which it was read one
            // rectangle: the planes hold the rest.
            let mut rows = flat(w, h, JUNK);
            paint(&mut rows, named, LIT);
            let pixels = packed(&rows, bgrx);
            if bgrx { stream.read_bgrx(&pixels, wu * 4, Some(&[named])) } else { stream.read_rgb(&pixels, Some(&[named])) }.expect("a read");
            let mut out = Vec::new();
            assert!(stream.encode(true, &mut out).expect("an encode"));
            let back = decode_chain(&[out], wu, hu);
            assert!(near(rgb_at(&back, wu, 48, 24), LIT) && near(rgb_at(&back, wu, 48, 70), LIT) && near(rgb_at(&back, wu, 4, 4), BACK), "bgrx {bgrx}: the keyframe was not the whole picture");

            // A buffer that is not the picture is refused whatever is to be
            // read of it, nothing included.
            for changed in [None, Some(&[named][..]), Some(&[][..])] {
                assert!(matches!(stream.read_rgb(&[0; 9], changed), Err(Error::Crop(..))));
                assert!(matches!(stream.read_bgrx(&[0; 9], 4, changed), Err(Error::Buffer { .. })));
            }
        }
    }

    /// The loop filter level a frame's uncompressed header carries (VP9
    /// bitstream §6.2), for a frame of this crate's encoder: eight bits, shown,
    /// and after a keyframe predicted from frames of its own size.
    fn filter_level(frame: &[u8]) -> u32 {
        let mut bits = frame.iter().flat_map(|byte| (0..8).rev().map(move |i| u32::from(byte >> i & 1)));
        let mut read = |n: usize| (0..n).fold(0, |value, _| value << 1 | bits.next().expect("a whole header"));
        assert_eq!(read(2), 2, "frame_marker");
        assert_eq!(read(1) | read(1) << 1, u32::from(Chroma::Yuv444.profile()), "profile");
        assert_eq!(read(1), 0, "show_existing_frame");
        let keyframe = read(1) == 0;
        assert_eq!((read(1), read(1)), (1, 0), "show_frame, error_resilient_mode");
        if keyframe {
            assert_eq!(read(24), 0x49_83_42, "frame_sync_code");
            assert_ne!(read(3), 7, "color_space is not sRGB");
            // color_range, the subsampling and a reserved bit.
            read(4);
            read(32);
            assert_eq!(read(1), 0, "render_and_frame_size_different");
        } else {
            // reset_frame_context, refresh_frame_flags, and a reference and its
            // sign bias three times.
            read(2 + 8 + 12);
            assert_eq!(read(1), 1, "found_ref");
            assert_eq!(read(1), 0, "render_and_frame_size_different");
            // allow_high_precision_mv, then the interpolation filter: one bit
            // if a block chooses its own, two more if the frame does.
            read(1);
            if read(1) == 0 {
                read(2);
            }
        }
        // refresh_frame_context, frame_parallel_decoding_mode, frame_context_idx.
        read(4);
        read(6)
    }

    /// A stream is filtered unless it is in four tile columns, and one that
    /// is not says so in every frame's header and decodes as any other.
    #[test]
    fn the_loop_filter_is_left_out_of_four_tile_columns() {
        assert!(loop_filtered(tile_columns_log2(2047, 8)));
        assert!(!loop_filtered(tile_columns_log2(2048, 8)));
        assert!(!loop_filtered(tile_columns_log2(3840, 4)));
        assert!(loop_filtered(tile_columns_log2(3840, 3)));

        for (w, threads, filtered) in [(1440, 4, true), (2048, 4, false), (2048, 2, true)] {
            let mut encoder = Encoder::new(w, 64, 50, Chroma::Yuv444, threads).expect("an encoder");
            let frames: Vec<Vec<u8>> = [[30, 60, 90], [200, 30, 30], [40, 180, 70]].iter().map(|colour| encode(&mut encoder, &picture(w, 64, &flat(w, 64, *colour)), false).0).collect();
            for (n, frame) in frames.iter().enumerate() {
                assert_eq!(filter_level(frame) > 0, filtered, "frame {n} of {w} wide, {threads} threads");
            }
            let decoded = decode_chain(&frames, usize::from(w), 64);
            let last = rgb_at(&decoded, usize::from(w), usize::from(w) - 1, 63);
            assert!(last.iter().zip([40u8, 180, 70]).all(|(c, want)| c.abs_diff(want) <= 8), "{w} wide, {threads} threads decoded as {last:?}");
        }
    }

    /// The two conversions are each other's inverse to within the rounding a
    /// studio-swing round trip costs, on every channel's full range, through a
    /// real encode at the finest quantizer.
    #[test]
    fn the_colour_conversion_round_trips_through_the_codec() {
        let colours: Vec<[u8; 4]> = (0..=255u8)
            .step_by(15)
            .flat_map(|r| (0..=255u8).step_by(51).flat_map(move |g| (0..=255u8).step_by(85).map(move |b| [b, g, r, 0])))
            .collect();
        let width = colours.len();
        let pixels: Vec<u8> = colours.iter().flatten().copied().collect();
        let mut picture = Picture::new(width as u16, 1, Chroma::Yuv444, 1).expect("a picture");
        picture.read_bgrx(&pixels, width * 4).expect("a row");
        assert!(picture.planes()[0].iter().all(|&y| (16..=235).contains(&y)), "luma stays in studio range");

        let mut encoder = Encoder::new(width as u16, 1, QUALITY_MAX, Chroma::Yuv444, 1).expect("an encoder");
        let (frame, _) = encode(&mut encoder, &picture, false);
        let back = decode_chain(&[frame], width, 1);
        for (i, (want, got)) in pixels.as_chunks::<4>().0.iter().zip(back.as_chunks::<4>().0).enumerate() {
            // Studio swing folds 256 levels into 219 and the quantizer costs a
            // little more; a wrong matrix would be off by tens.
            for channel in 0..3 {
                assert!(want[channel].abs_diff(got[channel]) <= 6, "pixel {i}: {want:?} came back {got:?}");
            }
            assert_eq!(got[3], 0, "X is written as zero");
        }
    }

    /// A last row with nothing after it is converted like the rows before it,
    /// which have a stride's padding after them, and written back the same way.
    #[test]
    fn the_last_row_needs_no_padding_after_it() {
        let (width, height, stride) = (3usize, 3usize, 3 * 4 + 8);
        let colour = |row: usize| [40 * row as u8, 200 - 50 * row as u8, 90, 0];
        let mut pixels = vec![0xAA; (height - 1) * stride + width * 4];
        for row in 0..height {
            for x in 0..width {
                pixels[row * stride + x * 4..][..4].copy_from_slice(&colour(row));
            }
        }
        let mut picture = Picture::new(width as u16, height as u16, Chroma::Yuv444, 1).expect("a picture");
        picture.read_bgrx(&pixels, stride).expect("a picture that fits");
        let mut encoder = Encoder::new(width as u16, height as u16, QUALITY_MAX, Chroma::Yuv444, 1).expect("an encoder");
        let (frame, _) = encode(&mut encoder, &picture, false);
        let mut decoder = Decoder::new(1).expect("a decoder");
        let mut back = vec![0xAA; (height - 1) * stride + width * 4];
        decoder.decode(&frame).expect("a decode").write_bgrx(&mut back, stride).expect("a picture that fits");
        for row in 0..height {
            for x in 0..width {
                let got = &back[row * stride + x * 4..][..4];
                let want = colour(row);
                // What is tested is the stride, not the quantizer: the
                // finest one still costs a few code values on a 3×3
                // picture.
                assert!((0..3).all(|c| want[c].abs_diff(got[c]) <= 8) && got[3] == 0, "row {row}: {want:?} came back {got:?}");
            }
        }
        assert!(back[width * 4..stride].iter().all(|&b| b == 0xAA), "the padding is not written");
    }

    #[test]
    fn a_lower_quality_makes_a_smaller_stream() {
        let bytes = |quality| {
            let mut encoder = Encoder::new(320, 240, quality, Chroma::Yuv444, 2).expect("an encoder");
            (0..5u16).map(|step| encode(&mut encoder, &picture(320, 240, &moving(step)), false).0.len()).sum::<usize>()
        };
        let (coarse, fine) = (bytes(5), bytes(90));
        assert!(coarse < fine, "quality 5 encoded {coarse} bytes and quality 90 encoded {fine}; the dial is not reaching the encoder");
    }

    #[test]
    fn the_first_frame_is_a_keyframe_and_another_can_be_asked_for() {
        let mut encoder = Encoder::new(320, 240, 60, Chroma::Yuv444, 2).expect("an encoder");
        let mut frame = |step, keyframe| encode(&mut encoder, &picture(320, 240, &moving(step)), keyframe);
        let first = frame(0, false);
        assert!(first.1, "a decoder has to be able to start somewhere");
        assert_eq!(frame_header(&first.0), Some(FrameHeader { profile: Chroma::Yuv444.profile(), keyframe: true }));
        assert!(!frame(1, false).1, "an unasked-for keyframe is bytes for nothing");
        assert!(!frame(2, false).1);
        let asked = frame(3, true);
        assert!(asked.1, "the keyframe flag did not reach the encoder");
        assert_eq!(frame_header(&asked.0), Some(FrameHeader { profile: Chroma::Yuv444.profile(), keyframe: true }));
        // And it is not sticky: the frame after a forced keyframe is an ordinary one.
        let after = frame(4, false);
        assert!(!after.1);
        assert_eq!(frame_header(&after.0), Some(FrameHeader { profile: Chroma::Yuv444.profile(), keyframe: false }));
    }

    /// However long a stream runs, no keyframe comes that was not asked for.
    #[test]
    fn no_keyframe_comes_unasked() {
        let (w, h) = (64u16, 48u16);
        let mut encoder = Encoder::new(w, h, 60, Chroma::Yuv444, 1).expect("an encoder");
        let mut rgb = flat(w, h, [80, 40, 20]);
        for step in 0..300u32 {
            let x = (step * 3) as usize % 48;
            for y in 8..24 {
                rgb[(y * usize::from(w) + x) * 3..][..3].copy_from_slice(&[step as u8, (step * 7) as u8, (step * 13) as u8]);
            }
            let (frame, keyframe) = encode(&mut encoder, &picture(w, h, &rgb), false);
            assert_eq!(keyframe, step == 0, "frame {step}");
            assert_eq!(frame_header(&frame).map(|header| header.profile), Some(Chroma::Yuv444.profile()), "frame {step}");
        }
    }

    /// What a settle rests on: re-encoding a picture that has not changed, at a
    /// finer quantizer, sharpens the *unchanged* blocks — as an ordinary inter
    /// frame, with no keyframe. Were libvpx to skip blocks whose source had not
    /// moved, a desktop sent coarse on a link that was behind would stay coarse
    /// until it next changed, and the settle would have to spend a keyframe.
    #[test]
    fn a_finer_quantizer_sharpens_an_unchanged_picture_without_a_keyframe() {
        let (w, h) = (320u16, 240u16);
        // Speckle, so a coarse quantizer has detail to lose.
        let mut rgb = flat(w, h, [240, 240, 240]);
        let mut seed = 12_345u32;
        for px in rgb.chunks_mut(3) {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if (seed >> 16).is_multiple_of(5) {
                px.copy_from_slice(&[20, 20, 20]);
            }
        }
        let source = picture(w, h, &rgb);
        let (wu, hu) = (usize::from(w), usize::from(h));
        let error = |bgrx: &[u8]| {
            let sum: u64 = (0..hu).flat_map(|y| (0..wu).map(move |x| (x, y))).map(|(x, y)| u64::from(rgb_at(bgrx, wu, x, y)[1].abs_diff(rgb[(y * wu + x) * 3 + 1]))).sum();
            sum as f64 / (wu * hu) as f64
        };

        let mut encoder = Encoder::new(w, h, QUALITY_MIN, Chroma::Yuv444, 2).expect("an encoder");
        let mut frames = vec![encode(&mut encoder, &source, false).0];
        let coarse = error(&decode_chain(&frames, wu, hu));

        encoder.set_quality(90);
        let (settle, keyframe) = encode(&mut encoder, &source, false);
        assert!(!keyframe, "the settle frame cost a keyframe");
        frames.push(settle);
        let settled = error(&decode_chain(&frames, wu, hu));

        let mut fresh = Encoder::new(w, h, 90, Chroma::Yuv444, 2).expect("an encoder");
        let fine = error(&decode_chain(&[encode(&mut fresh, &source, false).0], wu, hu));
        assert!(
            settled < coarse / 4.0 && settled < fine * 2.0,
            "one frame at quality 90 left the unchanged picture at error {settled:.2} (coarse {coarse:.2}, a quality-90 keyframe {fine:.2}): the encoder skipped blocks that did not move"
        );
    }

    /// A stream settles its picture with one frame: whole and at the quality
    /// asked whatever damage the frames before it were told, an inter frame
    /// unless a keyframe is asked, and with the dial back where the link had it
    /// for the frame after. A walk told what each frame left owes the settle
    /// until that one has gone.
    #[test]
    fn a_stream_settles_a_coarse_picture_and_keeps_its_dial() {
        let (w, h) = (320u16, 240u16);
        let (wu, hu) = (usize::from(w), usize::from(h));
        // Speckle, so a coarse quantizer has detail to lose.
        let mut rgb = flat(w, h, [240, 240, 240]);
        let mut seed = 12_345u32;
        for px in rgb.chunks_mut(3) {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if (seed >> 16).is_multiple_of(5) {
                px.copy_from_slice(&[20, 20, 20]);
            }
        }
        let bgrx: Vec<u8> = rgb.chunks(3).flat_map(|px| [px[2], px[1], px[0], 0]).collect();
        let error = |back: &[u8]| {
            let sum: u64 = (0..hu).flat_map(|y| (0..wu).map(move |x| (x, y))).map(|(x, y)| u64::from(rgb_at(back, wu, x, y)[1].abs_diff(rgb[(y * wu + x) * 3 + 1]))).sum();
            sum as f64 / (wu * hu) as f64
        };
        let start = std::time::Instant::now();
        for packed_rgb in [false, true] {
            let mut walk = walk::QualityWalk::new(90, std::time::Duration::from_millis(33), true);
            let mut stream = Stream::new(w, h, 90, Chroma::Yuv444, 2).expect("a stream");
            stream.set_quality(QUALITY_MIN);
            let mut frames = vec![Vec::new()];
            if packed_rgb { stream.read_rgb(&rgb, None) } else { stream.read_bgrx(&bgrx, wu * 4, None) }.expect("a read");
            stream.encode(false, &mut frames[0]).expect("an encode");
            walk.sent(stream.coarsest(), start);
            assert_eq!((stream.coarsest(), walk.settle_at()), (QUALITY_MIN, Some(start + walk::SETTLE_IDLE)));
            let coarse = error(&decode_chain(&frames, wu, hu));

            walk.settle(start + walk::SETTLE_IDLE);
            let mut settle = Vec::new();
            // Nothing is read for it: the planes are the quiet desktop.
            assert!(!stream.settle(walk.ceiling(), false, &mut settle).expect("a settle"), "the settle cost a keyframe");
            frames.push(settle);
            walk.sent(stream.coarsest(), start + walk::SETTLE_IDLE);
            assert_eq!((stream.quality(), stream.coarsest(), walk.settle_at()), (QUALITY_MIN, 90, None), "the settle left the dial or the picture where it was not");
            let settled = error(&decode_chain(&frames, wu, hu));
            assert!(settled < coarse / 4.0, "the settle left the picture at error {settled:.2} of {coarse:.2}");

            // The motion after it is at what the link bears, and where it is.
            let mut moved = Vec::new();
            stream.read_rgb(&rgb, Some(&[Rect { x: 0, y: 0, width: 16, height: 16 }])).expect("a read");
            stream.encode(false, &mut moved).expect("an encode");
            walk.sent(stream.coarsest(), start + walk::SETTLE_IDLE);
            assert_eq!((stream.coarsest(), walk.settle_at().is_some()), (QUALITY_MIN, true), "a coarse frame after the settle owes none");

            // That frame coded its corner and left the rest as the settle
            // had it, and a frame codes what was read for it and not what
            // was read for the one before: a finer frame elsewhere leaves
            // the corner coarse, and a finer one of the corner leaves nothing.
            stream.set_quality(90);
            stream.read_rgb(&rgb, Some(&[Rect { x: 64, y: 64, width: 16, height: 16 }])).expect("a read");
            stream.encode(false, &mut Vec::new()).expect("an encode");
            assert_eq!(stream.coarsest(), QUALITY_MIN, "a frame coded what was read for the one before it");
            stream.read_rgb(&rgb, Some(&[Rect { x: 0, y: 0, width: 16, height: 16 }])).expect("a read");
            stream.encode(false, &mut Vec::new()).expect("an encode");
            assert_eq!(stream.coarsest(), 90, "a frame of one corner coded more than it");
            stream.set_quality(QUALITY_MIN);

            // A settle that is to be a keyframe is one, at the settle's quality.
            let mut key = Vec::new();
            assert!(stream.settle(90, true, &mut key).expect("a settle"));
            assert_eq!((stream.quality(), stream.coarsest(), frame_header(&key).map(|header| header.keyframe)), (QUALITY_MIN, 90, Some(true)));

            assert_eq!(stream.quality(), QUALITY_MIN);
        }
    }

    /// The mechanism a quality walk rests on: quality can be given up
    /// mid-stream without spending a keyframe to do it.
    #[test]
    fn the_quality_moves_on_a_live_encoder_without_a_keyframe() {
        let mut encoder = Encoder::new(320, 240, 90, Chroma::Yuv444, 2).expect("an encoder");
        assert_eq!(encoder.quality(), 90);
        let frame = |encoder: &mut Encoder, step| encode(encoder, &picture(320, 240, &moving(step)), false);
        frame(&mut encoder, 0);
        let fine: usize = (1..5).map(|step| frame(&mut encoder, step).0.len()).sum();

        encoder.set_quality(QUALITY_MIN);
        assert_eq!(encoder.quality(), QUALITY_MIN);
        let coarse: Vec<_> = (5..9).map(|step| frame(&mut encoder, step)).collect();
        assert!(coarse.iter().map(|(data, _)| data.len()).sum::<usize>() < fine, "the quantizer did not reach the running encoder");
        assert!(!coarse.iter().any(|(_, keyframe)| *keyframe), "moving the quantizer cost a keyframe, which is what this avoids");

        encoder.set_quality(0);
        assert_eq!(encoder.quality(), QUALITY_MIN, "clamped, not wrapped");
    }

    /// The speed reaches the running encoder without a keyframe, and the chain
    /// across the change still decodes to its picture.
    #[test]
    fn the_speed_moves_on_a_live_encoder_without_a_keyframe() {
        let mut encoder = Encoder::new(320, 240, 90, Chroma::Yuv444, 2).expect("an encoder");
        assert_eq!(encoder.speed(), Speed::Usual);
        let frame = |encoder: &mut Encoder, step| encode(encoder, &picture(320, 240, &moving(step)), false);
        let mut frames = vec![frame(&mut encoder, 0).0];
        for speed in [Speed::Fastest, Speed::Usual] {
            encoder.set_speed(speed);
            assert_eq!(encoder.speed(), speed);
            for step in 1..4 {
                let (data, keyframe) = frame(&mut encoder, step);
                assert!(!keyframe, "moving the speed cost a keyframe");
                frames.push(data);
            }
        }
        let decoded = decode_chain(&frames, 320, 240);
        // Inside the block's last place, and the background far from it.
        let block = rgb_at(&decoded, 320, 3 * 3 + 50, 3 * 3 + 40);
        let ground = rgb_at(&decoded, 320, 300, 200);
        assert!(block.iter().all(|c| c.abs_diff(230) <= 8), "the block decoded as {block:?}");
        assert!(ground.iter().zip([30u8, 60, 90]).all(|(c, want)| c.abs_diff(want) <= 8), "the background decoded as {ground:?}");
    }

    /// An odd side is carried as it is, at either chroma, and decodes at its
    /// own size, every sample to its last row and column within 3 levels of the
    /// planes the encoder was given: the finest quantizer still moves a flat
    /// block by a level or two (luma 126 to 127 on x86_64, 125 to 127 on arm64).
    ///
    /// Judged on the planes rather than on RGB: the conversions either side of
    /// the codec are the `yuv` crate's, whose AVX2, NEON and scalar paths round
    /// differently, so a colour read back through them lands a CPU-dependent
    /// few levels further from where it started however well the codec did.
    #[test]
    fn an_odd_sized_picture_encodes_and_decodes_at_its_own_size() {
        let (w, h) = (33u16, 17u16);
        for chroma in [Chroma::Yuv444, Chroma::Yuv420] {
            let mut source = Picture::new(w, h, chroma, 1).expect("a picture");
            source.read_rgb(&flat(w, h, [40, 180, 90])).expect("its own picture");
            let mut encoder = Encoder::new(w, h, QUALITY_MAX, chroma, 1).expect("an encoder");
            let (frame, _) = encode(&mut encoder, &source, false);
            let mut decoder = Decoder::new(1).expect("a decoder");
            let decoded = decoder.decode(&frame).expect("a decode");
            assert_eq!((decoded.size(), decoded.chroma()), ((33, 17), chroma));
            let (given, got, given_strides, got_strides) = (source.planes(), decoded.planes(), source.strides(), decoded.strides());
            for plane in 0..3 {
                let (pw, ph) = if plane == 0 { (33, 17) } else { (given_strides[1], given[1].len() / given_strides[1]) };
                assert_eq!(got[plane].len(), (ph - 1) * got_strides[plane] + pw, "plane {plane} at {chroma:?}");
                for (x, y) in (0..ph).flat_map(|y| (0..pw).map(move |x| (x, y))) {
                    let (want, have) = (given[plane][y * given_strides[plane] + x], got[plane][y * got_strides[plane] + x]);
                    assert!(want.abs_diff(have) <= 3, "plane {plane} at {x},{y} at {chroma:?}: {have}, given {want}");
                }
            }
            let mut out = vec![0; 33 * 17 * 4];
            decoded.write_bgrx(&mut out, 33 * 4).expect("a picture that fits");
            // The last pixel, of the odd row and column, is the colour still.
            let last = rgb_at(&out, 33, 32, 16);
            assert!(last.iter().zip([40u8, 180, 90]).all(|(c, want)| c.abs_diff(want) <= 6), "the last pixel at {chroma:?} decoded as {last:?}");
            // 1919×1079 as a desktop is.
            let mut source = Picture::new(1919, 1079, chroma, 2).expect("a picture");
            source.read_rgb(&flat(1919, 1079, [90, 90, 90])).expect("its own picture");
            let mut encoder = Encoder::new(1919, 1079, 60, chroma, 2).expect("an encoder");
            assert!(encoder.encode(&source, false, None, &mut Vec::new()).is_ok());
        }
    }

    /// Only the frames that are asked to be keyframes are, and the ones between
    /// decode against them.
    #[test]
    fn keyframes_come_when_asked_and_the_frames_between_decode() {
        let (w, h) = (64u16, 32u16);
        let mut encoder = Encoder::new(w, h, 60, Chroma::Yuv444, 1).expect("an encoder");
        let mut decoder = Decoder::new(1).expect("a decoder");
        let mut out = vec![0; 64 * 32 * 4];
        for step in 0..5usize {
            let mut rgb = flat(w, h, [80, 40, 20]);
            for x in step * 4..step * 4 + 8 {
                for y in 0..32 {
                    rgb[(y * 64 + x) * 3..][..3].copy_from_slice(&[240, 240, 240]);
                }
            }
            let (frame, keyframe) = encode(&mut encoder, &picture(w, h, &rgb), step == 3);
            assert_eq!(keyframe, step == 0 || step == 3, "frame {step}");
            decoder.decode(&frame).expect("a decode").write_bgrx(&mut out, 64 * 4).expect("a picture that fits");
            let lit = &out[(step * 4 + 4) * 4..][..3];
            assert!(lit.iter().all(|&c| c > 200), "frame {step} did not decode to its own picture: {lit:?}");
        }
    }

    /// **Where the picture loss on a desktop stream would be.** At the dial's
    /// finest quantizer a one-pixel coloured glyph stem comes back as it was,
    /// where 4:2:0 would have averaged its one chroma sample with three
    /// background pixels and returned it at a fraction of its colour.
    #[test]
    fn a_stream_keeps_a_one_pixel_stem_its_colour() {
        let (rgb, at) = stems(64, 64);
        let mut encoder = Encoder::new(64, 64, QUALITY_MAX, Chroma::Yuv444, 1).expect("an encoder");
        let (frame, _) = encode(&mut encoder, &picture(64, 64, &rgb), false);
        let decoded = decode_chain(&[frame], 64, 64);
        let worst = at.iter().map(|&(x, y)| rgb_at(&decoded, 64, x, y).iter().zip([255u8, 121, 198]).map(|(a, b)| a.abs_diff(b)).max().unwrap()).max().unwrap();
        assert!(worst <= 24, "a stem pixel came back {worst} code values off at the finest quantizer");
    }

    /// The keyframe header says which matrix and range the pixels were converted
    /// with, so a decoder does not guess — and guesses BT.709 for an HD picture,
    /// which is not what the conversion did.
    #[test]
    fn the_bitstream_declares_bt601_studio_swing() {
        let mut encoder = Encoder::new(64, 64, 60, Chroma::Yuv444, 1).expect("an encoder");
        let (frame, _) = encode(&mut encoder, &picture(64, 64, &flat(64, 64, [200, 30, 30])), false);
        let mut decoder = Decoder::new(1).expect("a decoder");
        let decoded = decoder.decode(&frame).expect("a decode");
        assert!(decoded.declares_bt601_studio_swing());
        assert_eq!(decoded.planes()[1].len(), 63 * decoded.strides()[1] + 64);
    }

    /// A 16×16 grey keyframe at 4:2:0, VP9 profile 0, as libvpx coded one.
    const SUBSAMPLED_KEYFRAME: [u8; 41] = [
        0x82, 0x49, 0x83, 0x42, 0x20, 0x00, 0xf0, 0x00, 0xf6, 0x1a, 0x38, 0x24, 0x1c, 0x18, 0xf8, 0x00, 0x00, 0x20, 0x40, 0x00, 0x22, 0x9b, 0xfd, 0x6c, 0x3c, 0x2f,
        0x2f, 0xc0, 0xaf, 0x42, 0xc5, 0xec, 0xe9, 0x2f, 0x81, 0x85, 0x05, 0xc0, 0xce, 0x9d, 0x00,
    ];

    /// A profile 0 frame libvpx coded elsewhere decodes as the 4:2:0 picture
    /// it is, its chroma planes at half its size.
    #[test]
    fn a_420_frame_decodes_at_its_chroma() {
        assert_eq!(frame_header(&SUBSAMPLED_KEYFRAME), Some(FrameHeader { profile: Chroma::Yuv420.profile(), keyframe: true }));
        let mut decoder = Decoder::new(1).expect("a decoder");
        let decoded = decoder.decode(&SUBSAMPLED_KEYFRAME).expect("a decode");
        assert_eq!((decoded.size(), decoded.chroma()), ((16, 16), Chroma::Yuv420));
        assert_eq!(decoded.planes()[1].len(), 7 * decoded.strides()[1] + 8);
    }

    #[test]
    fn a_wrong_picture_or_frame_is_an_error_rather_than_a_read_past_the_end() {
        let mut encoder = Encoder::new(32, 16, 60, Chroma::Yuv444, 1).expect("an encoder");
        let other = picture(16, 16, &flat(16, 16, [0, 0, 0]));
        assert!(matches!(encoder.encode(&other, false, None, &mut Vec::new()), Err(Error::Mismatch(32, 16, 16, 16))));
        let subsampled = Picture::new(32, 16, Chroma::Yuv420, 1).expect("a picture");
        assert!(matches!(encoder.encode(&subsampled, false, None, &mut Vec::new()), Err(Error::Sampling(Chroma::Yuv444, Chroma::Yuv420))));
        assert!(matches!(Encoder::new(0, 16, 60, Chroma::Yuv444, 1), Err(Error::Empty(0, 16))));
        assert!(matches!(Encoder::new(16, 16, 60, Chroma::Yuv444, 0), Err(Error::Threads(0))));
        assert!(matches!(Decoder::new(0), Err(Error::Threads(0))));

        let (frame, _) = encode(&mut encoder, &picture(32, 16, &flat(32, 16, [0, 0, 0])), false);
        let mut decoder = Decoder::new(1).expect("a decoder");
        assert!(decoder.decode(&[0xFF, 0x00, 0x12]).is_err());
        let decoded = decoder.decode(&frame).expect("a decode");
        let mut out = [0; 10];
        assert!(matches!(decoded.write_bgrx(&mut out, 128), Err(Error::Buffer { .. })));
    }

    /// The walk and a live encoder together, on a link that carries so many
    /// bytes a second: every move of the dial reaches the encoder without a
    /// keyframe, the frames answer it in size and in sharpness, an independent
    /// decoder takes every one of them, and the walk settles where the link
    /// keeps up and comes back to the ceiling once it widens.
    ///
    /// The picture is new every frame, as a video playing on the desktop is, so
    /// a frame's size answers to the quantizer; the link is half of what the
    /// ceiling costs, measured on an encoder of its own. Time is simulated —
    /// the walk takes `now` — so the run takes as long as its encodes.
    #[test]
    fn a_walk_moves_a_live_encoder_to_what_a_link_bears_and_back() {
        use std::collections::VecDeque;
        use std::time::{Duration, Instant};
        use walk::{Pace, QualityWalk, QUALITY_FLOOR};

        const W: u16 = 128;
        const H: u16 = 96;
        const CEILING: u8 = 90;
        /// 30 Hz.
        const FRAME: Duration = Duration::from_micros(33_333);
        /// Each way, so the link's floor is twice this and a frame's time on the
        /// wire: what the walk learns to read as distance rather than queueing.
        const ONE_WAY: Duration = Duration::from_millis(20);

        /// Noise new every frame.
        fn churning(step: u32) -> Picture {
            let rgb: Vec<u8> = (0..u32::from(W) * u32::from(H))
                .flat_map(|at| {
                    let v = ((at.wrapping_mul(2_654_435_761) ^ step.wrapping_mul(40_503)).wrapping_mul(2_246_822_519) >> 24) as u8;
                    [v, v / 2 + 40, 255 - v]
                })
                .collect();
            picture(W, H, &rgb)
        }

        /// A frame as it went out.
        struct Sent {
            quality: u8,
            bytes: usize,
            /// How long it waited behind the frames before it for the wire.
            queued: Duration,
            /// Mean distance of the decoded luma from the luma encoded.
            error: f64,
        }

        struct Session {
            encoder: Encoder,
            decoder: Decoder,
            walk: QualityWalk,
            now: Instant,
            /// When the wire is next free.
            free: Instant,
            /// Fences on their way back: when each arrives, the delivery it
            /// reports, and whether it is a verdict.
            fences: VecDeque<(Instant, Duration, bool)>,
            step: u32,
        }

        impl Session {
            /// `span` of frames onto a link that carries `capacity` bytes a second.
            fn run(&mut self, capacity: f64, span: Duration) -> Vec<Sent> {
                let end = self.now + span;
                let mut sent = Vec::new();
                while self.now < end {
                    while let Some((back, delivery, verdict)) = self.fences.front().copied().filter(|(back, ..)| *back <= self.now) {
                        self.fences.pop_front();
                        if let Some(pace) = self.walk.fenced(delivery, verdict, back) {
                            self.encoder.set_quality(pace.quality);
                        }
                    }
                    assert_eq!(self.encoder.quality(), self.walk.quality(), "the encoder is not coding what the walk holds");

                    let source = churning(self.step);
                    let (frame, keyframe) = encode(&mut self.encoder, &source, false);
                    assert_eq!(keyframe, self.step == 0, "frame {}: a move of the dial cost a keyframe", self.step);
                    let decoded = self.decoder.decode(&frame).expect("a decode");
                    assert_eq!(decoded.size(), (u32::from(W), u32::from(H)));
                    let (given, stride) = (source.planes()[0], source.strides()[0]);
                    let (got, got_stride) = (decoded.planes()[0], decoded.strides()[0]);
                    let distance: u64 = (0..usize::from(H))
                        .flat_map(|y| (0..usize::from(W)).map(move |x| (x, y)))
                        .map(|(x, y)| u64::from(given[y * stride + x].abs_diff(got[y * got_stride + x])))
                        .sum();
                    if keyframe {
                        self.walk.keyframe(self.now);
                    }

                    let queued = self.free.saturating_duration_since(self.now);
                    self.free = self.free.max(self.now) + Duration::from_secs_f64(frame.len() as f64 / capacity);
                    let back = self.free + 2 * ONE_WAY;
                    self.fences.push_back((back, back - self.now, !keyframe));
                    sent.push(Sent { quality: self.encoder.quality(), bytes: frame.len(), queued, error: distance as f64 / f64::from(W) / f64::from(H) });
                    self.step += 1;
                    self.now += self.walk.interval();
                }
                sent
            }
        }

        // A second of delta frames at `quality`, on an encoder of its own.
        let per_second = |quality| {
            let mut encoder = Encoder::new(W, H, quality, Chroma::Yuv444, 2).expect("an encoder");
            (0..31).map(|step| encode(&mut encoder, &churning(step), false).0.len()).skip(1).sum::<usize>() as f64
        };
        let (ceiling, floor) = (per_second(CEILING), per_second(QUALITY_FLOOR));
        let narrow = ceiling / 2.0;
        assert!(floor < narrow / 2.0, "the floor costs {floor} bytes a second against a link of {narrow}: the picture does not answer to the quantizer");

        let start = Instant::now();
        let mut session = Session {
            encoder: Encoder::new(W, H, CEILING, Chroma::Yuv444, 2).expect("an encoder"),
            decoder: Decoder::new(1).expect("a decoder"),
            walk: QualityWalk::new(CEILING, FRAME, true),
            now: start,
            free: start,
            fences: VecDeque::new(),
            step: 0,
        };
        let mean = |frames: &[Sent], of: fn(&Sent) -> f64| frames.iter().map(of).sum::<f64>() / frames.len() as f64;

        // Long enough to settle: the keyframe's hold, the queue the ceiling built
        // behind it drained, a step up refused and walked back.
        let behind = session.run(narrow, Duration::from_secs(25));
        let fine = &behind[1..31];
        let settled = &behind[behind.len() - 150..];
        let quality = session.walk.quality();
        assert!(QUALITY_FLOOR < quality && quality < CEILING, "the walk settled at {quality} on a link half the ceiling's width");
        assert!(!session.walk.slowed(), "the frames did not come back once the quality had been given up");
        assert!(settled.iter().all(|sent| sent.queued < walk::LAG_CLEAR), "the link is still behind five seconds from the end");
        let (fine_bytes, settled_bytes) = (mean(fine, |sent| sent.bytes as f64), mean(settled, |sent| sent.bytes as f64));
        assert!(settled_bytes < fine_bytes * 0.6, "frames at {quality} averaged {settled_bytes:.0} bytes against {fine_bytes:.0} at the ceiling");
        let (fine_error, settled_error) = (mean(fine, |sent| sent.error), mean(settled, |sent| sent.error));
        assert!(settled_error > fine_error * 2.0, "frames at {quality} decoded at error {settled_error:.2} against {fine_error:.2} at the ceiling");

        // Long enough for the hold on the quality the link refused to run out,
        // and the steps up from under it.
        let wide = session.run(ceiling * 4.0, Duration::from_secs(20));
        assert_eq!(session.walk.pace(), Pace { quality: CEILING, interval: FRAME });
        let back = &wide[wide.len() - 30..];
        assert!(back.iter().all(|sent| sent.quality == CEILING));
        let back_error = mean(back, |sent| sent.error);
        assert!(back_error < fine_error * 1.25, "back at the ceiling the frames decoded at error {back_error:.2}, against {fine_error:.2} before");
    }
}
