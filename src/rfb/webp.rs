//! The tiles encoding's stills on a list that holds its dial: a rectangle of
//! the desktop as a lossless WebP ([`wlshare_proto::tiles::KIND_WEBP`]).
//!
//! The coder is libwebp, compiled from `libwebp-sys`'s own copy of its source
//! and linked in, for the target's baseline. Only its encoder is spoken to,
//! and only here: the picture is handed over as the framebuffer's own
//! `B, G, R, X` rows, read in place, and the file is written straight onto
//! the update's bytes. What it makes of the pixels — a palette where they
//! are few, the transforms, the colour cache and the back references
//! otherwise — is its own choice at the effort [`LEVEL`] names; the tests
//! read every one back with `image-webp`'s decoder, plain Rust that shares
//! nothing with it.

use std::ffi::{c_int, c_void};

use libwebp_sys::{
    WebPConfig, WebPConfigLosslessPreset, WebPEncode, WebPEncodingError, WebPPicture,
    WebPPictureFree, WebPPictureImportBGRX, WebPValidateConfig,
};
use thiserror::Error;

/// libwebp's lossless preset, 0 to 9, the fastest first: the effort a still
/// is coded with. The second is the knee: over a session at the desk it is
/// three quarters of the fastest's bytes for a fifth more of the encoder's
/// time, and the ones past it a tenth fewer at most for a tenth to a third
/// more again (measured in `docs/tiles-performance.md`).
const LEVEL: c_int = 1;

/// The longest side a WebP has.
const MAX_SIDE: u16 = 16383;

/// Why a rectangle could not be coded.
#[derive(Debug, Error)]
pub enum WebpError {
    #[error("a {0}x{1} rectangle is no WebP: a side is 1 to {MAX_SIDE}")]
    Size(u16, u16),
    #[error("{0} bytes of pixels are short of {1}x{2} rows {3} bytes apart")]
    Short(usize, u16, u16, usize),
    #[error("libwebp's configuration was refused")]
    Config,
    #[error("libwebp could not take the pixels: {0:?}")]
    Import(WebPEncodingError),
    #[error("libwebp could not code the picture: {0:?}")]
    Encode(WebPEncodingError),
}

/// libwebp's writer: the bytes appended to the `Vec` the picture carries.
unsafe extern "C" fn append(data: *const u8, len: usize, picture: *const WebPPicture) -> c_int {
    // SAFETY: libwebp calls this with the picture `encode` handed it, whose
    // `custom_ptr` is the `Vec` that call borrows for as long as it runs, and
    // with `len` bytes at `data`.
    unsafe {
        let out = &mut *(*picture).custom_ptr.cast::<Vec<u8>>();
        out.extend_from_slice(std::slice::from_raw_parts(data, len));
    }
    1
}

/// The `w`×`h` picture at the start of `pixels` — `B, G, R, X`, rows `stride`
/// bytes apart — as a lossless WebP appended to `out`, which an error leaves
/// as it was.
pub fn encode(
    pixels: &[u8],
    stride: usize,
    w: u16,
    h: u16,
    out: &mut Vec<u8>,
) -> Result<(), WebpError> {
    if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
        return Err(WebpError::Size(w, h));
    }
    let needed = (usize::from(h) - 1) * stride + usize::from(w) * 4;
    if stride < usize::from(w) * 4 || pixels.len() < needed {
        return Err(WebpError::Short(pixels.len(), w, h, stride));
    }
    let mut config = WebPConfig::new().map_err(|()| WebpError::Config)?;
    // SAFETY: `config` is one libwebp initialised.
    if unsafe { WebPConfigLosslessPreset(&mut config, LEVEL) } == 0
        || unsafe { WebPValidateConfig(&config) } == 0
    {
        return Err(WebpError::Config);
    }
    let mut picture = WebPPicture::new().map_err(|()| WebpError::Config)?;
    picture.use_argb = 1;
    picture.width = c_int::from(w);
    picture.height = c_int::from(h);
    let from = out.len();
    // SAFETY: the picture is one libwebp initialised, with its size set; the
    // import reads `h` rows of `w` pixels `stride` apart, which `pixels` was
    // checked to hold, into memory the picture owns until it is freed below.
    // The writer appends to `out`, borrowed for the whole of the encode.
    let coded = unsafe {
        let stride =
            c_int::try_from(stride).map_err(|_| WebpError::Short(pixels.len(), w, h, stride))?;
        if WebPPictureImportBGRX(&mut picture, pixels.as_ptr(), stride) == 0 {
            Err(WebpError::Import(picture.error_code))
        } else {
            picture.writer = Some(append);
            picture.custom_ptr = std::ptr::from_mut(out).cast::<c_void>();
            if WebPEncode(&config, &mut picture) == 0 {
                Err(WebpError::Encode(picture.error_code))
            } else {
                Ok(())
            }
        }
    };
    // SAFETY: the picture is libwebp's, freed once, after its last use.
    unsafe { WebPPictureFree(&mut picture) };
    if coded.is_err() {
        out.truncate(from);
    }
    coded
}

/// The tests' reader, here and the tiles encoder's.
#[cfg(test)]
pub mod decoder {
    /// A WebP read back by `image-webp`: its size, and its pixels as packed
    /// RGB.
    pub fn decode(webp: &[u8]) -> (u32, u32, Vec<u8>) {
        let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(webp)).expect("a WebP");
        assert!(!decoder.is_lossy(), "a lossless WebP");
        assert!(!decoder.has_alpha(), "with no alpha");
        let (w, h) = decoder.dimensions();
        let mut rgb = vec![0; decoder.output_buffer_size().expect("a size")];
        decoder.read_image(&mut rgb).expect("its pixels");
        (w, h, rgb)
    }
}

#[cfg(test)]
mod tests {
    use super::decoder::decode;
    use super::*;

    /// `B, G, R, X` rows of `w` pixels as packed RGB.
    fn rgb(pixels: &[u8], stride: usize, w: u16, h: u16) -> Vec<u8> {
        (0..usize::from(h))
            .flat_map(|row| {
                pixels[row * stride..][..usize::from(w) * 4]
                    .as_chunks::<4>()
                    .0
            })
            .flat_map(|px| [px[2], px[1], px[0]])
            .collect()
    }

    /// Pixels no palette holds, the padding byte set, which is no alpha.
    fn noise(w: u16, h: u16) -> Vec<u8> {
        let mut state = 0x9E37_79B9u32;
        (0..usize::from(w) * usize::from(h) * 4)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect()
    }

    /// Every pixel comes back as it went in, whatever the padding byte holds:
    /// noise, a few colours, one colour, and a rectangle inside a wider
    /// picture, read in place.
    #[test]
    fn a_picture_is_read_back_exact() {
        let (w, h) = (200u16, 64u16);
        let stride = usize::from(w) * 4;
        let busy = noise(w, h);
        let few: Vec<u8> = (0..usize::from(w) * usize::from(h))
            .flat_map(|i| [(i % 7) as u8 * 30, (i % 3) as u8 * 90, 5, 0xFF])
            .collect();
        let flat = [10, 20, 30, 0].repeat(usize::from(w) * usize::from(h));
        for pixels in [&busy, &few, &flat] {
            let mut out = vec![0xAA];
            encode(pixels, stride, w, h, &mut out).unwrap();
            assert_eq!(out[0], 0xAA, "appended");
            let (rw, rh, read) = decode(&out[1..]);
            assert_eq!((rw, rh), (u32::from(w), u32::from(h)));
            assert!(read == rgb(pixels, stride, w, h), "every pixel exact");
        }
        // 64×32 from (100, 16) of the noise.
        let inside = &busy[16 * stride + 100 * 4..];
        let mut out = Vec::new();
        encode(inside, stride, 64, 32, &mut out).unwrap();
        let (rw, rh, read) = decode(&out);
        assert_eq!((rw, rh), (64, 32));
        assert!(read == rgb(inside, stride, 64, 32), "the rectangle's own");
        // One pixel.
        out.clear();
        encode(&[1, 2, 3, 0], 4, 1, 1, &mut out).unwrap();
        assert_eq!(decode(&out), (1, 1, vec![3, 2, 1]));
    }

    /// A few colours cost far less than their pixels, and noise no more than
    /// a little over them.
    #[test]
    fn a_picture_of_few_colours_is_small() {
        let (w, h) = (256u16, 64u16);
        let stride = usize::from(w) * 4;
        let few: Vec<u8> = (0..usize::from(w) * usize::from(h))
            .flat_map(|i| [(i / 40 % 4) as u8 * 60, 0, 200, 0])
            .collect();
        let mut out = Vec::new();
        encode(&few, stride, w, h, &mut out).unwrap();
        assert!(out.len() < 1024, "{} bytes of four colours", out.len());
        out.clear();
        encode(&noise(w, h), stride, w, h, &mut out).unwrap();
        let raw = usize::from(w) * usize::from(h) * 3;
        assert!(out.len() < raw + raw / 16, "{} bytes of noise", out.len());
    }

    /// What is no picture is refused, and nothing is appended.
    #[test]
    fn what_is_no_picture_is_refused() {
        let mut out = vec![7];
        assert!(matches!(
            encode(&[], 0, 0, 4, &mut out),
            Err(WebpError::Size(0, 4))
        ));
        assert!(matches!(
            encode(&[0; 16], 16, 20000, 1, &mut out),
            Err(WebpError::Size(20000, 1))
        ));
        assert!(matches!(
            encode(&[0; 15], 8, 2, 2, &mut out),
            Err(WebpError::Short(15, 2, 2, 8))
        ));
        assert!(matches!(
            encode(&[0; 16], 4, 2, 2, &mut out),
            Err(WebpError::Short(16, 2, 2, 4))
        ));
        assert_eq!(out, [7]);
    }
}
