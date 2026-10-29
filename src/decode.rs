//! H.264 into pictures, for the client's camera: the system's libavcodec,
//! linked dynamically, and nothing else of FFmpeg's.
//!
//! The client encoded the stream — a browser's `VideoEncoder`, Constrained
//! Baseline — so what arrives is one Annex B access unit at a time, parameter
//! sets inline on every keyframe, and what leaves is one planar 4:2:0 picture per
//! unit, copied out of the decoder's frame with its line padding removed. That
//! is PipeWire's `I420`, which is what the camera offers the desktop.
//!
//! The decoder is asked for no delay: one thread, so no frame-threading queue
//! holds pictures back, and `LOW_DELAY`, so a unit is a picture the moment it
//! is decoded. A camera frame late by a frame interval is a video call's lip
//! sync gone.

use std::ptr;

use ffmpeg_sys_next as ff;

/// One H.264 decoder: a libavcodec context, and the packet and frame it
/// reuses for every unit.
pub struct Decoder {
    context: *mut ff::AVCodecContext,
    packet: *mut ff::AVPacket,
    frame: *mut ff::AVFrame,
}

// SAFETY: the pointers are owned by this value alone and libavcodec keeps no
// thread affinity on a context; it is only ever used from one thread at a time.
unsafe impl Send for Decoder {}

/// A decoded picture's geometry. The planes are in the buffer handed to
/// [`Decoder::decode`], one after another, luma first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
}

impl Picture {
    /// The size of a chroma plane: half of each dimension, rounded up.
    pub fn chroma(self) -> (usize, usize) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }

    /// Bytes of the whole picture, planes back to back.
    pub fn bytes(self) -> usize {
        let (cw, ch) = self.chroma();
        self.width * self.height + 2 * cw * ch
    }
}

impl Decoder {
    pub fn new() -> anyhow::Result<Self> {
        // SAFETY: each allocation is checked before use, and a failure part way
        // through frees what came before it through `Drop`.
        unsafe {
            let codec = ff::avcodec_find_decoder(ff::AVCodecID::AV_CODEC_ID_H264);
            anyhow::ensure!(!codec.is_null(), "this libavcodec has no H.264 decoder");
            let decoder = Self { context: ff::avcodec_alloc_context3(codec), packet: ff::av_packet_alloc(), frame: ff::av_frame_alloc() };
            anyhow::ensure!(
                !decoder.context.is_null() && !decoder.packet.is_null() && !decoder.frame.is_null(),
                "libavcodec could not allocate an H.264 decoder"
            );
            (*decoder.context).thread_count = 1;
            (*decoder.context).flags |= ff::AV_CODEC_FLAG_LOW_DELAY as i32;
            check(ff::avcodec_open2(decoder.context, codec, ptr::null_mut()), "opening the H.264 decoder")?;
            Ok(decoder)
        }
    }

    /// Decode one access unit. A picture, when the unit completed one, is
    /// written to `out` — which is resized to hold it — and its geometry
    /// returned; a unit that completed none returns `None`. An error is a unit
    /// the decoder could not take, after which the stream wants a keyframe.
    pub fn decode(&mut self, unit: &[u8], out: &mut Vec<u8>) -> anyhow::Result<Option<Picture>> {
        let len = i32::try_from(unit.len()).map_err(|_| anyhow::anyhow!("an access unit of {} bytes", unit.len()))?;
        // SAFETY: `av_new_packet` allocates `len` bytes plus the input padding
        // libavcodec reads past the end, and the copy stays inside the first
        // `len`. The frame's planes are read only inside the geometry and line
        // sizes the decoder reported for them.
        unsafe {
            check(ff::av_new_packet(self.packet, len), "allocating a packet")?;
            ptr::copy_nonoverlapping(unit.as_ptr(), (*self.packet).data, unit.len());
            let sent = ff::avcodec_send_packet(self.context, self.packet);
            ff::av_packet_unref(self.packet);
            check(sent, "decoding an access unit")?;

            let mut picture = None;
            loop {
                let received = ff::avcodec_receive_frame(self.context, self.frame);
                if received == ff::AVERROR(ff::EAGAIN) {
                    break;
                }
                check(received, "receiving a picture")?;
                let copied = self.copy_frame(out);
                ff::av_frame_unref(self.frame);
                picture = Some(copied?);
            }
            Ok(picture)
        }
    }

    /// Copy the current frame's three planes into `out`, dropping each line's
    /// padding. Only 4:2:0 at eight bits is taken, which is every stream a
    /// Constrained Baseline encoder can produce.
    unsafe fn copy_frame(&self, out: &mut Vec<u8>) -> anyhow::Result<Picture> {
        // SAFETY: the caller has a frame the decoder just filled.
        unsafe {
            let frame = &*self.frame;
            let format = frame.format;
            anyhow::ensure!(
                format == ff::AVPixelFormat::AV_PIX_FMT_YUV420P as i32 || format == ff::AVPixelFormat::AV_PIX_FMT_YUVJ420P as i32,
                "the decoder produced pixel format {format}, not 4:2:0"
            );
            let picture = Picture { width: usize::try_from(frame.width)?, height: usize::try_from(frame.height)? };
            let (cw, ch) = picture.chroma();
            out.clear();
            out.reserve(picture.bytes());
            for (plane, (width, height)) in [(picture.width, picture.height), (cw, ch), (cw, ch)].into_iter().enumerate() {
                let stride = usize::try_from(frame.linesize[plane])?;
                anyhow::ensure!(stride >= width, "plane {plane} has lines of {stride} bytes for a width of {width}");
                for row in 0..height {
                    let line = frame.data[plane].add(row * stride);
                    out.extend_from_slice(std::slice::from_raw_parts(line, width));
                }
            }
            Ok(picture)
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: each free takes a pointer this value owns, and accepts null.
        unsafe {
            ff::av_frame_free(&mut self.frame);
            ff::av_packet_free(&mut self.packet);
            ff::avcodec_free_context(&mut self.context);
        }
    }
}

/// A libavcodec return value as a result: negative is an error code.
fn check(code: i32, what: &str) -> anyhow::Result<()> {
    if code >= 0 {
        return Ok(());
    }
    let mut text = [0 as std::ffi::c_char; 128];
    // SAFETY: the buffer is its stated length, and av_strerror terminates it.
    let message = unsafe {
        ff::av_strerror(code, text.as_mut_ptr(), text.len());
        std::ffi::CStr::from_ptr(text.as_ptr()).to_string_lossy().into_owned()
    };
    anyhow::bail!("{what}: {message} ({code})")
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: usize = 64;
    const HEIGHT: usize = 48;

    /// The picture every test stream starts from: a luma ramp across, chroma
    /// flat, so a decoded copy can be compared value for value.
    fn luma(x: usize, y: usize) -> u8 {
        (16 + x * 3 + y) as u8
    }

    /// An H.264 stream made by an encoder independent of the decoder under test:
    /// libx264, which Debian's libavcodec carries. `frames` pictures, the first
    /// a keyframe.
    fn encode(frames: usize) -> Vec<Vec<u8>> {
        // SAFETY: test code driving libavcodec's documented encode loop, each
        // allocation checked and freed at the end.
        unsafe {
            let name = std::ffi::CString::new("libx264").unwrap();
            let codec = ff::avcodec_find_encoder_by_name(name.as_ptr());
            assert!(!codec.is_null(), "libavcodec was built without libx264");
            let mut context = ff::avcodec_alloc_context3(codec);
            (*context).width = WIDTH as i32;
            (*context).height = HEIGHT as i32;
            (*context).pix_fmt = ff::AVPixelFormat::AV_PIX_FMT_YUV420P;
            (*context).time_base = ff::AVRational { num: 1, den: 15 };
            (*context).gop_size = 15;
            (*context).max_b_frames = 0;
            let mut options = ptr::null_mut();
            // Baseline, which is what a browser sends and which cannot be
            // lossless, at a quantizer low enough to compare within a tolerance.
            for (key, value) in [("profile", "baseline"), ("tune", "zerolatency"), ("qp", "10")] {
                let (key, value) = (std::ffi::CString::new(key).unwrap(), std::ffi::CString::new(value).unwrap());
                ff::av_dict_set(&mut options, key.as_ptr(), value.as_ptr(), 0);
            }
            check(ff::avcodec_open2(context, codec, &mut options), "opening libx264").unwrap();
            ff::av_dict_free(&mut options);

            let mut frame = ff::av_frame_alloc();
            (*frame).width = WIDTH as i32;
            (*frame).height = HEIGHT as i32;
            (*frame).format = ff::AVPixelFormat::AV_PIX_FMT_YUV420P as i32;
            check(ff::av_frame_get_buffer(frame, 0), "allocating a frame").unwrap();
            let mut packet = ff::av_packet_alloc();
            let mut units = Vec::new();
            let drain = |units: &mut Vec<Vec<u8>>| loop {
                let got = ff::avcodec_receive_packet(context, packet);
                if got == ff::AVERROR(ff::EAGAIN) || got == ff::AVERROR_EOF {
                    break;
                }
                check(got, "receiving a packet").unwrap();
                units.push(std::slice::from_raw_parts((*packet).data, (*packet).size as usize).to_vec());
                ff::av_packet_unref(packet);
            };
            for index in 0..frames {
                check(ff::av_frame_make_writable(frame), "a writable frame").unwrap();
                for plane in 0..3 {
                    let (w, h) = if plane == 0 { (WIDTH, HEIGHT) } else { (WIDTH / 2, HEIGHT / 2) };
                    let stride = (*frame).linesize[plane] as usize;
                    for y in 0..h {
                        for x in 0..w {
                            *(*frame).data[plane].add(y * stride + x) = if plane == 0 { luma(x, y) } else { 128 };
                        }
                    }
                }
                (*frame).pts = index as i64;
                check(ff::avcodec_send_frame(context, frame), "encoding a frame").unwrap();
                drain(&mut units);
            }
            check(ff::avcodec_send_frame(context, ptr::null()), "flushing the encoder").unwrap();
            drain(&mut units);
            ff::av_packet_free(&mut packet);
            ff::av_frame_free(&mut frame);
            ff::avcodec_free_context(&mut context);
            units
        }
    }

    #[test]
    fn a_stream_decodes_to_the_pictures_it_was_encoded_from() {
        let units = encode(3);
        assert_eq!(units.len(), 3, "one unit a picture at zero latency");
        let mut decoder = Decoder::new().unwrap();
        let mut out = Vec::new();
        for unit in &units {
            let picture = decoder.decode(unit, &mut out).unwrap().expect("a picture for every unit");
            assert_eq!(picture, Picture { width: WIDTH, height: HEIGHT });
            assert_eq!(out.len(), picture.bytes());
            // Lossy, but at qp 10 a smooth ramp comes back within a few levels,
            // and a plane copied with the wrong stride or offset would not.
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let got = out[y * WIDTH + x];
                    assert!(got.abs_diff(luma(x, y)) <= 8, "luma at {x},{y}: {got}, not {}", luma(x, y));
                }
            }
            assert!(out[WIDTH * HEIGHT..].iter().all(|&c| c.abs_diff(128) <= 4), "flat chroma");
        }
    }

    /// A delta with no keyframe before it is not a picture the decoder can
    /// make, and the decoder is still usable afterwards: the keyframe that
    /// follows decodes.
    #[test]
    fn a_stream_joined_mid_gop_recovers_at_the_next_keyframe() {
        let units = encode(2);
        let mut decoder = Decoder::new().unwrap();
        let mut out = Vec::new();
        let _ = decoder.decode(&units[1], &mut out);
        let picture = decoder.decode(&units[0], &mut out).unwrap();
        assert_eq!(picture, Some(Picture { width: WIDTH, height: HEIGHT }));
    }

    #[test]
    fn an_odd_picture_rounds_its_chroma_up() {
        let picture = Picture { width: 5, height: 3 };
        assert_eq!(picture.chroma(), (3, 2));
        assert_eq!(picture.bytes(), 15 + 2 * 6);
    }
}
