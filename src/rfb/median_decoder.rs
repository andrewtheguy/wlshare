//! A [`wlshare_proto::tiles::KIND_MEDIAN`] body read back as the protocol
//! lays it out (`wlshare_proto::tiles`), written for the tests from that
//! text and not from the encoder, its zstd frames read by
//! ruzstd-decompress; the examples' pages draw with it too.

/// What a zstd frame begins with.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// The three zstd frames `body` is, green's, red less green's and blue less
/// green's, or `Err` where it is anything but three frames one after
/// another: the decoders read every frame of an input, and skip skippable
/// ones, so each is held to begin a frame of its own.
pub fn frames(body: &[u8]) -> Result<[&[u8]; 3], String> {
    let mut rest = body;
    let mut frames = [&body[..0]; 3];
    for frame in &mut frames {
        if !rest.starts_with(&ZSTD_MAGIC) {
            return Err("a plane that begins no zstd frame".into());
        }
        let len = ruzstd_decompress::decoding::frame_compressed_size(rest)
            .map_err(|e| format!("a zstd frame: {e}"))?;
        (*frame, rest) = rest.split_at(len);
    }
    if !rest.is_empty() {
        return Err(format!("{} bytes past its three zstd frames", rest.len()));
    }
    Ok(frames)
}

/// The pixels each of an update's [`wlshare_proto::tiles::KIND_MEDIAN`]
/// bodies carries, `(body, w, h)`, `R, G, B` a pixel, top row first, read as
/// zDesk's page reads a batch's: every body's three frames decoded side by
/// side by ruzstd-decompress, each into the room of a plane of its
/// rectangle, which it must fill exactly, then each rectangle's planes
/// taken from their residuals side by side. One result for each body, in
/// order.
pub fn unpack_each(bodies: &[(&[u8], usize, usize)]) -> Vec<Result<Vec<u8>, String>> {
    use rayon::prelude::*;
    let mut inputs: Vec<&[u8]> = Vec::with_capacity(bodies.len() * 3);
    let mut outputs: Vec<Vec<u8>> = Vec::with_capacity(bodies.len() * 3);
    let mut framed: Vec<Result<(), String>> = Vec::with_capacity(bodies.len());
    for &(body, w, h) in bodies {
        let planes = frames(body).and_then(|planes| {
            for plane in planes {
                let bound = ruzstd_decompress::decoding::decompressed_bound(plane)
                    .map_err(|e| format!("a zstd frame: {e}"))?;
                if bound != (w * h) as u64 {
                    return Err(format!(
                        "a zstd frame of {bound} bytes, where a plane of {w}x{h} is {}",
                        w * h
                    ));
                }
            }
            Ok(planes)
        });
        // A body that is refused still takes its three places, with nothing
        // to decode in them.
        let room = if planes.is_ok() { w * h } else { 0 };
        inputs.extend(planes.as_ref().map_or([&body[..0]; 3], |planes| *planes));
        outputs.extend((0..3).map(|_| Vec::with_capacity(room)));
        framed.push(planes.map(|_| ()));
    }
    let decoded = ruzstd_decompress::decoding::FrameDecoder::new()
        .decode_each_parallel(&inputs, &mut outputs);
    bodies
        .par_iter()
        .zip(outputs.par_chunks_mut(3))
        .zip(decoded.par_chunks(3))
        .zip(framed)
        .map(|((((_, w, h), planes), decoded), framed)| {
            framed?;
            for (plane, decoded) in planes.iter_mut().zip(decoded) {
                decoded.as_ref().map_err(|e| format!("a zstd frame: {e}"))?;
                if plane.len() != w * h {
                    return Err(format!(
                        "a plane of {} residuals, where {w}x{h} has {}",
                        plane.len(),
                        w * h
                    ));
                }
                samples(plane, *w, *h);
            }
            let [green, red, blue] = planes else {
                unreachable!("three planes")
            };
            Ok(green
                .iter()
                .zip(red.iter())
                .zip(blue.iter())
                .flat_map(|((&g, &r), &b)| [r.wrapping_add(g), g, b.wrapping_add(g)])
                .collect())
        })
        .collect()
}

/// A `w`×`h` plane's samples from its residuals, in place: each its
/// residual and its prediction modulo 256, which is nothing for the first,
/// the sample to the left along the top row, the one above down the left
/// column, and elsewhere the median of the sample to the left, the one above
/// and left + above − above-left.
fn samples(plane: &mut [u8], w: usize, h: usize) {
    for y in 0..h {
        for x in 0..w {
            let at = |x: usize, y: usize| i32::from(plane[y * w + x]);
            let prediction = match (x, y) {
                (0, 0) => 0,
                (_, 0) => at(x - 1, 0),
                (0, _) => at(0, y - 1),
                _ => {
                    let (left, above) = (at(x - 1, y), at(x, y - 1));
                    let mut three = [left, above, left + above - at(x - 1, y - 1)];
                    three.sort_unstable();
                    three[1]
                }
            };
            plane[y * w + x] = (i32::from(plane[y * w + x]) + prediction).rem_euclid(256) as u8;
        }
    }
}
