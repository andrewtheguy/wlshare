//! The video encoding's server side: the framebuffer's pixels in, and each
//! frame out in the rectangle's body the encoding has ([`wlshare_proto::video`]):
//! its length and the access unit.
//!
//! **What a frame holds.** Every frame is one H.264 access unit of one stream
//! over the whole desktop, 8-bit 4:2:0 converted from the framebuffer as
//! BT.601 at studio swing, by the [`H264Coder`] the daemon hands the encoder:
//! its host's hardware encoder, or libx264 where it has none. Each frame is
//! coded at the dial it is handed, the walk's ([`crate::walk`]), from wlshare's
//! dial ([`crate::rfb::QUALITY`]) down: no bitrate, no dropped frames, so every
//! frame is sent at exactly the quality it was asked at, and a step of the walk
//! is the next frame's with no keyframe. What a link that falls behind gives
//! up is quality first and frames after, the session's to pace.
//!
//! The coding itself is the coder's. What this module owns is the
//! framebuffer's pixels in, and the frame framed as
//! [`wlshare_proto::video::frame_header`] lays it out.

use thiserror::Error;

use crate::rfb::{H264Coder, H264Error, H264Stream, Rect};

/// Why a picture could not be encoded.
#[derive(Debug, Error)]
pub enum VideoError {
    #[error("a {0}x{1} desktop has no pixels")]
    Empty(u16, u16),
    /// The coder refused the stream, or the stream its pixels or its frame.
    #[error("coding the video stream: {0}")]
    Codec(H264Error),
    #[error("a frame of {0} bytes is over the {max}-byte ceiling", max = wlshare_proto::MAX_BODY)]
    FrameTooLong(usize),
}

/// One connection's video stream, server side: a stream at one picture size,
/// which converts the framebuffer and codes it. A framebuffer of another size
/// needs another of these, whose first frame is an IDR by construction.
pub struct VideoEncoder {
    stream: Box<dyn H264Stream>,
    size: (u16, u16),
}

impl VideoEncoder {
    /// An encoder for a `width`×`height` picture, on a stream `coder` opens.
    pub fn new(coder: &mut dyn H264Coder, width: u16, height: u16) -> Result<Self, VideoError> {
        if width == 0 || height == 0 {
            return Err(VideoError::Empty(width, height));
        }
        let stream = coder.open(width, height).map_err(VideoError::Codec)?;
        Ok(Self { stream, size: (width, height) })
    }

    /// The picture size this encoder codes.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Read the picture — [`Self::size`] of `B, G, R, X` pixels whose rows
    /// are `stride` bytes apart — for the next frame to carry
    /// ([`Self::encode`]). It is read into the encoder's own planes, so
    /// `pixels` is the framebuffer itself, for as long as the conversion
    /// takes and no longer: nothing of it is kept.
    ///
    /// `changed` is the damage since the picture read before, or `None` for
    /// a picture that may differ anywhere. With it only the rectangles are
    /// read from `pixels`, widened to the 2×2 blocks they touch, which the
    /// framebuffer holds as it holds the rest. An encoder's first read is the
    /// whole picture whatever `changed` says.
    pub fn read(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> Result<(), VideoError> {
        self.stream.read_bgrx(pixels, stride, changed).map_err(VideoError::Codec)
    }

    /// Encode the picture as it was last read at `quality`, the 1–100 dial,
    /// and append the rectangle's body to `out`: the length word and the
    /// access unit. `keyframe` makes it an IDR a decoder can start from; an
    /// encoder's first frame is one either way.
    ///
    /// Returns whether a body was appended: `false`, with `out` as it was,
    /// when the coder made no frame of the picture, since an empty rectangle
    /// is not a frame a client can decode and the pixels are the next frame's
    /// to carry. An error leaves `out` as it was too.
    pub fn encode(&mut self, keyframe: bool, quality: u8, out: &mut Vec<u8>) -> Result<bool, VideoError> {
        let body_at = out.len();
        out.extend_from_slice(&wlshare_proto::video::frame_header(0));
        match self.stream.encode(keyframe, quality, out) {
            Ok(true) => {}
            Ok(false) => {
                out.truncate(body_at);
                return Ok(false);
            }
            Err(e) => {
                out.truncate(body_at);
                return Err(VideoError::Codec(e));
            }
        }
        let len = out.len() - body_at - wlshare_proto::video::FRAME_HEADER_LEN;
        if len > wlshare_proto::MAX_BODY {
            out.truncate(body_at);
            return Err(VideoError::FrameTooLong(len));
        }
        out[body_at..body_at + wlshare_proto::video::FRAME_HEADER_LEN].copy_from_slice(&wlshare_proto::video::frame_header(len as u32));
        Ok(true)
    }
}

/// The half this module owns — the length word, what is
/// left of `out` when nothing or an error comes back, the stream it opens —
/// on a coder that keeps a record of what it was asked. The coding itself is
/// held to libavcodec's decoder in [`crate::h264`]'s tests, through this
/// encoder.
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// What the fake coder was asked: the streams opened, and every read as
    /// what changed and encode as `(keyframe, quality)`, in order.
    #[derive(Default)]
    struct Log {
        opened: Vec<(u16, u16)>,
        reads: Vec<Option<Vec<Rect>>>,
        encodes: Vec<(bool, u8)>,
    }

    /// A coder whose frames are `frame` copied out, an empty one for `None`
    /// coded as no frame, and an error where `fail` is set.
    struct Fake {
        log: Arc<Mutex<Log>>,
        frame: Option<Vec<u8>>,
        fail: bool,
    }

    struct FakeStream {
        log: Arc<Mutex<Log>>,
        frame: Option<Vec<u8>>,
        fail: bool,
    }

    impl H264Coder for Fake {
        fn open(&mut self, width: u16, height: u16) -> Result<Box<dyn H264Stream>, H264Error> {
            self.log.lock().unwrap().opened.push((width, height));
            Ok(Box::new(FakeStream { log: self.log.clone(), frame: self.frame.clone(), fail: self.fail }))
        }
    }

    impl H264Stream for FakeStream {
        fn read_bgrx(&mut self, _pixels: &[u8], _stride: usize, changed: Option<&[Rect]>) -> Result<(), H264Error> {
            self.log.lock().unwrap().reads.push(changed.map(<[Rect]>::to_vec));
            Ok(())
        }

        fn encode(&mut self, keyframe: bool, quality: u8, out: &mut Vec<u8>) -> Result<bool, H264Error> {
            self.log.lock().unwrap().encodes.push((keyframe, quality));
            if self.fail {
                out.push(0xAB);
                return Err("the encoder gave up".into());
            }
            match &self.frame {
                Some(frame) => {
                    out.extend_from_slice(frame);
                    Ok(true)
                }
                None => Ok(false),
            }
        }
    }

    fn fake(frame: Option<Vec<u8>>, fail: bool) -> (Fake, Arc<Mutex<Log>>) {
        let log = Arc::new(Mutex::new(Log::default()));
        (Fake { log: log.clone(), frame, fail }, log)
    }

    /// The body is the length word and the access unit, appended; the stream
    /// is opened at the picture's size; and what was read and asked of it,
    /// the frame's quality among it, is passed through as it came.
    #[test]
    fn a_frame_is_the_length_and_the_access_unit() {
        let unit = vec![0, 0, 0, 1, 0x65, 0x88, 0x84];
        let (mut coder, log) = fake(Some(unit.clone()), false);
        let mut encoder = VideoEncoder::new(&mut coder, 64, 48).unwrap();
        assert_eq!(encoder.size(), (64, 48));
        let damage = [Rect { x: 2, y: 4, width: 8, height: 6 }];
        encoder.read(&[0; 64 * 48 * 4], 64 * 4, Some(&damage)).unwrap();
        let mut out = vec![0xEE];
        assert!(encoder.encode(true, 47, &mut out).unwrap());
        assert_eq!(out[0], 0xEE, "appended, not overwritten");
        assert_eq!(u32::from_be_bytes(out[1..5].try_into().unwrap()) as usize, unit.len(), "the length word is the access unit's");
        assert_eq!(&out[5..], unit);
        let log = log.lock().unwrap();
        assert_eq!(log.opened, [(64, 48)]);
        assert_eq!(log.reads, [Some(damage.to_vec())]);
        assert_eq!(log.encodes, [(true, 47)]);
    }

    /// A desktop of no pixels is refused before a stream is opened.
    #[test]
    fn an_empty_desktop_is_refused() {
        let (mut coder, log) = fake(Some(vec![1]), false);
        assert!(matches!(VideoEncoder::new(&mut coder, 0, 16), Err(VideoError::Empty(0, 16))));
        assert!(log.lock().unwrap().opened.is_empty());
    }

    /// A coder that made no frame, or failed, leaves `out` as it was.
    #[test]
    fn no_frame_or_an_error_leaves_nothing_behind() {
        let (mut coder, _) = fake(None, false);
        let mut encoder = VideoEncoder::new(&mut coder, 16, 16).unwrap();
        let mut out = vec![0xEE];
        assert!(!encoder.encode(false, crate::rfb::QUALITY, &mut out).unwrap(), "no frame");
        assert_eq!(out, [0xEE]);

        let (mut coder, _) = fake(Some(vec![1]), true);
        let mut encoder = VideoEncoder::new(&mut coder, 16, 16).unwrap();
        assert!(matches!(encoder.encode(false, crate::rfb::QUALITY, &mut out), Err(VideoError::Codec(_))));
        assert_eq!(out, [0xEE], "a failed frame left something behind");
    }
}
