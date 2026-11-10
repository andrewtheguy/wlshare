//! The lossless tiles' moving runs by a median predictor: the body of a
//! [`wlshare_proto::tiles::KIND_MEDIAN`] rectangle ([`pack`]), what a list
//! that asks for the predictor ([`wlshare_proto::tiles::TilesPicture::Median`])
//! is sent where the desktop moves, in place of the planar bitmap a lossless
//! list is sent there ([`super::planar`]). The still stream is planar under
//! both.
//!
//! Most of what a codec built for lossless pictures takes off a moving one
//! is the predictor. Planar's is the pixel above alone, over red, green and
//! blue apart. This one is JPEG-LS's ([ITU-T T.87] A.4.2): a rectangle's
//! pixels are cut into a green plane, a red less green and a blue less
//! green, and each sample is coded as its residual from the median of the
//! sample to its left, the one above and their gradient, left + above −
//! above-left: an edge either way is followed, and a slope is carried on.
//! The top row is predicted from the left alone, the left column from above
//! alone, and the first sample goes as it is. There is no context and no
//! entropy coder of its own: each plane's residuals are one zstd frame with
//! the parameters a planar bitmap's frame has ([`super::planar::compressor`]),
//! the three one after another, so the literals' Huffman coding is all of
//! it where the picture is a video's and its matches still find a window's
//! flat rows.
//!
//! Over the samples it is two thirds of the planar frames' bytes for 4% more
//! of the host's processor (`docs/tiles-performance.md`). A row the same
//! colours as the row above is a row of zero residuals in every plane, taken
//! at once, as planar's coder takes it.
//!
//! Its tests decode with a reader written from the protocol's text
//! (`median_decoder.rs`), which shares nothing with the encoder, the zstd by
//! ruzstd-decompress as the planar tiles' is.

use std::cell::RefCell;
use std::io;

use super::planar::{compressor, same_colours};

/// A thread's working space: the planes' rows of the row being coded and of
/// the row above, the planes' residuals, and a zstd context, made on the
/// thread's first rectangle and kept for the process.
struct Work {
    rows: Rows,
    residuals: [Vec<u8>; 3],
    zstd: zstd::bulk::Compressor<'static>,
}

/// Green, red less green and blue less green of two rows of pixels: the one
/// being coded and the one above it, whichever is which.
type Rows = [[Vec<u8>; 3]; 2];

thread_local! {
    static WORK: RefCell<Option<Work>> = const { RefCell::new(None) };
}

/// The body of a [`wlshare_proto::tiles::KIND_MEDIAN`] rectangle of the
/// `w`×`h` rectangle of `pixels`, `B, G, R, X` whose rows are `stride` bytes
/// apart from its top-left: its planes' residuals ([`residuals`]), each one
/// zstd frame, which says its size, green's first. On whichever thread calls
/// it, with that thread's working space. An error is zstd's, which a frame
/// of a plane this size has none of.
pub fn pack(pixels: &[u8], stride: usize, w: u16, h: u16) -> io::Result<Vec<u8>> {
    WORK.with_borrow_mut(|work| {
        let work = match work {
            Some(work) => work,
            None => work.insert(Work {
                rows: Default::default(),
                residuals: Default::default(),
                zstd: compressor()?,
            }),
        };
        residuals(pixels, stride, w, h, &mut work.rows, &mut work.residuals);
        let [green, red, blue] = &work.residuals;
        let mut body = work.zstd.compress(green)?;
        for plane in [red, blue] {
            body.extend_from_slice(&work.zstd.compress(plane)?);
        }
        Ok(body)
    })
}

/// Each plane's residuals of the `w`×`h` rectangle of `pixels`, a row after
/// a row, green's, red less green's and blue less green's, each `w * h`
/// bytes over whatever `out` held. `rows` is working space, kept from one
/// call to the next.
///
/// On x86-64 it is compiled twice, for the target's baseline and for AVX2,
/// and the processor chooses at run time: the same source, so the same
/// bytes, the loops that cut the planes and take the medians twice as wide.
fn residuals(
    pixels: &[u8],
    stride: usize,
    w: u16,
    h: u16,
    rows: &mut Rows,
    out: &mut [Vec<u8>; 3],
) {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: the processor has AVX2.
        return unsafe { residuals_avx2(pixels, stride, w, h, rows, out) };
    }
    residuals_in(pixels, stride, w, h, rows, out);
}

/// [`residuals`] for a processor with AVX2.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn residuals_avx2(
    pixels: &[u8],
    stride: usize,
    w: u16,
    h: u16,
    rows: &mut Rows,
    out: &mut [Vec<u8>; 3],
) {
    residuals_in(pixels, stride, w, h, rows, out);
}

#[inline(always)]
fn residuals_in(
    pixels: &[u8],
    stride: usize,
    w: u16,
    h: u16,
    rows: &mut Rows,
    out: &mut [Vec<u8>; 3],
) {
    let (w, h) = (usize::from(w), usize::from(h));
    debug_assert!(w > 0 && h > 0 && pixels.len() >= (h - 1) * stride + w * 4);
    for plane in out.iter_mut() {
        plane.clear();
        plane.resize(w * h, 0);
    }
    for row in rows.iter_mut().flatten() {
        row.resize(w, 0);
    }
    let [mut above, mut here] = rows.each_mut();
    for y in 0..h {
        let row = &pixels[y * stride..y * stride + w * 4];
        // A row the same colours as the row above is every plane's row of
        // zero residuals, which the planes were filled with, and its planes'
        // rows are the ones above's, kept.
        if y > 0 && same_colours(row, &pixels[(y - 1) * stride..(y - 1) * stride + w * 4]) {
            continue;
        }
        {
            let [green, red, blue] = &mut *here;
            split(row, green, red, blue);
        }
        for ((here, above), out) in here.iter().zip(above.iter()).zip(out.iter_mut()) {
            let (here, out) = (&here[..w], &mut out[y * w..(y + 1) * w]);
            if y == 0 {
                out[0] = here[0];
                for x in 1..w {
                    out[x] = here[x].wrapping_sub(here[x - 1]);
                }
            } else {
                out[0] = here[0].wrapping_sub(above[0]);
                median_row(here, &above[..w], out);
            }
        }
        std::mem::swap(&mut above, &mut here);
    }
}

/// Green, red less green and blue less green of a row of `B, G, R, X`
/// pixels, each into its plane's row.
#[inline(always)]
fn split(row: &[u8], green: &mut [u8], red: &mut [u8], blue: &mut [u8]) {
    let w = green.len();
    let (row, red, blue) = (&row.as_chunks::<4>().0[..w], &mut red[..w], &mut blue[..w]);
    for i in 0..w {
        green[i] = row[i][1];
        red[i] = row[i][2].wrapping_sub(row[i][1]);
        blue[i] = row[i][0].wrapping_sub(row[i][1]);
    }
}

/// A row's residuals from its second sample on, into `out` from its second:
/// each sample less the median of the sample to its left, the one above and
/// left + above − above-left.
#[inline(always)]
fn median_row(here: &[u8], above: &[u8], out: &mut [u8]) {
    let n = here.len() - 1;
    let (left, up, corner, at, out) = (
        &here[..n],
        &above[1..][..n],
        &above[..n],
        &here[1..][..n],
        &mut out[1..][..n],
    );
    for i in 0..n {
        out[i] = at[i].wrapping_sub(median(left[i], up[i], corner[i]));
    }
}

/// The median of `a`, `b` and `a + b - c`: where `c` is past both it is the
/// one further from `c`, and between them the sum, which is then between
/// them too and so a byte.
#[inline(always)]
fn median(a: u8, b: u8, c: u8) -> u8 {
    let (lo, hi) = (a.min(b), a.max(b));
    if c >= hi {
        lo
    } else if c <= lo {
        hi
    } else {
        a.wrapping_add(b).wrapping_sub(c)
    }
}

// The tests' reader, written from the protocol's text; the examples include
// it themselves. The daemon never decodes.
#[cfg(test)]
#[path = "median_decoder.rs"]
pub mod decoder;

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
                pixels[y * stride + x * 4..y * stride + x * 4 + 4]
                    .copy_from_slice(&[b, g, r, 0x77]);
            }
        }
        (pixels, stride)
    }

    fn rgb(pixels: &[u8], stride: usize, w: usize, h: usize) -> Vec<u8> {
        (0..h)
            .flat_map(|y| {
                (0..w).flat_map(move |x| {
                    [
                        pixels[y * stride + x * 4 + 2],
                        pixels[y * stride + x * 4 + 1],
                        pixels[y * stride + x * 4],
                    ]
                })
            })
            .collect()
    }

    fn noise(seed: u32) -> impl Fn(usize, usize) -> [u8; 3] {
        move |x, y| {
            let mut n =
                (x as u32).wrapping_mul(0x9E37_79B9) ^ (y as u32).wrapping_mul(0x85EB_CA6B) ^ seed;
            n ^= n >> 15;
            n = n.wrapping_mul(0x2C1B_3C6D);
            n ^= n >> 12;
            [n as u8, (n >> 8) as u8, (n >> 16) as u8]
        }
    }

    /// A photograph's kind of picture: slopes that cross, and a grain.
    fn soft(seed: u32) -> impl Fn(usize, usize) -> [u8; 3] {
        let grain = noise(seed);
        move |x, y| {
            let [n, _, _] = grain(x, y);
            [
                (x * 2 + y + usize::from(n & 3)) as u8,
                (x + y * 3 + usize::from(n >> 6)) as u8,
                (200 + x / 2 - y.min(200) + usize::from(n & 1)) as u8,
            ]
        }
    }

    /// Pack and unpack, asserting the round trip exact, and return the body.
    fn packed(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let (pixels, stride) = picture(w, h, f);
        let body = pack(&pixels, stride, w as u16, h as u16).unwrap();
        assert_eq!(
            decoder::unpack_each(&[(&body, w, h)])
                .pop()
                .expect("a result for the one body")
                .unwrap_or_else(|e| panic!("{w}x{h}: {e}")),
            rgb(&pixels, stride, w, h),
            "{w}x{h}"
        );
        body
    }

    #[test]
    fn every_shape_of_picture_comes_back_exact() {
        for (w, h) in [
            (1, 1),
            (2, 1),
            (1, 2),
            (2, 2),
            (3, 3),
            (17, 5),
            (64, 64),
            (63, 17),
            (200, 64),
            (64, 1),
            (1, 64),
            (1920, 64),
        ] {
            packed(w, h, |_, _| [0, 0, 0]);
            packed(w, h, |_, _| [200, 13, 255]);
            packed(w, h, |x, y| [x as u8, y as u8, (x * y) as u8]);
            packed(w, h, |x, y| {
                if (x / 3 + y / 2) % 2 == 0 {
                    [255, 255, 255]
                } else {
                    [30, 30, 30]
                }
            });
            packed(w, h, noise(w as u32 * 31 + h as u32));
            packed(w, h, soft(w as u32 + h as u32));
            // Rows that repeat the row above beside rows that do not.
            packed(w, h, |x, y| [(x * 5) as u8, (y / 3 * 70) as u8, 9]);
        }
    }

    /// The residuals are the protocol's, written out by hand: the first
    /// sample as it is, the top row from the left, the left column from
    /// above, and the rest from the median, over green, red less green and
    /// blue less green.
    #[test]
    fn the_residuals_are_the_documented_ones() {
        // Green alone varies: red and blue follow it, so their planes are
        // nothing past what they are less green.
        let green = [[10u8, 12, 20], [11, 30, 5], [11, 30, 5]];
        let (pixels, stride) = picture(3, 3, |x, y| {
            let g = green[y][x];
            [g.wrapping_add(3), g, g.wrapping_sub(1)]
        });
        let (mut rows, mut out) = (Rows::default(), <[Vec<u8>; 3]>::default());
        residuals(&pixels, stride, 3, 3, &mut rows, &mut out);
        let byte = |v: i32| v.rem_euclid(256) as u8;
        assert_eq!(
            out[0],
            [
                // The top row: 10, then each less the one to its left.
                10,
                2,
                8,
                // 11 less 10 above; 30 less the median of 11, 12 and
                // 11 + 12 - 10 = 13, which is 12; 5 less the median of 30,
                // 20 and 30 + 20 - 12 = 38, which is 30.
                1,
                18,
                byte(5 - 30),
                // The row above again.
                0,
                0,
                0,
            ]
        );
        assert_eq!(out[1], [3, 0, 0, 0, 0, 0, 0, 0, 0], "red less green");
        assert_eq!(out[2], [255, 0, 0, 0, 0, 0, 0, 0, 0], "blue less green");
    }

    #[test]
    fn the_median_is_of_left_above_and_their_gradient() {
        for a in 0..=255u8 {
            for b in (0..=255u8).step_by(5) {
                for c in (0..=255u8).step_by(3) {
                    let mut three = [
                        i32::from(a),
                        i32::from(b),
                        i32::from(a) + i32::from(b) - i32::from(c),
                    ];
                    three.sort_unstable();
                    assert_eq!(i32::from(median(a, b, c)), three[1], "{a} {b} {c}");
                }
            }
        }
    }

    /// A body is three zstd frames, each of a plane of the rectangle, and
    /// nothing else: a body of fewer or more, of a plane of another size, or
    /// with a skippable frame among them, which the decoders would read
    /// past, is refused.
    #[test]
    fn a_body_of_anything_but_its_three_planes_is_refused() {
        let f = |x: usize, y: usize| [x as u8, (y * 9) as u8, 7];
        let body = packed(17, 5, f);
        let frames = decoder::frames(&body).unwrap();
        let skippable = [&0x184D_2A50u32.to_le_bytes()[..], &1u32.to_le_bytes(), &[0]].concat();
        let two = [frames[0], frames[1]].concat();
        let four = [&body[..], frames[2]].concat();
        let after = [&body[..], &skippable].concat();
        let before = [&skippable[..], &body].concat();
        let among = [frames[0], &skippable, frames[1], frames[2]].concat();
        let other = packed(16, 5, f);
        let read = decoder::unpack_each(&[
            (&two, 17, 5),
            (&four, 17, 5),
            (&after, 17, 5),
            (&before, 17, 5),
            (&among, 17, 5),
            (&other, 17, 5),
            (&body, 5, 17),
            (&body, 17, 5),
        ]);
        let refused: Vec<bool> = read.iter().map(Result::is_err).collect();
        assert_eq!(
            refused,
            [true, true, true, true, true, true, false, false],
            "a plane is its bytes, whichever way the rectangle lies"
        );
    }

    /// A flat or a repeating picture is a few bytes, a picture with slopes
    /// fewer than its planar bitmap's frame, and noise little more than its
    /// planes.
    #[test]
    fn a_body_is_small_where_the_picture_is_plain_or_smooth() {
        let flat = packed(1920, 64, |_, _| [40, 50, 60]);
        assert!(
            flat.len() < 256,
            "a flat run of tiles is {} bytes",
            flat.len()
        );
        let text = |x: usize, y: usize| {
            if (x % 8 < 2 || y % 16 == 3) && y % 16 < 12 {
                [0, 0, 0]
            } else {
                [255, 255, 255]
            }
        };
        let body = packed(1920, 64, text);
        assert!(
            body.len() * 40 < 3 * 1920 * 64,
            "a line of text is {} bytes",
            body.len()
        );
        let (pixels, stride) = picture(1920, 64, soft(5));
        let planar = super::super::planar::pack(&pixels, stride, 1920, 64).unwrap();
        let body = packed(1920, 64, soft(5));
        assert!(
            body.len() * 10 < planar.len() * 9,
            "slopes and a grain are {} bytes, a planar bitmap's frame {}",
            body.len(),
            planar.len()
        );
        assert!(
            packed(64, 64, noise(3)).len() <= 3 * 64 * 64 + 3 * 32,
            "noise framed is the planes and three frames' few bytes"
        );
    }

    /// The AVX2 copy codes what the baseline's does, every shape of picture.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn the_avx2_copy_codes_the_same() {
        if !std::arch::is_x86_feature_detected!("avx2") {
            eprintln!("no AVX2 here: the copy is not run");
            return;
        }
        for (w, h) in [(1, 1), (17, 5), (64, 64), (63, 17), (200, 64), (1920, 8)] {
            for (pixels, stride) in [
                picture(w, h, noise(w as u32 ^ 5)),
                picture(w, h, soft(w as u32)),
                picture(w, h, |x, y| {
                    [(x / 5 * 9) as u8, (y / 3) as u8, (x % 7 == 0) as u8 * 200]
                }),
            ] {
                let (mut narrow, mut wide) = (<[Vec<u8>; 3]>::default(), <[Vec<u8>; 3]>::default());
                residuals_in(
                    &pixels,
                    stride,
                    w as u16,
                    h as u16,
                    &mut Rows::default(),
                    &mut narrow,
                );
                // SAFETY: the processor has AVX2.
                unsafe {
                    residuals_avx2(
                        &pixels,
                        stride,
                        w as u16,
                        h as u16,
                        &mut Rows::default(),
                        &mut wide,
                    )
                };
                assert_eq!(narrow, wide, "{w}x{h}");
            }
        }
    }

    /// The working space is kept from one rectangle to the next, of any
    /// size after any other: nothing of the one before shows in the next.
    #[test]
    fn a_rectangle_after_another_is_coded_alone() {
        let (mut rows, mut kept) = (Rows::default(), <[Vec<u8>; 3]>::default());
        for (w, h, seed) in [(64, 64, 1), (17, 5, 2), (200, 9, 3), (17, 5, 2), (1, 1, 4)] {
            let (pixels, stride) = picture(w, h, soft(seed));
            let mut fresh = <[Vec<u8>; 3]>::default();
            residuals(&pixels, stride, w as u16, h as u16, &mut rows, &mut kept);
            residuals(
                &pixels,
                stride,
                w as u16,
                h as u16,
                &mut Rows::default(),
                &mut fresh,
            );
            assert_eq!(kept, fresh, "{w}x{h}");
        }
    }
}
