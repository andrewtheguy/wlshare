//! The standard RFB Cursor pseudo-rectangle used to keep the pointer separate
//! from framebuffer pixels.
//!
//! wlshare cannot observe application-selected Wayland cursor surfaces through
//! wlr-screencopy. It therefore sends one neutral arrow when a client advertises
//! [`crate::ENCODING_CURSOR`]. The arrow is point-sized in intent but RFB cursor
//! dimensions are framebuffer pixels, so it is rasterized at the output's
//! density. The client owns positioning from then on: every pointer event it
//! sends already identifies the pointer's current location, so the shape moves
//! locally without waiting for another captured frame.

use crate::msg::rect_header;
use crate::pixel::PixelFormat;
use crate::ENCODING_CURSOR;

const BASE_WIDTH: u16 = 12;
const BASE_HEIGHT: u16 = 19;
const MIN_SCALE: f64 = 0.5;
const MAX_SCALE: f64 = 8.0;

// `B` is the black outline, `W` the white fill, and spaces are transparent.
// Byte-string rows make both dimensions compile-time properties.
const ARROW: [&[u8; BASE_WIDTH as usize]; BASE_HEIGHT as usize] = [
    b"B           ",
    b"BB          ",
    b"BWB         ",
    b"BWWB        ",
    b"BWWWB       ",
    b"BWWWWB      ",
    b"BWWWWWB     ",
    b"BWWWWWWB    ",
    b"BWWWWWWWB   ",
    b"BWWWWWWWWB  ",
    b"BWWWWWWWWWB ",
    b"BWWWWBBBBBBB",
    b"BWWBWB      ",
    b"BWB BWB     ",
    b"BB  BWB     ",
    b"B    BWB    ",
    b"     BWB    ",
    b"     BWB    ",
    b"      BB    ",
];

/// A Cursor pseudo-rectangle carrying wlshare's neutral arrow in the client's
/// negotiated pixel format. `scale` is framebuffer pixels per desktop point;
/// its hotspot is the arrow's top-left pixel.
pub fn cursor_rect(format: &PixelFormat, scale: f64) -> Vec<u8> {
    let scale = if scale.is_finite() { scale.clamp(MIN_SCALE, MAX_SCALE) } else { 1.0 };
    let width = (f64::from(BASE_WIDTH) * scale).round() as u16;
    let height = (f64::from(BASE_HEIGHT) * scale).round() as u16;
    let mask_stride = usize::from(width).div_ceil(8);
    let mut rect = Vec::with_capacity(12 + usize::from(width) * usize::from(height) * 4 + mask_stride * usize::from(height));
    rect.extend_from_slice(&rect_header(0, 0, width, height, ENCODING_CURSOR));

    for y in 0..usize::from(height) {
        let source_y = y * usize::from(BASE_HEIGHT) / usize::from(height);
        for x in 0..usize::from(width) {
            let source_x = x * usize::from(BASE_WIDTH) / usize::from(width);
            let pixel = ARROW[source_y][source_x];
            let bgrx = match pixel {
                b'W' => [255, 255, 255, 0],
                b'B' | b' ' => [0, 0, 0, 0],
                _ => unreachable!("the cursor palette is fixed"),
            };
            rect.extend_from_slice(&format.pixel_bytes(format.value(bgrx)));
        }
    }

    let mut mask = vec![0u8; mask_stride * usize::from(height)];
    for y in 0..usize::from(height) {
        let source_y = y * usize::from(BASE_HEIGHT) / usize::from(height);
        for x in 0..usize::from(width) {
            let source_x = x * usize::from(BASE_WIDTH) / usize::from(width);
            let pixel = ARROW[source_y][source_x];
            if pixel != b' ' {
                mask[y * mask_stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    rect.extend_from_slice(&mask);
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_independent_decoder_sees_the_arrow_header_pixels_and_mask() {
        // This format puts colour in the high three bytes. The assertions below
        // decode the wire directly rather than reusing PixelFormat or rect_header.
        let format = PixelFormat {
            big_endian: true,
            red_shift: 24,
            green_shift: 16,
            blue_shift: 8,
            ..PixelFormat::NATIVE
        };
        let wire = cursor_rect(&format, 2.0);

        assert_eq!(u16::from_be_bytes([wire[0], wire[1]]), 0);
        assert_eq!(u16::from_be_bytes([wire[2], wire[3]]), 0);
        assert_eq!(u16::from_be_bytes([wire[4], wire[5]]), 24);
        assert_eq!(u16::from_be_bytes([wire[6], wire[7]]), 38);
        assert_eq!(i32::from_be_bytes([wire[8], wire[9], wire[10], wire[11]]), -239);

        let pixels = &wire[12..12 + 24 * 38 * 4];
        let mask = &wire[12 + 24 * 38 * 4..];
        assert_eq!(wire.len(), 12 + 24 * 38 * 4 + 3 * 38);

        // Black hotspot, transparent black beside it, then a white interior
        // pixel on the third row. The white byte order proves the requested
        // non-native format was used.
        assert_eq!(&pixels[0..4], &[0, 0, 0, 0]);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 0]);
        assert_eq!(&pixels[(4 * 24 + 2) * 4..(4 * 24 + 3) * 4], &[255, 255, 255, 0]);

        // Rows are padded to whole bytes, MSB first. Scaling doubles each source
        // row and column; the widest source row fills all 24 output pixels.
        assert_eq!(&mask[0..3], &[0b1100_0000, 0, 0]);
        assert_eq!(&mask[2 * 3..3 * 3], &[0b1111_0000, 0, 0]);
        assert_eq!(&mask[22 * 3..23 * 3], &[0xFF, 0xFF, 0xFF]);
    }
}
