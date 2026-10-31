//! The lossless tiles' coding: RDP 6.0 bitmap compression, the planar codec
//! of RDP's graphics pipeline ([MS-RDPEGDI] 2.2.2.5.1, [MS-RDPEGFX]
//! `RDPGFX_CODECID_PLANAR`), which a Windows host whose policy asks for a
//! lossless picture draws everything with. What a
//! [`wlshare_proto::tiles::KIND_PLANAR`] rectangle's body is.
//!
//! A rectangle's pixels are cut into a red, a green and a blue plane, and
//! each plane is coded row by row: the top row as it is, every row below as
//! its difference from the row above, a byte's difference folded so that a
//! small step either way is a small byte (`2d`, or `-2d - 1` below zero);
//! each row of those bytes then as segments, a control byte of how many
//! bytes follow as they are and how many times the last of them repeats
//! after. A flat area is a plane of zeroes under its first row, and a row of
//! zeroes is a byte or two. There is no alpha plane (`NA`), no colour loss
//! and no subsampling: what is decoded is the pixels, exact.
//!
//! A rectangle whose segments would come to more than its planes as they
//! are — noise, a photograph's grain — goes as the planes as they are
//! instead, with no `RLE` flag and the padding byte the format ends that
//! form with.
//!
//! Its tests decode with a reader written here from the specification,
//! which shares nothing with the encoder; zDesk's page decodes it with its
//! RDP compositor's planar codec, the one that draws a Windows host's
//! lossless pipeline.

/// The format header's `RLE`: the planes are differences of rows, run-length coded.
const RLE: u8 = 0x10;
/// The format header's `NA`: no alpha plane.
const NO_ALPHA: u8 = 0x20;

/// The most bytes a segment carries as they are, and the most a short run
/// repeats: a control byte's nibble.
const NIBBLE: usize = 15;
/// The shortest run worth a segment of its own: one of one or two is the
/// long run's form, and costs no less than the bytes as they are.
const RUN_LEAST: usize = 3;

/// Append to `out` the planar bitmap of the `w`×`h` rectangle of `pixels`,
/// `B, G, R, X` whose rows are `stride` bytes apart from the rectangle's
/// top-left, top row first. `rows` is working space, kept from one call to
/// the next.
pub fn encode(pixels: &[u8], stride: usize, w: u16, h: u16, rows: &mut Rows, out: &mut Vec<u8>) {
    let (w, h) = (usize::from(w), usize::from(h));
    debug_assert!(w > 0 && h > 0 && pixels.len() >= (h - 1) * stride + w * 4);
    let from = out.len();
    out.push(RLE | NO_ALPHA);
    for plane in &mut rows.coded {
        plane.clear();
    }
    rows.diff.iter_mut().for_each(|row| {
        row.clear();
        row.resize(w, 0);
    });
    for y in 0..h {
        let row = &pixels[y * stride..y * stride + w * 4];
        let above = (y > 0).then(|| &pixels[(y - 1) * stride..(y - 1) * stride + w * 4]);
        // Red, green and blue are the pixel's third, second and first bytes.
        for (plane, byte) in [2usize, 1, 0].into_iter().enumerate() {
            let diff = &mut rows.diff[plane];
            match above {
                None => {
                    for (d, px) in diff.iter_mut().zip(row.as_chunks::<4>().0) {
                        *d = px[byte];
                    }
                }
                Some(above) => {
                    for ((d, px), up) in diff.iter_mut().zip(row.as_chunks::<4>().0).zip(above.as_chunks::<4>().0) {
                        *d = fold(px[byte].wrapping_sub(up[byte]));
                    }
                }
            }
            scanline(diff, &mut rows.coded[plane]);
        }
    }
    let coded: usize = rows.coded.iter().map(Vec::len).sum();
    if coded < 3 * w * h + 1 {
        for plane in &rows.coded {
            out.extend_from_slice(plane);
        }
        return;
    }
    // The planes as they are, and the padding byte that form ends with.
    out.truncate(from);
    out.push(NO_ALPHA);
    for byte in [2usize, 1, 0] {
        for y in 0..h {
            out.extend(pixels[y * stride..y * stride + w * 4].as_chunks::<4>().0.iter().map(|px| px[byte]));
        }
    }
    out.push(0);
}

/// The encoder's working space: each plane's coded rows, and the row of
/// differences being coded.
#[derive(Default)]
pub struct Rows {
    coded: [Vec<u8>; 3],
    diff: [Vec<u8>; 3],
}

/// A row's difference from the row above, folded: the sign the bottom bit.
fn fold(d: u8) -> u8 {
    if d & 0x80 == 0 { d << 1 } else { ((!d) << 1) | 1 }
}

/// One row as segments: bytes as they are, then a run of the last of them —
/// or, where none came before in the row, of zero.
fn scanline(row: &[u8], out: &mut Vec<u8>) {
    // What a run repeats here: the last byte sent as it is, zero at the row's start.
    let mut last = 0u8;
    let (mut literal_from, mut at) = (0, 0);
    while at < row.len() {
        let value = if at > literal_from { row[at - 1] } else { last };
        let run = row[at..].iter().take_while(|&&b| b == value).count();
        if run >= RUN_LEAST {
            segments(&row[literal_from..at], run, value, out);
            last = value;
            at += run;
            literal_from = at;
        } else {
            at += 1;
        }
    }
    if literal_from < row.len() {
        segments(&row[literal_from..], 0, 0, out);
    }
}

/// `literals` as they are, then a run of `run` of `value`, the last of them
/// or, where there are none, what a run repeats there.
fn segments(mut literals: &[u8], mut run: usize, value: u8, out: &mut Vec<u8>) {
    while literals.len() > NIBBLE {
        out.push((NIBBLE as u8) << 4);
        out.extend_from_slice(&literals[..NIBBLE]);
        literals = &literals[NIBBLE..];
    }
    // The bytes left, with as much of the run as a short one carries.
    let short = if run <= NIBBLE { run } else { 0 };
    if !literals.is_empty() || (short > 0) {
        out.push(((literals.len() as u8) << 4) | short as u8);
        out.extend_from_slice(literals);
        run -= short;
    }
    while run > 0 {
        let take = match run {
            32.. => run.min(32 + NIBBLE),
            16.. => run,
            RUN_LEAST.. => run,
            _ => {
                // One or two of the byte again: as they are, which is what a
                // run of them would have drawn.
                out.push((run as u8) << 4);
                out.extend(std::iter::repeat_n(value, run));
                return;
            }
        };
        out.push(match take {
            32.. => (((take - 32) as u8) << 4) | 2,
            16.. => (((take - 16) as u8) << 4) | 1,
            _ => take as u8,
        });
        run -= take;
    }
}

// The examples draw with it too; the daemon never decodes.
#[allow(dead_code)]
pub mod decoder {
    //! A planar bitmap read back as the specification lays it out
    //! ([MS-RDPEGDI] 2.2.2.5.1 and its RLE planes, 2.2.2.5.1.1–3), written
    //! for the tests from the text and not from the encoder; the examples'
    //! pages draw with it too.

    /// The `w`×`h` pixels of `bitmap`, `R, G, B` a pixel, top row first, and
    /// the bytes it took; `Err` where it is not a whole lossless bitmap of
    /// that size with no alpha plane.
    pub fn decode(bitmap: &[u8], w: usize, h: usize) -> Result<(Vec<u8>, usize), String> {
        let header = *bitmap.first().ok_or("no format header")?;
        if header & 0x07 != 0 || header & 0x08 != 0 {
            return Err(format!("format header {header:#04x}: colour loss or subsampling"));
        }
        if header & 0x20 == 0 {
            return Err("an alpha plane".into());
        }
        if header & 0xC0 != 0 {
            return Err(format!("reserved bits in {header:#04x}"));
        }
        let mut planes = vec![vec![0u8; w * h]; 3];
        let mut at = 1;
        if header & 0x10 == 0 {
            for plane in &mut planes {
                plane.copy_from_slice(bitmap.get(at..at + w * h).ok_or("short raw plane")?);
                at += w * h;
            }
            // The padding byte.
            bitmap.get(at).ok_or("no padding byte")?;
            at += 1;
        } else {
            for plane in &mut planes {
                for y in 0..h {
                    let mut x = 0;
                    let mut previous = 0u8;
                    while x < w {
                        let control = *bitmap.get(at).ok_or("short row")?;
                        at += 1;
                        let (mut raw, mut run) = ((control >> 4) as usize, (control & 0x0F) as usize);
                        if run == 1 {
                            run = 16 + raw;
                            raw = 0;
                        } else if run == 2 {
                            run = 32 + raw;
                            raw = 0;
                        }
                        if raw + run == 0 {
                            return Err("an empty segment".into());
                        }
                        if x + raw + run > w {
                            return Err(format!("a segment past the row at {x} of {w}"));
                        }
                        for _ in 0..raw {
                            previous = *bitmap.get(at).ok_or("short literals")?;
                            at += 1;
                            plane[y * w + x] = previous;
                            x += 1;
                        }
                        for _ in 0..run {
                            plane[y * w + x] = previous;
                            x += 1;
                        }
                    }
                    if y > 0 {
                        for x in 0..w {
                            let v = plane[y * w + x];
                            // The bottom bit is the sign of the difference.
                            let delta = if v & 1 == 0 { (v >> 1) as i16 } else { -(((v >> 1) as i16) + 1) };
                            plane[y * w + x] = (plane[(y - 1) * w + x] as i16 + delta).rem_euclid(256) as u8;
                        }
                    }
                }
            }
        }
        let [red, green, blue] = &planes[..] else { unreachable!("three planes") };
        let rgb = red.iter().zip(green).zip(blue).flat_map(|((&r, &g), &b)| [r, g, b]).collect();
        Ok((rgb, at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `w`×`h` `B, G, R, X` pixels at a stride wider than the rows, from `f`.
    fn picture(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> (Vec<u8>, usize) {
        let stride = w * 4 + 12;
        let mut pixels = vec![0xEE; stride * h];
        for y in 0..h {
            for x in 0..w {
                let [r, g, b] = f(x, y);
                pixels[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&[b, g, r, 0x77]);
            }
        }
        (pixels, stride)
    }

    fn rgb(pixels: &[u8], stride: usize, w: usize, h: usize) -> Vec<u8> {
        (0..h).flat_map(|y| (0..w).flat_map(move |x| [pixels[y * stride + x * 4 + 2], pixels[y * stride + x * 4 + 1], pixels[y * stride + x * 4]])).collect()
    }

    /// Code and decode, asserting the round trip exact, and return the bitmap.
    fn round_trip(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let (pixels, stride) = picture(w, h, f);
        let mut out = vec![0xAB];
        encode(&pixels, stride, w as u16, h as u16, &mut Rows::default(), &mut out);
        assert_eq!(out[0], 0xAB, "appended");
        let (decoded, used) = decoder::decode(&out[1..], w, h).unwrap_or_else(|e| panic!("{w}x{h}: {e}"));
        assert_eq!(used, out.len() - 1, "{w}x{h}: every byte read");
        assert_eq!(decoded, rgb(&pixels, stride, w, h), "{w}x{h}");
        out.split_off(1)
    }

    fn noise(seed: u32) -> impl Fn(usize, usize) -> [u8; 3] {
        move |x, y| {
            let mut n = (x as u32).wrapping_mul(0x9E37_79B9) ^ (y as u32).wrapping_mul(0x85EB_CA6B) ^ seed;
            n ^= n >> 15;
            n = n.wrapping_mul(0x2C1B_3C6D);
            n ^= n >> 12;
            [n as u8, (n >> 8) as u8, (n >> 16) as u8]
        }
    }

    #[test]
    fn every_shape_of_picture_comes_back_exact() {
        for (w, h) in [(1, 1), (2, 1), (1, 2), (3, 3), (17, 5), (64, 64), (63, 17), (200, 64), (64, 1)] {
            round_trip(w, h, |_, _| [0, 0, 0]);
            round_trip(w, h, |_, _| [200, 13, 255]);
            round_trip(w, h, |x, y| [x as u8, y as u8, (x * y) as u8]);
            round_trip(w, h, |x, y| if (x / 3 + y / 2) % 2 == 0 { [255, 255, 255] } else { [30, 30, 30] });
            round_trip(w, h, noise(w as u32 * 31 + h as u32));
            // Runs of every length against every kind of row start.
            round_trip(w, h, |x, y| [((x + y) / 7 * 40) as u8, (x % 3 * 90) as u8, ((x / 16 + y) % 2 * 255) as u8]);
        }
    }

    /// Runs one longer and shorter than each form's limit, and runs that
    /// start a row, where what repeats is zero.
    #[test]
    fn runs_at_every_length_come_back_exact() {
        for len in 1..=130 {
            round_trip(len + 4, 2, |x, _| [if x < len { 0 } else { 9 }, if x < 2 || x >= len + 2 { 7 } else { 3 }, (x % 2) as u8]);
            round_trip(len, 3, |x, y| [if y == 1 { 5 } else { 250 }, (x == len / 2) as u8, 0]);
        }
    }

    #[test]
    fn a_flat_tile_is_a_few_bytes_a_row() {
        let out = round_trip(64, 64, |_, _| [40, 50, 60]);
        assert_eq!(out[0], RLE | NO_ALPHA);
        // A first row of one byte and a run of 63, then every row a run of
        // 64 zeroes: four bytes and two, in each plane.
        assert_eq!(out.len(), 1 + 3 * (4 + 63 * 2));
    }

    #[test]
    fn noise_goes_as_the_planes_as_they_are() {
        let out = round_trip(64, 64, noise(7));
        assert_eq!(out[0], NO_ALPHA, "no RLE");
        assert_eq!(out.len(), 1 + 3 * 64 * 64 + 1, "the planes and the padding byte");
    }

    /// The layout the specification shows, written out by hand: one row of
    /// a plane, three bytes and a run of the last.
    #[test]
    fn a_row_is_literals_then_a_run_of_the_last() {
        let mut out = Vec::new();
        scanline(&[1, 2, 3, 3, 3, 3, 3, 3], &mut out);
        assert_eq!(out, [0x35, 1, 2, 3]);
        out.clear();
        scanline(&[0; 64], &mut out);
        assert_eq!(out, [0xF2, 0x01 | (1 << 4)], "47 then 17 of zero, at the row's start");
        out.clear();
        scanline(&[0, 0, 9], &mut out);
        assert_eq!(out, [0x30, 0, 0, 9], "a run of two of zero is no run");
    }

    #[test]
    fn a_difference_is_folded_by_its_sign() {
        assert_eq!([0u8, 1, 0xFF, 2, 0xFE, 127, 0x80].map(fold), [0, 2, 1, 4, 3, 254, 255]);
    }
}
