//! A planar bitmap read back as the specification lays it out
//! ([MS-RDPEGDI] 2.2.2.5.1 and its RLE planes, 2.2.2.5.1.1–3), written
//! for the tests from the text and not from the encoder, and the zstd frame
//! around it read by ruzstd; the examples' pages draw with it too.

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

/// The `w`×`h` pixels a [`wlshare_proto::tiles::KIND_PLANAR`] body carries,
/// `R, G, B` a pixel, top row first: one zstd frame and nothing after it,
/// read by ruzstd, of a planar bitmap of exactly that size.
pub fn unpack(body: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
    let mut source = body;
    let mut frame = ruzstd::decoding::StreamingDecoder::new(&mut source).map_err(|e| format!("a zstd frame: {e}"))?;
    let mut bitmap = Vec::new();
    std::io::Read::read_to_end(&mut frame, &mut bitmap).map_err(|e| format!("a zstd frame: {e}"))?;
    drop(frame);
    if !source.is_empty() {
        return Err(format!("{} bytes past the zstd frame", source.len()));
    }
    let (rgb, used) = decode(&bitmap, w, h)?;
    if used != bitmap.len() {
        return Err(format!("a planar bitmap of {} bytes, {used} of them its {w}x{h} pixels", bitmap.len()));
    }
    Ok(rgb)
}
