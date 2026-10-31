//! The lossless tiles' coding: RDP 6.0 bitmap compression, the planar codec
//! of RDP's graphics pipeline ([MS-RDPEGDI] 2.2.2.5.1, [MS-RDPEGFX]
//! `RDPGFX_CODECID_PLANAR`), which a Windows host whose policy asks for a
//! lossless picture draws everything with, each bitmap then zstd, as the
//! host's own bulk compression wraps its pipeline: one frame of its own
//! ([`pack`]), a [`wlshare_proto::tiles::KIND_PLANAR`] rectangle's body, or
//! the still stream's next bytes ([`StillStream`]), a
//! [`wlshare_proto::tiles::KIND_STILL`]'s.
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
//! **zstd.** Where the desktop moves, each bitmap is one zstd frame,
//! depending on no frame before it, so a rectangle is packed on any thread
//! and read on its own: the planes' runs leave a text's or a window's
//! repeats and a photograph's skewed bytes, which the frame takes a further
//! 1.5 to 10 times off, and more where a rectangle repeats itself. Where it
//! is still, each is the still stream's, one zstd stream across the
//! session flushed at each bitmap, with a 1 MiB window: a page, a terminal
//! or a menu shown again is matched against what it already carried, a
//! third to three fifths more off. It is state both ends keep, coded in
//! order and started over together. Its parameters are level 1's fast
//! strategy with a smaller table and longer matches ([`zstd_parameters`]):
//! a moving picture's frames are the literals' entropy coding more than its
//! matches, and these coded a video's nearly twice as fast as level 1 for
//! 1.67 times off the planes against 1.69, a page of text 4.9 times against
//! 5.2. Levels below 1, which code no literal, gave a moving picture back
//! almost nothing over the planes, and above it cost twice the time for a
//! percent.
//!
//! Its tests decode with a reader written here from the specifications,
//! which shares nothing with the encoder, the zstd by ruzstd, a decoder in
//! Rust that shares nothing with libzstd, the still stream by its fork's
//! `FrameDecoder::peek`, which hands out an open frame's newest bytes;
//! zDesk's page decodes the zstd with the same, and the bitmaps with its RDP
//! compositor's planar codec, the one that draws a Windows host's lossless
//! pipeline.

use std::cell::RefCell;
use std::io;

/// The zstd parameters a bitmap is framed with: the fast strategy, a
/// 128 KiB window, a 4096-entry table and matches of 7 bytes or more.
fn zstd_parameters() -> [zstd::zstd_safe::CParameter; 7] {
    use zstd::zstd_safe::{CParameter, Strategy};
    [
        CParameter::Strategy(Strategy::ZSTD_fast),
        CParameter::WindowLog(17),
        CParameter::HashLog(12),
        CParameter::ChainLog(12),
        CParameter::SearchLog(1),
        CParameter::MinMatch(7),
        CParameter::TargetLength(0),
    ]
}

/// A zstd context set to [`zstd_parameters`].
fn compressor() -> io::Result<zstd::bulk::Compressor<'static>> {
    let mut zstd = zstd::bulk::Compressor::new(1)?;
    for parameter in zstd_parameters() {
        zstd.set_parameter(parameter)?;
    }
    Ok(zstd)
}

/// What a thread packs a rectangle with: the planar coder's rows, the bitmap
/// it codes, and a zstd context, made on the thread's first rectangle and
/// kept for the process.
struct Packer {
    rows: Rows,
    bitmap: Vec<u8>,
    zstd: zstd::bulk::Compressor<'static>,
}

thread_local! {
    static PACKER: RefCell<Option<Packer>> = const { RefCell::new(None) };
}

/// The body of a [`wlshare_proto::tiles::KIND_PLANAR`] rectangle of the
/// `w`×`h` rectangle of `pixels`, `B, G, R, X` whose rows are `stride` bytes
/// apart from its top-left: its planar bitmap ([`encode`]) as one zstd frame,
/// which says its size. On whichever thread calls it, with that thread's
/// working space. An error is zstd's, which a frame of a bitmap this size
/// has none of.
pub fn pack(pixels: &[u8], stride: usize, w: u16, h: u16) -> io::Result<Vec<u8>> {
    PACKER.with_borrow_mut(|packer| {
        let packer = match packer {
            Some(packer) => packer,
            None => packer.insert(Packer { rows: Rows::default(), bitmap: Vec::new(), zstd: compressor()? }),
        };
        packer.bitmap.clear();
        encode(pixels, stride, w, h, &mut packer.rows, &mut packer.bitmap);
        packer.zstd.compress(&packer.bitmap)
    })
}

/// The planar bitmap ([`encode`]) of the `w`×`h` rectangle of `pixels`,
/// `B, G, R, X` whose rows are `stride` bytes apart from its top-left, for
/// the still stream ([`StillStream`]). On whichever thread calls it, with
/// that thread's working space.
pub fn bitmap(pixels: &[u8], stride: usize, w: u16, h: u16) -> io::Result<Vec<u8>> {
    PACKER.with_borrow_mut(|packer| {
        let packer = match packer {
            Some(packer) => packer,
            None => packer.insert(Packer { rows: Rows::default(), bitmap: Vec::new(), zstd: compressor()? }),
        };
        let mut bitmap = Vec::new();
        encode(pixels, stride, w, h, &mut packer.rows, &mut bitmap);
        Ok(bitmap)
    })
}

/// The still stream's window: 1 MiB, which a desktop of pages, a terminal
/// or a menu shown again is found in. 8 MiB took no more off.
const STILL_WINDOW_LOG: u32 = 20;

/// The still stream: one zstd stream across every
/// [`wlshare_proto::tiles::KIND_STILL`] rectangle of a session, each body its
/// bytes through a flush after the rectangle's bitmap, so that a page, a
/// terminal or a menu shown again is matched against what the stream already
/// carried. The parameters are a body's own ([`zstd_parameters`]) but the
/// window, a mebibyte. Coded in order, on the encoder's thread; made anew
/// where it starts over, its first body a new frame.
pub struct StillStream {
    zstd: zstd::zstd_safe::CCtx<'static>,
}

impl StillStream {
    pub fn new() -> io::Result<Self> {
        let mut zstd = zstd::zstd_safe::CCtx::create();
        let set = |zstd: &mut zstd::zstd_safe::CCtx, parameter| zstd.set_parameter(parameter).map(drop).map_err(zstd_error);
        set(&mut zstd, zstd::zstd_safe::CParameter::CompressionLevel(1))?;
        for parameter in zstd_parameters() {
            set(&mut zstd, parameter)?;
        }
        set(&mut zstd, zstd::zstd_safe::CParameter::WindowLog(STILL_WINDOW_LOG))?;
        Ok(Self { zstd })
    }

    /// Append to `out` the stream's bytes of `bitmap`, through a flush: the
    /// body of a STILL rectangle. An error is zstd's, which a flush of a
    /// bitmap this size has none of.
    pub fn push(&mut self, bitmap: &[u8], out: &mut Vec<u8>) -> io::Result<()> {
        use zstd::zstd_safe::{InBuffer, OutBuffer, zstd_sys::ZSTD_EndDirective};
        let mut input = InBuffer::around(bitmap);
        loop {
            out.reserve(zstd::zstd_safe::compress_bound(bitmap.len() - input.pos()) + 64);
            let at = out.len();
            let mut output = OutBuffer::around_pos(out, at);
            let left = self.zstd.compress_stream2(&mut output, &mut input, ZSTD_EndDirective::ZSTD_e_flush).map_err(zstd_error)?;
            if left == 0 && input.pos() == bitmap.len() {
                return Ok(());
            }
        }
    }
}

/// A zstd error code as an [`io::Error`].
fn zstd_error(code: usize) -> io::Error {
    io::Error::other(zstd::zstd_safe::get_error_name(code))
}

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
///
/// On x86-64 it is compiled twice, for the target's baseline and for AVX2,
/// and the processor chooses at run time: the same source, so the same
/// bytes, the loops that cut the planes from the pixels twice as wide.
pub fn encode(pixels: &[u8], stride: usize, w: u16, h: u16, rows: &mut Rows, out: &mut Vec<u8>) {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: the processor has AVX2.
        return unsafe { encode_avx2(pixels, stride, w, h, rows, out) };
    }
    encode_in(pixels, stride, w, h, rows, out);
}

/// [`encode`] for a processor with AVX2.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn encode_avx2(pixels: &[u8], stride: usize, w: u16, h: u16, rows: &mut Rows, out: &mut Vec<u8>) {
    encode_in(pixels, stride, w, h, rows, out);
}

#[inline(always)]
fn encode_in(pixels: &[u8], stride: usize, w: u16, h: u16, rows: &mut Rows, out: &mut Vec<u8>) {
    let (w, h) = (usize::from(w), usize::from(h));
    debug_assert!(w > 0 && h > 0 && pixels.len() >= (h - 1) * stride + w * 4);
    let from = out.len();
    out.push(RLE | NO_ALPHA);
    for plane in &mut rows.coded {
        plane.clear();
    }
    rows.diff.iter_mut().for_each(|row| row.resize(w, 0));
    // A row the same colours as the row above is every plane's row of
    // zeroes, coded once.
    if rows.zero_width != w {
        rows.diff[0].fill(0);
        rows.zero.clear();
        scanline(&rows.diff[0], &mut rows.zero);
        rows.zero_width = w;
    }
    for y in 0..h {
        let row = &pixels[y * stride..y * stride + w * 4];
        let [red, green, blue] = &mut rows.diff;
        if y == 0 {
            split(row, red, green, blue);
        } else {
            let above = &pixels[(y - 1) * stride..(y - 1) * stride + w * 4];
            if same_colours(row, above) {
                for plane in &mut rows.coded {
                    plane.extend_from_slice(&rows.zero);
                }
                continue;
            }
            difference(row, above, red, green, blue);
        }
        for (diff, coded) in rows.diff.iter().zip(&mut rows.coded) {
            scanline(diff, coded);
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

/// The encoder's working space: each plane's coded rows, the row of
/// differences being coded, and the coding of a row of zeroes
/// `zero_width` wide.
#[derive(Default)]
pub struct Rows {
    coded: [Vec<u8>; 3],
    diff: [Vec<u8>; 3],
    zero: Vec<u8>,
    zero_width: usize,
}

/// Red, green and blue of a row of `B, G, R, X` pixels, each into its plane's row.
#[inline(always)]
fn split(row: &[u8], red: &mut [u8], green: &mut [u8], blue: &mut [u8]) {
    let w = red.len();
    let (row, green, blue) = (&row.as_chunks::<4>().0[..w], &mut green[..w], &mut blue[..w]);
    for i in 0..w {
        red[i] = row[i][2];
        green[i] = row[i][1];
        blue[i] = row[i][0];
    }
}

/// Each plane's row of the differences of a row of `B, G, R, X` pixels
/// from the row above, folded.
#[inline(always)]
fn difference(row: &[u8], above: &[u8], red: &mut [u8], green: &mut [u8], blue: &mut [u8]) {
    let w = red.len();
    let (row, above) = (&row.as_chunks::<4>().0[..w], &above.as_chunks::<4>().0[..w]);
    let (green, blue) = (&mut green[..w], &mut blue[..w]);
    for i in 0..w {
        red[i] = fold(row[i][2].wrapping_sub(above[i][2]));
        green[i] = fold(row[i][1].wrapping_sub(above[i][1]));
        blue[i] = fold(row[i][0].wrapping_sub(above[i][0]));
    }
}

/// Whether two rows of `B, G, R, X` pixels are the same colours, whatever their `X`.
#[inline(always)]
fn same_colours(row: &[u8], above: &[u8]) -> bool {
    let colour = |px: &[u8; 4]| u32::from_le_bytes(*px) & 0x00FF_FFFF;
    // A word of pixels at a time, so the loop stops at a block that differs.
    let (row, above) = (row.as_chunks::<4>().0, above.as_chunks::<4>().0);
    row.chunks(8).zip(above.chunks(8)).all(|(a, b)| a.iter().zip(b).fold(0, |differ, (a, b)| differ | (colour(a) ^ colour(b))) == 0)
}

/// A row's difference from the row above, folded: the sign the bottom bit.
#[inline(always)]
fn fold(d: u8) -> u8 {
    (d << 1) ^ ((d as i8 >> 7) as u8)
}

/// The eight bytes of `row` from `at`, the first the word's lowest.
#[inline(always)]
fn word(row: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(*row[at..].first_chunk().expect("eight bytes"))
}

/// Of a word, 0x80 in each byte that is zero and nothing in the others.
#[inline(always)]
fn zero_bytes(x: u64) -> u64 {
    const LOW: u64 = 0x7F7F_7F7F_7F7F_7F7F;
    !(((x & LOW) + LOW) | x | LOW)
}

/// How many bytes of `row` from `at` are `value`.
#[inline(always)]
fn run_of(row: &[u8], at: usize, value: u8) -> usize {
    let every = u64::from(value) * 0x0101_0101_0101_0101;
    let mut i = at;
    while i + 8 <= row.len() {
        let differ = word(row, i) ^ every;
        if differ != 0 {
            return i + differ.trailing_zeros() as usize / 8 - at;
        }
        i += 8;
    }
    i + row[i..].iter().take_while(|&&b| b == value).count() - at
}

/// The first place in `row` from `from` that begins four bytes alike: the
/// last byte of a segment's literals and a run of it after.
#[inline(always)]
fn four_alike(row: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    // Eight places a word, which reads the three bytes after them too.
    while i + 11 <= row.len() {
        let [a, b, c, d] = [0, 1, 2, 3].map(|k| word(row, i + k));
        let alike = zero_bytes((a ^ b) | (b ^ c) | (c ^ d));
        if alike != 0 {
            return Some(i + alike.trailing_zeros() as usize / 8);
        }
        i += 8;
    }
    (i..row.len().saturating_sub(3)).find(|&i| row[i] == row[i + 1] && row[i] == row[i + 2] && row[i] == row[i + 3])
}

/// One row as segments: bytes as they are, then a run of the last of them —
/// or, where none came before in the row, of zero. A run is taken at the
/// first place it can be, three bytes or more.
#[inline(always)]
fn scanline(row: &[u8], out: &mut Vec<u8>) {
    out.reserve(row.len() + row.len() / NIBBLE + 2);
    // A run of zero at the row's start, where what a run repeats is zero.
    let mut literal_from = match run_of(row, 0, 0) {
        run if run >= RUN_LEAST => {
            segments(&[], run, 0, out);
            run
        }
        _ => 0,
    };
    while let Some(i) = four_alike(row, literal_from) {
        let (value, at) = (row[i], i + 1);
        let run = run_of(row, at, value);
        segments(&row[literal_from..at], run, value, out);
        literal_from = at + run;
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

// The tests' reader, written from the specifications; the examples include
// it themselves. The daemon never decodes.
#[cfg(test)]
#[path = "planar_decoder.rs"]
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
                pixels[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&[b, g, r, 0x77]);
            }
        }
        (pixels, stride)
    }

    fn rgb(pixels: &[u8], stride: usize, w: usize, h: usize) -> Vec<u8> {
        (0..h).flat_map(|y| (0..w).flat_map(move |x| [pixels[y * stride + x * 4 + 2], pixels[y * stride + x * 4 + 1], pixels[y * stride + x * 4]])).collect()
    }

    /// Pack and unpack, asserting the round trip exact, and return the body.
    fn packed(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let (pixels, stride) = picture(w, h, f);
        let body = pack(&pixels, stride, w as u16, h as u16).unwrap();
        assert_eq!(decoder::unpack(&body, w, h).unwrap_or_else(|e| panic!("{w}x{h}: {e}")), rgb(&pixels, stride, w, h), "{w}x{h}");
        body
    }

    /// A body is one zstd frame of the bitmap, read back by ruzstd: every
    /// shape exact, a flat or a repeating picture a few bytes, and noise
    /// little more than its bitmap.
    #[test]
    fn a_body_is_the_bitmap_in_one_zstd_frame() {
        for (w, h) in [(1, 1), (17, 5), (64, 64), (200, 64), (1920, 64)] {
            packed(w, h, |x, y| [x as u8, y as u8, (x * y) as u8]);
            packed(w, h, noise(w as u32 + h as u32));
        }
        let flat = packed(1920, 64, |_, _| [40, 50, 60]);
        assert!(flat.len() < 256, "a flat run of tiles is {} bytes", flat.len());
        // A line of text, the same glyph every 8 pixels: the planes' runs
        // leave the glyph, and the frame its repeats.
        let text = |x: usize, y: usize| if (x % 8 < 2 || y % 16 == 3) && y % 16 < 12 { [0, 0, 0] } else { [255, 255, 255] };
        let mut bitmap = Vec::new();
        let (pixels, stride) = picture(1920, 64, text);
        encode(&pixels, stride, 1920, 64, &mut Rows::default(), &mut bitmap);
        let body = packed(1920, 64, text);
        assert!(body.len() * 4 < bitmap.len(), "{} bytes framed of a {}-byte bitmap", body.len(), bitmap.len());
        let (pixels, stride) = picture(64, 64, noise(3));
        let mut bitmap = Vec::new();
        encode(&pixels, stride, 64, 64, &mut Rows::default(), &mut bitmap);
        assert!(packed(64, 64, noise(3)).len() <= bitmap.len() + 32, "noise framed is the bitmap and a frame's few bytes");
    }

    /// Every STILL's body is the stream's bytes of its bitmap, read after the
    /// ones before it: the first a new frame and none after it, each read
    /// exact by the client's reader, ruzstd a block at a time, a picture shown
    /// again a few bytes, and a stream made anew a new frame.
    #[test]
    fn a_still_is_the_streams_bytes_of_its_bitmap() {
        let text = |x: usize, y: usize| if (x % 8 < 2 || y % 16 == 3) && y % 16 < 12 { [0, 0, 0] } else { [255, 255, 255] };
        let pictures = [
            (1920, 64, picture(1920, 64, text)),
            (64, 64, picture(64, 64, noise(9))),
            (200, 30, picture(200, 30, |x, y| [x as u8, y as u8, 7])),
            (1920, 64, picture(1920, 64, text)),
            (1, 1, picture(1, 1, |_, _| [1, 2, 3])),
        ];
        let mut stream = StillStream::new().unwrap();
        let mut reader = decoder::StillReader::default();
        let mut bodies = Vec::new();
        for (i, (w, h, (pixels, stride))) in pictures.iter().enumerate() {
            let (pixels, stride) = (pixels, *stride);
            let bitmap = bitmap(pixels, stride, *w as u16, *h as u16).unwrap();
            let mut body = vec![0xAB];
            stream.push(&bitmap, &mut body).unwrap();
            let body = body.split_off(1);
            assert_eq!(body.starts_with(&decoder::ZSTD_MAGIC), i == 0, "body {i} begins a frame");
            assert_eq!(reader.unpack(&body, *w, *h).unwrap(), rgb(pixels, stride, *w, *h), "body {i} by the reader");
            bodies.push(body);
        }
        assert!(bodies[3].len() * 4 < bodies[0].len(), "a line of text again is {} bytes, against {}", bodies[3].len(), bodies[0].len());
        // Made anew, the stream starts over, and the reader with it.
        let (pixels, stride) = picture(64, 64, noise(4));
        let mut body = Vec::new();
        StillStream::new().unwrap().push(&bitmap(&pixels, stride, 64, 64).unwrap(), &mut body).unwrap();
        assert!(body.starts_with(&decoder::ZSTD_MAGIC));
        assert_eq!(reader.unpack(&body, 64, 64).unwrap(), rgb(&pixels, stride, 64, 64));
        assert!(decoder::StillReader::default().unpack(&bodies[1], 64, 64).is_err(), "a body that continues no stream");
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

    /// A row the same colours as the row above is every plane's row of
    /// zeroes, whatever the pixels' fourth bytes, at every width.
    #[test]
    fn a_row_like_the_one_above_is_zeroes_whatever_its_fourth_bytes() {
        for w in [1, 2, 3, 7, 8, 9, 64, 65, 200] {
            let (mut pixels, stride) = picture(w, 4, |x, _| [x as u8, (x * 7) as u8, 200]);
            let mut plain = Vec::new();
            encode(&pixels, stride, w as u16, 4, &mut Rows::default(), &mut plain);
            for (i, px) in pixels.chunks_mut(4).enumerate() {
                px[3] = i as u8;
            }
            let mut rows = Rows::default();
            for _ in 0..2 {
                let mut out = Vec::new();
                encode(&pixels, stride, w as u16, 4, &mut rows, &mut out);
                assert_eq!(out, plain, "{w} wide");
                assert_eq!(decoder::decode(&out, w, 4).unwrap().0, rgb(&pixels, stride, w, 4));
            }
        }
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
            for (pixels, stride) in [picture(w, h, noise(w as u32 ^ 5)), picture(w, h, |x, y| [(x / 5 * 9) as u8, (y / 3) as u8, (x % 7 == 0) as u8 * 200])] {
                let (mut narrow, mut wide) = (Vec::new(), Vec::new());
                encode_in(&pixels, stride, w as u16, h as u16, &mut Rows::default(), &mut narrow);
                // SAFETY: the processor has AVX2.
                unsafe { encode_avx2(&pixels, stride, w as u16, h as u16, &mut Rows::default(), &mut wide) };
                assert_eq!(narrow, wide, "{w}x{h}");
            }
        }
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
