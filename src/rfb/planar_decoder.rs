//! A planar bitmap read back as the specification lays it out
//! ([MS-RDPEGDI] 2.2.2.5.1 and its RLE planes, 2.2.2.5.1.1–3), written
//! for the tests from the text and not from the encoder, and the zstd frame
//! around it read by ruzstd-decompress; the examples' pages draw with it too, and the
//! still stream ([`StillReader`]) by ruzstd-decompress as well.

/// The `w`×`h` pixels of `bitmap`, `R, G, B` a pixel, top row first, and
/// the bytes it took; `Err` where it is not a whole lossless bitmap of
/// that size with no alpha plane.
pub fn decode(bitmap: &[u8], w: usize, h: usize) -> Result<(Vec<u8>, usize), String> {
    let header = *bitmap.first().ok_or("no format header")?;
    if header & 0x07 != 0 || header & 0x08 != 0 {
        return Err(format!(
            "format header {header:#04x}: colour loss or subsampling"
        ));
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
                        let delta = if v & 1 == 0 {
                            (v >> 1) as i16
                        } else {
                            -(((v >> 1) as i16) + 1)
                        };
                        plane[y * w + x] =
                            (plane[(y - 1) * w + x] as i16 + delta).rem_euclid(256) as u8;
                    }
                }
            }
        }
    }
    let [red, green, blue] = &planes[..] else {
        unreachable!("three planes")
    };
    let rgb = red
        .iter()
        .zip(green)
        .zip(blue)
        .flat_map(|((&r, &g), &b)| [r, g, b])
        .collect();
    Ok((rgb, at))
}

/// Whether `body` is one zstd frame and nothing after it, as a
/// [`wlshare_proto::tiles::KIND_PLANAR`] body is: the decoders read every
/// frame of an input, and skip skippable ones.
fn one_frame(body: &[u8]) -> Result<(), String> {
    if !body.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        return Err("a body that begins no zstd frame".into());
    }
    let len = ruzstd_decompress::decoding::frame_compressed_size(body)
        .map_err(|e| format!("a zstd frame: {e}"))?;
    if len != body.len() {
        return Err(format!("{} bytes past its zstd frame", body.len() - len));
    }
    Ok(())
}

/// The pixels each of an update's [`wlshare_proto::tiles::KIND_PLANAR`]
/// bodies carries, `(body, w, h)`, `R, G, B` a pixel, top row first, read as
/// zDesk's page reads a batch's: their zstd frames decoded side by side by
/// ruzstd-decompress, each into the room its headers bound it to and held to
/// what a planar bitmap of its rectangle can be, then each bitmap side by
/// side, which must be one planar bitmap of exactly its rectangle and nothing
/// after it. One result for each body, in order.
pub fn unpack_each(bodies: &[(&[u8], usize, usize)]) -> Vec<Result<Vec<u8>, String>> {
    use rayon::prelude::*;
    let mut outputs: Vec<Vec<u8>> = Vec::with_capacity(bodies.len());
    let mut bounded: Vec<Result<(), String>> = Vec::with_capacity(bodies.len());
    for &(body, w, h) in bodies {
        // A format header, three planes of at most two bytes a pixel, a
        // run-length coded row's worst, and the plain form's padding byte.
        let most = 1 + 3 * 2 * w * h + 1;
        let bound = one_frame(body)
            .and_then(|()| {
                ruzstd_decompress::decoding::decompressed_bound(body)
                    .map_err(|e| format!("a zstd frame: {e}"))
            })
            .and_then(|bound| {
                usize::try_from(bound)
                    .ok()
                    .filter(|&bound| bound <= most)
                    .ok_or_else(|| {
                        format!("a zstd frame of {bound} bytes, more than a {w}x{h} bitmap can be")
                    })
            });
        outputs.push(Vec::with_capacity(*bound.as_ref().unwrap_or(&0)));
        bounded.push(bound.map(|_| ()));
    }
    let inputs: Vec<&[u8]> = bodies.iter().map(|&(body, _, _)| body).collect();
    let decoded = ruzstd_decompress::decoding::FrameDecoder::new()
        .decode_each_parallel(&inputs, &mut outputs);
    bodies
        .par_iter()
        .zip(outputs)
        .zip(bounded)
        .zip(decoded)
        .map(|((((_, w, h), bitmap), bounded), decoded)| {
            bounded?;
            decoded.map_err(|e| format!("a zstd frame: {e}"))?;
            let (rgb, used) = decode(&bitmap, *w, *h)?;
            if used != bitmap.len() {
                return Err(format!(
                    "a planar bitmap of {} bytes, {used} of them its {w}x{h} pixels",
                    bitmap.len()
                ));
            }
            Ok(rgb)
        })
        .collect()
}

/// The still stream as a client reads it: each STILL body decompressed,
/// after the ones before it, to one bitmap of its rectangle, by ruzstd-decompress a
/// block at a time, the bitmap what the body's blocks decode to, which the
/// frame hands out as soon as they are decoded.
#[derive(Default)]
pub struct StillReader {
    frame: Option<ruzstd_decompress::decoding::FrameDecoder>,
    bitmap: Vec<u8>,
}

/// The largest window a still stream's frame may have, which the page's
/// reader holds it to as this one does.
const STILL_WINDOW_MOST: u64 = 8 << 20;

/// What a zstd frame begins with.
pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

impl StillReader {
    /// The `w`×`h` pixels of a STILL's `body`, `R, G, B` a pixel, top row
    /// first: a body that begins a zstd frame starts the stream over, and
    /// any other continues it. `Err` where it is not whole blocks of the
    /// frame, or decompresses to anything but one whole bitmap of that size.
    pub fn unpack(&mut self, body: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
        let mut source = body;
        if body.starts_with(&ZSTD_MAGIC) {
            let mut frame = ruzstd_decompress::decoding::FrameDecoder::new();
            frame.set_max_window_size(STILL_WINDOW_MOST);
            frame
                .init(&mut source)
                .map_err(|e| format!("the still stream's frame: {e}"))?;
            self.frame = Some(frame);
        }
        let frame = self
            .frame
            .as_mut()
            .ok_or("a STILL that continues no stream")?;
        // What a body before left uncollected, one that did not read, is not this one's.
        frame
            .collect_to_writer(std::io::sink())
            .map_err(|e| e.to_string())?;
        while !source.is_empty() {
            let header = source.get(..3).ok_or("a block header cut short")?;
            let header = u32::from_le_bytes([header[0], header[1], header[2], 0]);
            if header & 1 != 0 {
                return Err("a last block, which ends the stream's frame".into());
            }
            // An RLE block's content is its one byte.
            let content = if (header >> 1) & 3 == 1 {
                1
            } else {
                (header >> 3) as usize
            };
            let block = source.get(..3 + content).ok_or("a block cut short")?;
            let (read, _) = frame
                .decode_from_to(block, &mut [])
                .map_err(|e| format!("the still stream: {e}"))?;
            if read != block.len() {
                return Err("a block that does not decode".into());
            }
            source = &source[block.len()..];
        }
        self.bitmap.clear();
        frame
            .collect_to_writer(&mut self.bitmap)
            .map_err(|e| e.to_string())?;
        let (rgb, used) = decode(&self.bitmap, w, h)?;
        if used != self.bitmap.len() {
            return Err(format!(
                "a still of {} bytes, {used} of them its {w}x{h} pixels",
                self.bitmap.len()
            ));
        }
        Ok(rgb)
    }
}
