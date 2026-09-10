//! The standard RFB Cursor pseudo-rectangle used to keep the pointer separate
//! from framebuffer pixels.
//!
//! wlshare cannot observe application-selected Wayland cursor surfaces through
//! wlr-screencopy. It therefore sends one neutral arrow when a client advertises
//! [`crate::ENCODING_CURSOR`]. The client owns positioning from then on: every
//! pointer event it sends already identifies the pointer's current location, so
//! the shape moves locally without waiting for another captured frame.

use crate::msg::rect_header;
use crate::pixel::PixelFormat;
use crate::ENCODING_CURSOR;

const WIDTH: u16 = 12;
const HEIGHT: u16 = 19;

// `B` is the black outline, `W` the white fill, and spaces are transparent.
// Byte-string rows make both dimensions compile-time properties.
const ARROW: [&[u8; WIDTH as usize]; HEIGHT as usize] = [
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
/// negotiated pixel format. Its hotspot is the arrow's top-left pixel.
pub fn cursor_rect(format: &PixelFormat) -> Vec<u8> {
    let mask_stride = usize::from(WIDTH).div_ceil(8);
    let mut rect = Vec::with_capacity(
        12 + usize::from(WIDTH) * usize::from(HEIGHT) * 4 + mask_stride * usize::from(HEIGHT),
    );
    rect.extend_from_slice(&rect_header(0, 0, WIDTH, HEIGHT, ENCODING_CURSOR));

    for row in ARROW {
        for &pixel in row {
            let bgrx = match pixel {
                b'W' => [255, 255, 255, 0],
                b'B' | b' ' => [0, 0, 0, 0],
                _ => unreachable!("the cursor palette is fixed"),
            };
            rect.extend_from_slice(&format.pixel_bytes(format.value(bgrx)));
        }
    }

    let mut mask = vec![0u8; mask_stride * usize::from(HEIGHT)];
    for (y, row) in ARROW.iter().enumerate() {
        for (x, &pixel) in row.iter().enumerate() {
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
        let wire = cursor_rect(&format);

        assert_eq!(u16::from_be_bytes([wire[0], wire[1]]), 0);
        assert_eq!(u16::from_be_bytes([wire[2], wire[3]]), 0);
        assert_eq!(u16::from_be_bytes([wire[4], wire[5]]), 12);
        assert_eq!(u16::from_be_bytes([wire[6], wire[7]]), 19);
        assert_eq!(i32::from_be_bytes([wire[8], wire[9], wire[10], wire[11]]), -239);

        let pixels = &wire[12..12 + 12 * 19 * 4];
        let mask = &wire[12 + 12 * 19 * 4..];
        assert_eq!(wire.len(), 12 + 12 * 19 * 4 + 2 * 19);

        // Black hotspot, transparent black beside it, then a white interior
        // pixel on the third row. The white byte order proves the requested
        // non-native format was used.
        assert_eq!(&pixels[0..4], &[0, 0, 0, 0]);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 0]);
        assert_eq!(&pixels[(2 * 12 + 1) * 4..(2 * 12 + 2) * 4], &[255, 255, 255, 0]);

        // Rows are padded to whole bytes, MSB first. The widest row occupies
        // twelve pixels and leaves the low nibble of its second byte clear.
        assert_eq!(&mask[0..2], &[0b1000_0000, 0]);
        assert_eq!(&mask[2..4], &[0b1100_0000, 0]);
        assert_eq!(&mask[11 * 2..12 * 2], &[0xFF, 0xF0]);
        assert!(mask.as_chunks::<2>().0.iter().all(|row| row[1] & 0x0F == 0));
    }
}
