//! A RemoteFX Progressive encoder: the codec a Windows host draws its
//! desktop with where it has no H.264 ([MS-RDPEGFX] 2.2.4 and 3.2.8.1), as
//! an encoder of this repository's own, for the progressive tiles
//! ([`wlshare_proto::tiles::KIND_PROGRESSIVE`]), whose tables and stream are
//! the protocol's ([`wlshare_proto::progressive`]). It is written to the
//! decoder the zDesk page runs (`zdesk-rdp-graphics`, `proto/progressive.rs`),
//! and its tests read every pass back with a decoder of their own.
//!
//! A 64×64 tile is converted to YCbCr in 11.5 fixed point, transformed by
//! three levels of the reduce-extrapolate 5/3 wavelet, quantized by a table
//! of shifts per sub-band, and sent in *passes*: a first pass at a
//! progressive stage, which holds back the low bits of every coefficient by
//! a second table of shifts, and upgrade passes that each send the bits the
//! stage before held back, for the coefficients the decoder already has as
//! raw bits and for the ones it has as zero in a run-length code. A changed
//! tile the decoder holds already is sent as the difference of its quantized
//! coefficients from what the decoder holds, where that has more zeros.
//!
//! The encoder keeps, for every tile, what the decoder holds — the
//! coefficients as sent so far, the sign the decoder has learnt of each, and
//! the stage each band stands at — so that each pass is coded against it,
//! and so that the picture the decoder shows can be had here without a
//! decoder ([`Encoder::shown`]).

use std::collections::HashMap;

use wlshare_proto::progressive::{WBT_TILE_FIRST, WBT_TILE_UPGRADE, block};

pub const TILE: usize = 64;
pub const COEFFS: usize = TILE * TILE;

/// A first pass's flag: the difference from what the decoder holds.
const TILE_DIFFERENCE: u8 = 0x01;

/// Where each sub-band sits in a component's 4096 coefficients, as
/// `(offset, count)`, in the order the quantization values are kept: HL1,
/// LH1, HH1, HL2, LH2, HH2, HL3, LH3, HH3, LL3.
pub const BANDS: [(usize, usize); 10] = [
    (0, 1023),
    (1023, 1023),
    (2046, 961),
    (3007, 272),
    (3279, 272),
    (3551, 256),
    (3807, 72),
    (3879, 72),
    (3951, 64),
    (4015, 81),
];
pub const LL3: usize = 9;

/// One shift per sub-band, in [`BANDS`] order: a quantization table, or a
/// stage's extra shifts.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Quant(pub [u8; 10]);

impl Quant {
    /// The shifts of the five packed bytes of an `RFX_COMPONENT_CODEC_QUANT`.
    pub fn from_bytes(b: [u8; 5]) -> Self {
        Self([b[3] >> 4, b[4] & 0xF, b[4] >> 4, b[2] & 0xF, b[2] >> 4, b[3] & 0xF, b[0] >> 4, b[1] & 0xF, b[1] >> 4, b[0] & 0xF])
    }
}

/// A progressive stage: the quality it is labelled with on the wire and the
/// bits each component's bands hold back at it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stage {
    pub quality: u8,
    pub shifts: [Quant; 3],
}

impl Stage {
    /// The full stage: nothing held back; `examples/progressive-stills.rs`
    /// ends its tables with it.
    #[allow(dead_code)]
    pub const FULL: Stage = Stage { quality: 100, shifts: [Quant([0; 10]); 3] };

    /// A quality label and the three components' packed shifts.
    pub fn from_bytes(b: &[u8]) -> Self {
        let q = |at: usize| Quant::from_bytes(b[at..at + 5].try_into().unwrap());
        Self { quality: b[0], shifts: [q(1), q(6), q(11)] }
    }
}

/// The tables an encoder codes with: one quantization table per component,
/// and the stages a tile is sent through, coarsest first, each holding back
/// no more than the one before in any band, the last of them full.
#[derive(Clone, Debug)]
pub struct Tables {
    pub quant: [Quant; 3],
    pub stages: Vec<Stage>,
}

impl Tables {
    /// The protocol's tables, which a decoder reads every tile with
    /// ([`wlshare_proto::progressive::QUANT`] and `STAGES`).
    pub fn protocol() -> Self {
        use wlshare_proto::progressive::{QUANT, STAGES};
        Self { quant: [Quant::from_bytes(QUANT); 3], stages: STAGES.iter().map(|stage| Stage::from_bytes(stage)).collect() }
    }

    /// Index of the full stage in the region's table, which the tile's
    /// quality byte names, or 0xFF for a stage holding nothing back.
    fn quality_byte(&self, stage: usize) -> u8 {
        if self.stages[stage].shifts == [Quant([0; 10]); 3] { 0xFF } else { stage as u8 }
    }
}

/// One tile as the decoder holds it, kept by the encoder.
#[derive(Clone)]
pub struct Held {
    /// The quantized coefficients the tile was last coded from: what the
    /// passes drive the decoder towards. Y, Cb, Cr, 4096 each.
    target: Vec<i16>,
    /// What the decoder holds of them, in the same units.
    held: Vec<i16>,
    /// The sign the decoder has learnt of each coefficient: what the first
    /// pass gave it, or an upgrade's run-length value.
    sign: Vec<i8>,
    /// The stage each component's bands stand at: the extra bits held back.
    shifts: [Quant; 3],
    /// Whether every band is at the full stage.
    pub full: bool,
}

/// One pass as coded: its tile block for the wire, and what it was.
pub struct Pass {
    pub block: Vec<u8>,
    /// A first pass coded as a difference from what the decoder held, which
    /// `examples/progressive-stills.rs` counts.
    #[allow(dead_code)]
    pub diff: bool,
}

pub struct Encoder {
    pub tables: Tables,
    cols: usize,
    tiles: HashMap<(u16, u16), Held>,
    /// Scratch: the tile's YCbCr, transformed in place.
    work: Vec<i32>,
    temp: Vec<i32>,
    /// First passes [`Self::precode`] coded ahead, by tile, dropped where
    /// anything else touches the tile first.
    ready: HashMap<(u16, u16), Ready>,
}

/// A first pass coded ahead: the stage and slack it was coded at, the pass,
/// and what the decoder holds after it.
struct Ready {
    stage: usize,
    slack: f32,
    pass: Pass,
    held: Option<Held>,
}

/// A tile [`Encoder::precode`] codes: its cell, where its pixels start, and
/// its width and height under the picture's edge.
pub struct Job {
    pub cell: (u16, u16),
    pub at: usize,
    pub w: usize,
    pub h: usize,
}

impl Encoder {
    pub fn new(width: u16, tables: Tables) -> Self {
        assert!(!tables.stages.is_empty(), "a stage to code at");
        Self { tables, cols: usize::from(width).div_ceil(TILE), tiles: HashMap::new(), work: vec![0; COEFFS * 3], temp: vec![0; COEFFS * 2], ready: HashMap::new() }
    }

    /// Code each of `jobs` as [`Self::first_or_full`] would, on up to
    /// `threads` threads, each a worker holding the one tile it codes, and
    /// keep the passes for the calls that ask for them; the decoder's state
    /// is untouched until they do.
    pub fn precode(&mut self, jobs: &[Job], pixels: &[u8], stride: usize, stage: usize, slack: f32, threads: usize) {
        let threads = threads.min(jobs.len());
        if threads < 2 {
            return;
        }
        let next = std::sync::atomic::AtomicUsize::new(0);
        let (tables, tiles, cols) = (&self.tables, &self.tiles, self.cols);
        let done: Vec<Vec<_>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        let mut worker = Encoder { tables: tables.clone(), cols, tiles: HashMap::new(), work: vec![0; COEFFS * 3], temp: vec![0; COEFFS * 2], ready: HashMap::new() };
                        let mut done = Vec::new();
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(job) = jobs.get(i) else { break };
                            worker.tiles.clear();
                            if let Some(held) = tiles.get(&job.cell) {
                                worker.tiles.insert(job.cell, held.clone());
                            }
                            let pass = worker.first_or_full(job.cell, &pixels[job.at..], stride, job.w, job.h, stage, slack);
                            done.push((job.cell, pass, worker.tiles.remove(&job.cell)));
                        }
                        done
                    })
                })
                .collect();
            workers.into_iter().map(|w| w.join().expect("a precoding thread")).collect()
        });
        for (cell, pass, held) in done.into_iter().flatten() {
            self.ready.insert(cell, Ready { stage, slack, pass, held });
        }
    }

    /// Drop every pass [`Self::precode`] coded that nothing asked for.
    pub fn drop_ready(&mut self) {
        self.ready.clear();
    }

    /// The tiles across the picture, which `examples/progressive-stills.rs`
    /// sizes its encoders by.
    #[allow(dead_code)]
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// The decoder holds nothing.
    pub fn forget(&mut self) {
        self.tiles.clear();
        self.ready.clear();
    }

    pub fn forget_tile(&mut self, cell: (u16, u16)) {
        self.tiles.remove(&cell);
        self.ready.remove(&cell);
    }

    #[cfg(test)]
    pub fn held(&self, cell: (u16, u16)) -> Option<&Held> {
        self.tiles.get(&cell)
    }

    /// Code tile `cell` of the picture — `B, G, R, X` pixels, `stride` bytes a
    /// row, `w`×`h` of them under the tile where it is cut by the picture's
    /// edge, the rest padded by the last pixel — as a first pass at
    /// `stage`, a difference from what the decoder holds where it holds the
    /// tile and the difference has more zeros, and leave the tile owed the
    /// stages after.
    #[allow(clippy::too_many_arguments)]
    pub fn first(&mut self, cell: (u16, u16), pixels: &[u8], stride: usize, w: usize, h: usize, stage: usize) -> Pass {
        self.ready.remove(&cell);
        let target = self.analyse(pixels, stride, w, h);
        self.code_first(cell, target, stage)
    }

    /// Code tile `cell` as [`Self::first`] does, at the full stage where its
    /// first pass there costs at most `slack` times the one at `stage`, or
    /// 64 bytes more, as a flat tile's does: refining it would cost more
    /// than the difference. Otherwise at `stage`. The full stage is tried
    /// only where it could be so: the pass at `stage` under 96 bytes, or the
    /// full stage keeping at most half as many coefficients again and 32 more
    /// (measured: every tile sent whole over the window and mixed sessions at
    /// 1920x1080, from a fifth of the tries).
    #[allow(clippy::too_many_arguments)]
    pub fn first_or_full(&mut self, cell: (u16, u16), pixels: &[u8], stride: usize, w: usize, h: usize, stage: usize, slack: f32) -> Pass {
        if let Some(Ready { stage: at, slack: by, pass, held }) = self.ready.remove(&cell)
            && (at, by) == (stage, slack)
        {
            match held {
                Some(held) => self.tiles.insert(cell, held),
                None => self.tiles.remove(&cell),
            };
            return pass;
        }
        let full = self.tables.stages.len() - 1;
        let target = self.analyse(pixels, stride, w, h);
        if stage == full {
            return self.code_first(cell, target, full);
        }
        let (kept_full, kept) = self.kept(&target, stage);
        let before = self.tiles.get(&cell).cloned();
        let coarse = self.code_first(cell, target.clone(), stage);
        if coarse.block.len() >= 96 && kept_full * 2 > kept * 3 + 64 {
            return coarse;
        }
        let coarse_held = self.tiles.remove(&cell);
        if let Some(before) = before {
            self.tiles.insert(cell, before);
        }
        let whole = self.code_first(cell, target, full);
        if whole.block.len() as f32 <= (coarse.block.len() as f32 * slack).max(coarse.block.len() as f32 + 64.0) {
            return whole;
        }
        if let Some(held) = coarse_held {
            self.tiles.insert(cell, held);
        }
        coarse
    }

    /// How many of `target`'s coefficients are not zero, and how many of
    /// them the shifts of `stage` keep.
    fn kept(&self, target: &[i16], stage: usize) -> (u32, u32) {
        let (mut all, mut kept) = (0, 0);
        for (c, shifts) in self.tables.stages[stage].shifts.iter().enumerate() {
            for (&(at, len), &shift) in BANDS.iter().zip(&shifts.0) {
                for &t in &target[c * COEFFS + at..c * COEFFS + at + len] {
                    all += u32::from(t != 0);
                    kept += u32::from(i32::from(t).abs() >> shift != 0);
                }
            }
        }
        (all, kept)
    }

    /// The stage tile `cell` stands at, where the decoder holds it.
    pub fn stage_of(&self, cell: (u16, u16)) -> Option<usize> {
        let held = self.tiles.get(&cell)?;
        Some(self.tables.stages.iter().position(|stage| stage.shifts == held.shifts).unwrap_or(0))
    }

    /// The tile's quantized coefficients: YCbCr, the wavelet, the quantizer.
    fn analyse(&mut self, pixels: &[u8], stride: usize, w: usize, h: usize) -> Vec<i16> {
        to_ycbcr(pixels, stride, w, h, &mut self.work);
        for c in 0..3 {
            let comp = &mut self.work[c * COEFFS..(c + 1) * COEFFS];
            dwt(comp, &mut self.temp);
            quantize(comp, self.tables.quant[c]);
        }
        self.work.iter().map(|&v| v.clamp(i16::MIN.into(), i16::MAX.into()) as i16).collect()
    }

    fn code_first(&mut self, cell: (u16, u16), target: Vec<i16>, stage: usize) -> Pass {
        let shifts = self.tables.stages[stage].shifts;
        let diff = self.tiles.get(&cell).is_some_and(|held| prefer_diff(&target, &held.held));
        let held = self.tiles.entry(cell).or_insert_with(|| Held { target: vec![0; COEFFS * 3], held: vec![0; COEFFS * 3], sign: vec![0; COEFFS * 3], shifts, full: false });
        if !diff {
            held.held.fill(0);
        }
        held.target = target;
        held.shifts = shifts;
        let mut data = [Vec::new(), Vec::new(), Vec::new()];
        let mut coded = vec![0i16; COEFFS];
        for c in 0..3 {
            let target = &held.target[c * COEFFS..(c + 1) * COEFFS];
            let have = &mut held.held[c * COEFFS..(c + 1) * COEFFS];
            let sign = &mut held.sign[c * COEFFS..(c + 1) * COEFFS];
            for (band, &(at, len)) in BANDS.iter().enumerate() {
                let p = u32::from(shifts[c].0[band]);
                for i in at..at + len {
                    let sb = i32::from(target[i]) - i32::from(have[i]);
                    let q = if band == LL3 { sb >> p } else { toward_zero(sb, p) };
                    have[i] = (i32::from(have[i]) + (q << p)) as i16;
                    sign[i] = q.signum() as i8;
                    coded[i] = q as i16;
                }
            }
            let (ll, len) = BANDS[LL3];
            for i in (ll + 1..ll + len).rev() {
                coded[i] = coded[i].wrapping_sub(coded[i - 1]);
            }
            data[c] = rlgr1_encode(&coded);
        }
        held.full = shifts == [Quant([0; 10]); 3];
        let mut b = vec![0u8, 1, 2];
        b.extend_from_slice(&cell.0.to_le_bytes());
        b.extend_from_slice(&cell.1.to_le_bytes());
        b.push(if diff { TILE_DIFFERENCE } else { 0 });
        b.push(self.tables.quality_byte(stage));
        for len in [data[0].len(), data[1].len(), data[2].len(), 0] {
            b.extend_from_slice(&(len as u16).to_le_bytes());
        }
        for d in &data {
            b.extend_from_slice(d);
        }
        Pass { block: block(WBT_TILE_FIRST, &b), diff }
    }

    /// Send tile `cell` the bits between the stage it stands at and `stage`,
    /// which holds back no more than it in any band; `None` where it stands
    /// there already, or the decoder does not hold it.
    pub fn upgrade(&mut self, cell: (u16, u16), stage: usize) -> Option<Vec<u8>> {
        self.ready.remove(&cell);
        let to = self.tables.stages[stage].shifts;
        let held = self.tiles.get_mut(&cell)?;
        if held.shifts == to {
            return None;
        }
        let mut streams: [Vec<u8>; 6] = Default::default();
        for c in 0..3 {
            let target = &held.target[c * COEFFS..(c + 1) * COEFFS];
            let have = &mut held.held[c * COEFFS..(c + 1) * COEFFS];
            let sign = &mut held.sign[c * COEFFS..(c + 1) * COEFFS];
            let mut srl = SrlWriter::new();
            let mut raw = BitWriter::new();
            for (band, &(at, len)) in BANDS.iter().enumerate() {
                let (from, p) = (u32::from(held.shifts[c].0[band]), u32::from(to[c].0[band]));
                assert!(from >= p, "an upgrade holds back no more than the pass before it");
                let bits = from - p;
                if bits == 0 {
                    continue;
                }
                for i in at..at + len {
                    let rem = i32::from(target[i]) - i32::from(have[i]);
                    if band == LL3 {
                        let v = rem >> p;
                        debug_assert!((0..1 << bits).contains(&v));
                        raw.put(v as u32, bits);
                        have[i] = (i32::from(have[i]) + (v << p)) as i16;
                    } else if sign[i] == 0 {
                        let v = toward_zero(rem, p);
                        debug_assert!(v.unsigned_abs() < 1 << bits);
                        srl.put(v, bits);
                        sign[i] = v.signum() as i8;
                        have[i] = (i32::from(have[i]) + (v << p)) as i16;
                    } else {
                        debug_assert!(rem == 0 || rem.signum() == i32::from(sign[i]));
                        let v = rem.unsigned_abs() >> p;
                        debug_assert!(v < 1 << bits);
                        raw.put(v, bits);
                        let v = (v as i32) << p;
                        have[i] = (i32::from(have[i]) + if sign[i] < 0 { -v } else { v }) as i16;
                    }
                }
            }
            streams[c * 2] = srl.finish();
            streams[c * 2 + 1] = raw.finish();
        }
        held.shifts = to;
        held.full = to == [Quant([0; 10]); 3];
        let mut b = vec![0u8, 1, 2];
        b.extend_from_slice(&cell.0.to_le_bytes());
        b.extend_from_slice(&cell.1.to_le_bytes());
        b.push(self.tables.quality_byte(stage));
        for s in &streams {
            b.extend_from_slice(&(s.len() as u16).to_le_bytes());
        }
        for s in &streams {
            b.extend_from_slice(s);
        }
        Some(block(WBT_TILE_UPGRADE, &b))
    }

    /// The tile as the decoder shows it from what it holds: 64×64 pixels,
    /// `B, G, R, X`, or none where it holds nothing.
    pub fn shown(&mut self, cell: (u16, u16)) -> Option<Vec<u8>> {
        let held = self.tiles.get(&cell)?;
        for c in 0..3 {
            let comp = &mut self.work[c * COEFFS..(c + 1) * COEFFS];
            for (band, &(at, len)) in BANDS.iter().enumerate() {
                let s = u32::from(self.tables.quant[c].0[band]) - 1;
                let held = &held.held[c * COEFFS + at..c * COEFFS + at + len];
                for (v, &h) in comp[at..at + len].iter_mut().zip(held) {
                    *v = i32::from((i32::from(h) << s) as i16);
                }
            }
            idwt(comp, &mut self.temp);
        }
        let mut pixels = vec![0u8; COEFFS * 4];
        to_bgrx(&self.work, &mut pixels);
        Some(pixels)
    }
}

/// `v / 2^p` rounded toward zero.
fn toward_zero(v: i32, p: u32) -> i32 {
    if v < 0 { -((-v) >> p) } else { v >> p }
}

/// Whether to send the difference: it has more zeros among Y's non-LL
/// coefficients than the tile itself, [MS-RDPEGFX] 3.2.8.1.4.
fn prefer_diff(target: &[i16], held: &[i16]) -> bool {
    let (ll, _) = BANDS[LL3];
    let (mut original, mut difference) = (0usize, 0usize);
    for i in 0..ll {
        original += usize::from(target[i] == 0);
        difference += usize::from(target[i] == held[i]);
    }
    difference > original
}

// ── Colour ───────────────────────────────────────────────────────────────────

/// The tile's pixels to Y, Cb, Cr in 11.5 fixed point, the reference's
/// constants, each component 4096 values; a tile cut by the picture's edge is
/// padded with its last row and column.
fn to_ycbcr(pixels: &[u8], stride: usize, w: usize, h: usize, out: &mut [i32]) {
    for y in 0..TILE {
        let row = y.min(h - 1);
        for x in 0..TILE {
            let col = x.min(w - 1);
            let px = &pixels[row * stride + col * 4..][..4];
            let (b, g, r) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
            let cy = (r * 9798 + g * 19235 + b * 3735) >> 10;
            let cb = (r * -5535 + g * -10868 + b * 16403) >> 10;
            let cr = (r * 16377 + g * -13714 + b * -2663) >> 10;
            let i = y * TILE + x;
            out[i] = (cy - 4096).clamp(-4096, 4095);
            out[COEFFS + i] = cb.clamp(-4096, 4095);
            out[2 * COEFFS + i] = cr.clamp(-4096, 4095);
        }
    }
}

/// The decoder's conversion back, [MS-RDPEGFX]'s reference fixed point.
fn to_bgrx(work: &[i32], pixels: &mut [u8]) {
    for i in 0..COEFFS {
        let y = i64::from(work[i]) + 4096;
        let (cb, cr) = (i64::from(work[COEFFS + i]), i64::from(work[2 * COEFFS + i]));
        let r = ((y << 16) + cr * 91916) >> 21;
        let g = ((y << 16) - cb * 22527 - cr * 46819) >> 21;
        let b = ((y << 16) + cb * 115992) >> 21;
        let px = &mut pixels[i * 4..i * 4 + 4];
        px[0] = b.clamp(0, 255) as u8;
        px[1] = g.clamp(0, 255) as u8;
        px[2] = r.clamp(0, 255) as u8;
        px[3] = 0;
    }
}

// ── The wavelet ──────────────────────────────────────────────────────────────

/// `v / 2` towards zero, as the decoder halves.
fn half(v: i32) -> i32 {
    (v + ((v >> 31) & 1)) >> 1
}

fn band_l(n: usize) -> usize {
    n / 2 + 1
}

fn band_h(n: usize) -> usize {
    if n.is_multiple_of(2) { n / 2 - 1 } else { n / 2 }
}

/// One line of `n` samples to its `band_l(n)` low and `band_h(n)` high
/// coefficients, [MS-RDPEGFX] 3.2.8.1.2.2: a line of 64 is extrapolated to
/// 65 and its last high coefficient, zero, dropped; a line of 33 or 17 is
/// mirrored at both ends. The reference [`dwt_lines`] is tested against.
#[cfg(test)]
fn dwt_line(x: &[i32], low: &mut [i32], high: &mut [i32]) {
    let n = x.len();
    let (nl, nh) = (band_l(n), band_h(n));
    let at = |i: usize| if i < n { x[i] } else { 2 * x[n - 1] - x[n - 2] };
    for k in 0..nh {
        high[k] = (x[2 * k + 1] - half(x[2 * k] + at(2 * k + 2))) >> 1;
    }
    low[0] = x[0] + high[0];
    for k in 1..nh {
        low[k] = x[2 * k] + half(high[k - 1] + high[k]);
    }
    if n.is_multiple_of(2) {
        low[nh] = x[2 * nh] + half(high[nh - 1]);
        low[nl - 1] = at(n);
    } else {
        low[nh] = x[2 * nh] + high[nh - 1];
    }
}

/// The inverse of [`dwt_line`], as the decoder runs it.
fn idwt_line(low: &[i32], high: &[i32], x: &mut [i32]) {
    let n = x.len();
    let (nl, nh) = (band_l(n), band_h(n));
    x[0] = low[0] - high[0];
    for k in 1..nh {
        x[2 * k] = low[k] - half(high[k - 1] + high[k]);
    }
    if nl == nh + 1 {
        x[2 * nh] = low[nh] - high[nh - 1];
    } else {
        x[2 * nh] = low[nh] - half(high[nh - 1]);
    }
    for k in 0..nh {
        x[2 * k + 1] = half(x[2 * k] + x[2 * k + 2]) + 2 * high[k];
    }
    if nl > nh + 1 {
        x[2 * nh + 1] = half(x[2 * nh] + low[nh + 1]);
    }
}

/// [`dwt_line`] down `w` lines at once: `x` is `n` rows of `w` samples, a
/// line down each column, and `out` takes the `band_l(n)` low rows, then the
/// `band_h(n)` high ones, every row a sample of each line, so that each
/// step runs along a row.
fn dwt_lines(x: &[i32], n: usize, w: usize, out: &mut [i32]) {
    let (nl, nh) = (band_l(n), band_h(n));
    let row = |i: usize| &x[i * w..(i + 1) * w];
    let (low, high) = out[..n * w].split_at_mut(nl * w);
    for (k, h) in high.chunks_exact_mut(w).enumerate() {
        for (((h, &a), &b), &c) in h.iter_mut().zip(row(2 * k)).zip(row(2 * k + 1)).zip(row(2 * k + 2)) {
            *h = (b - half(a + c)) >> 1;
        }
    }
    let high = &*high;
    for ((l, &a), &h) in low[..w].iter_mut().zip(row(0)).zip(&high[..w]) {
        *l = a + h;
    }
    for k in 1..nh {
        let (before, after) = (&high[(k - 1) * w..k * w], &high[k * w..(k + 1) * w]);
        for (((l, &a), &h0), &h1) in low[k * w..(k + 1) * w].iter_mut().zip(row(2 * k)).zip(before).zip(after) {
            *l = a + half(h0 + h1);
        }
    }
    let last = &high[(nh - 1) * w..nh * w];
    let edge = &mut low[nh * w..(nh + 1) * w];
    if n.is_multiple_of(2) {
        for ((l, &a), &h) in edge.iter_mut().zip(row(2 * nh)).zip(last) {
            *l = a + half(h);
        }
        for ((l, &a), &b) in low[(nl - 1) * w..nl * w].iter_mut().zip(row(n - 1)).zip(row(n - 2)) {
            *l = 2 * a - b;
        }
    } else {
        for ((l, &a), &h) in edge.iter_mut().zip(row(2 * nh)).zip(last) {
            *l = a + h;
        }
    }
}

/// [`dwt_line`] along one row, its even and odd samples apart first, so that
/// each step runs along an array.
fn dwt_row(x: &[i32], low: &mut [i32], high: &mut [i32]) {
    let n = x.len();
    let (nl, nh) = (band_l(n), band_h(n));
    let mut even = [0; TILE / 2 + 1];
    let mut odd = [0; TILE / 2];
    for (i, pair) in x.chunks(2).enumerate() {
        even[i] = pair[0];
        if let Some(&v) = pair.get(1) {
            odd[i] = v;
        }
    }
    for (((h, &b), &a), &c) in high[..nh].iter_mut().zip(&odd).zip(&even).zip(&even[1..]) {
        *h = (b - half(a + c)) >> 1;
    }
    low[0] = even[0] + high[0];
    for (((l, &a), &h0), &h1) in low[1..nh].iter_mut().zip(&even[1..]).zip(&high[..nh]).zip(&high[1..nh]) {
        *l = a + half(h0 + h1);
    }
    if n.is_multiple_of(2) {
        low[nh] = even[nh] + half(high[nh - 1]);
        low[nl - 1] = 2 * x[n - 1] - x[n - 2];
    } else {
        low[nh] = even[nh] + high[nh - 1];
    }
}

/// One level over the `n`×`n` picture at the front of `buf`: its four bands
/// written over it, HL, LH, HH, then LL, the next level's input.
fn dwt_level(buf: &mut [i32], temp: &mut [i32], n: usize) {
    let (nl, nh) = (band_l(n), band_h(n));
    // Vertical: low rows, then high rows.
    let rows = &mut temp[..n * n];
    dwt_lines(&buf[..n * n], n, n, rows);
    // Horizontal: each low row to LL and HL, each high row to LH and HH, HL
    // first.
    let (lh, hh, ll) = (nl * nh, nl * nh + nh * nl, nl * nh + nh * nl + nh * nh);
    let (lows, highs) = rows.split_at(nl * n);
    let (front, ll_band) = buf.split_at_mut(ll);
    let (hl_band, rest) = front.split_at_mut(lh);
    let (lh_band, hh_band) = rest.split_at_mut(hh - lh);
    for ((row, low), high) in lows.chunks_exact(n).zip(ll_band.chunks_exact_mut(nl)).zip(hl_band.chunks_exact_mut(nh)) {
        dwt_row(row, low, high);
    }
    for ((row, low), high) in highs.chunks_exact(n).zip(lh_band.chunks_exact_mut(nl)).zip(hh_band.chunks_exact_mut(nh)) {
        dwt_row(row, low, high);
    }
}

fn idwt_level(buf: &mut [i32], temp: &mut [i32], n: usize) {
    let (nl, nh) = (band_l(n), band_h(n));
    let (hl, lh, hh, ll) = (0, nl * nh, nl * nh + nh * nl, nl * nh + nh * nl + nh * nh);
    let rows = &mut temp[..n * n];
    let mut line = vec![0; n];
    for r in 0..nl {
        idwt_line(&buf[ll + r * nl..ll + (r + 1) * nl], &buf[hl + r * nh..hl + (r + 1) * nh], &mut line);
        rows[r * n..(r + 1) * n].copy_from_slice(&line);
    }
    for r in 0..nh {
        idwt_line(&buf[lh + r * nl..lh + (r + 1) * nl], &buf[hh + r * nh..hh + (r + 1) * nh], &mut line);
        rows[(nl + r) * n..(nl + r + 1) * n].copy_from_slice(&line);
    }
    let (mut low, mut high) = (vec![0; nl], vec![0; nh]);
    for x in 0..n {
        for k in 0..nl {
            low[k] = rows[k * n + x];
        }
        for k in 0..nh {
            high[k] = rows[(nl + k) * n + x];
        }
        idwt_line(&low, &high, &mut line);
        for y in 0..n {
            buf[y * n + x] = line[y];
        }
    }
}

/// Three levels over a component, into the [`BANDS`] layout.
fn dwt(comp: &mut [i32], temp: &mut [i32]) {
    dwt_level(comp, temp, 64);
    dwt_level(&mut comp[3007..], temp, 33);
    dwt_level(&mut comp[3807..], temp, 17);
}

fn idwt(comp: &mut [i32], temp: &mut [i32]) {
    idwt_level(&mut comp[3807..], temp, 17);
    idwt_level(&mut comp[3007..], temp, 33);
    idwt_level(comp, temp, 64);
}

/// Each band divided by its quantization scale, rounded to nearest, the
/// 11.5 fixed point's five bits going with it: a shift of the value less one.
fn quantize(comp: &mut [i32], quant: Quant) {
    for (band, &(at, len)) in BANDS.iter().enumerate() {
        let s = u32::from(quant.0[band]) - 1;
        for v in &mut comp[at..at + len] {
            *v = (*v + (1 << (s - 1))) >> s;
        }
    }
}

// ── Entropy coding ───────────────────────────────────────────────────────────

/// Bits written most significant first, padded with zeros to a byte.
struct BitWriter {
    bytes: Vec<u8>,
    used: usize,
}

impl BitWriter {
    fn new() -> Self {
        Self { bytes: Vec::new(), used: 0 }
    }

    fn put(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let bit = ((value >> i) & 1) as u8;
            let last = self.bytes.len() - 1;
            self.bytes[last] |= bit << (7 - (self.used % 8));
            self.used += 1;
        }
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

const KPMAX: u32 = 80;

/// Golomb-Rice: `code` as `code >> kr` ones, a zero, then `kr` low bits, and
/// the decoder's `kr` update.
fn golomb(w: &mut BitWriter, code: u32, kr: &mut u32, krp: &mut u32) {
    let vk = code >> *kr;
    for _ in 0..vk {
        w.put(1, 1);
    }
    w.put(0, 1);
    w.put(code & ((1 << *kr) - 1), *kr);
    if vk == 0 {
        *krp = krp.saturating_sub(2);
    } else if vk != 1 {
        *krp = (*krp + vk).min(KPMAX);
    }
    *kr = *krp >> 3;
}

/// RLGR1 entropy coding of a component's coefficients, [MS-RDPRFX]
/// 3.1.8.1.7.1; trailing zeros are left to the decoder's zero fill, and a
/// component of nothing is one zero byte, which the decoder reads as zeros.
pub fn rlgr1_encode(coeffs: &[i16]) -> Vec<u8> {
    let end = coeffs.iter().rposition(|&c| c != 0).map_or(0, |p| p + 1);
    let coeffs = &coeffs[..end];
    let mut w = BitWriter::new();
    let (mut k, mut kp, mut kr, mut krp) = (1u32, 8u32, 1u32, 8u32);
    let mut i = 0;
    while i < coeffs.len() {
        if k > 0 {
            let mut run = 0u32;
            while coeffs[i] == 0 {
                run += 1;
                i += 1;
            }
            while run >= (1 << k) {
                w.put(0, 1);
                run -= 1 << k;
                kp = (kp + 4).min(KPMAX);
                k = kp >> 3;
            }
            w.put(1, 1);
            w.put(run, k);
            let v = i32::from(coeffs[i]);
            i += 1;
            w.put(u32::from(v < 0), 1);
            golomb(&mut w, v.unsigned_abs() - 1, &mut kr, &mut krp);
            kp = kp.saturating_sub(6);
            k = kp >> 3;
        } else {
            let v = i32::from(coeffs[i]);
            i += 1;
            let code = if v < 0 { 2 * v.unsigned_abs() - 1 } else { 2 * v as u32 };
            golomb(&mut w, code, &mut kr, &mut krp);
            kp = if code == 0 { (kp + 3).min(KPMAX) } else { kp.saturating_sub(3) };
            k = kp >> 3;
        }
    }
    let mut bytes = w.finish();
    if bytes.is_empty() {
        bytes.push(0);
    }
    bytes
}

/// The simplified run-length coder of an upgrade pass, [MS-RDPEGFX]
/// 3.1.8.1.5: runs of zeros as RLGR codes them, a nonzero value as its sign
/// and its magnitude in unary, as many bits as the band's upgrade adds.
struct SrlWriter {
    w: BitWriter,
    kp: u32,
    zeros: u32,
}

impl SrlWriter {
    fn new() -> Self {
        Self { w: BitWriter::new(), kp: 8, zeros: 0 }
    }

    fn put(&mut self, v: i32, bits: u32) {
        if v == 0 {
            self.zeros += 1;
            return;
        }
        let mut k = self.kp >> 3;
        while self.zeros >= (1 << k) {
            self.w.put(0, 1);
            self.zeros -= 1 << k;
            self.kp = (self.kp + 4).min(KPMAX);
            k = self.kp >> 3;
        }
        self.w.put(1, 1);
        if k > 0 {
            self.w.put(self.zeros, k);
        }
        self.zeros = 0;
        self.w.put(u32::from(v < 0), 1);
        self.kp = self.kp.saturating_sub(6);
        if bits > 1 {
            let mag = v.unsigned_abs();
            let max = (1u32 << bits) - 1;
            self.w.put(0, mag - 1);
            if mag < max {
                self.w.put(1, 1);
            }
        }
    }

    /// The stream, its trailing zeros left to the decoder's reading of zeros
    /// past the end, with the zero byte the specification has after the last.
    fn finish(mut self) -> Vec<u8> {
        self.w.put(0, 8);
        self.w.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let mut px = vec![0u8; COEFFS * 4];
        for y in 0..TILE {
            for x in 0..TILE {
                let [r, g, b] = f(x, y);
                px[(y * TILE + x) * 4..][..4].copy_from_slice(&[b, g, r, 0]);
            }
        }
        px
    }

    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        let mut se = 0f64;
        let mut n = 0usize;
        for (pa, pb) in a.chunks(4).zip(b.chunks(4)) {
            for c in 0..3 {
                let d = f64::from(pa[c]) - f64::from(pb[c]);
                se += d * d;
                n += 1;
            }
        }
        if se == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 * n as f64 / se).log10() }
    }

    fn tables() -> Tables {
        Tables { quant: [Quant([6; 10]); 3], stages: vec![Stage { quality: 25, shifts: [Quant([5, 5, 5, 4, 4, 4, 3, 3, 3, 2]); 3] }, Stage { quality: 50, shifts: [Quant([3, 3, 3, 2, 2, 2, 1, 1, 1, 1]); 3] }, Stage::FULL] }
    }

    /// Every pass of a sequence — first passes at each stage and whole,
    /// upgrades to full, a changed tile as a difference, a tile forgotten
    /// and sent again — read back from the protocol's stream by a decoder of
    /// its own shows what the encoder says the client shows, byte for byte.
    #[test]
    fn every_pass_reads_back_as_the_encoder_says() {
        let pictures = [
            tile(|x, y| {
                let h = ((x * 2654435761usize) ^ (y * 40503)) % 251;
                [h as u8, (h * 7 % 256) as u8, (x * 4) as u8]
            }),
            tile(|x, y| if (x * 7 + y * 13) % 11 < 3 && y % 9 != 0 { [0, 0, 0] } else { [255, 255, 255] }),
            tile(|x, y| [(x * 4) as u8, (y * 4) as u8, ((x ^ y) * 4) as u8]),
            tile(|_, _| [37, 120, 200]),
        ];
        let mut enc = Encoder::new(256, Tables::protocol());
        let mut dec = super::decoder::Decoder::default();
        let full = enc.tables.stages.len() - 1;
        let mut frame = 0;
        let mut check = |enc: &mut Encoder, cell: (u16, u16), block: Vec<u8>, what: &str| {
            frame += 1;
            let rect = (cell.0 * 64, cell.1 * 64, 64, 64);
            let shown = dec.decode(&wlshare_proto::progressive::stream(frame, &[rect], &[&block]));
            assert_eq!(shown.len(), 1);
            assert_eq!(shown[0].0, cell, "{what}: the block's tile");
            assert!(shown[0].1 == enc.shown(cell).unwrap(), "{what}: the decoder shows what the encoder says");
        };
        for (i, px) in pictures.iter().enumerate() {
            let cell = (i as u16, 0);
            for stage in 0..full {
                enc.forget_tile(cell);
                let pass = enc.first_or_full(cell, px, TILE * 4, TILE, TILE, stage, 1.25);
                check(&mut enc, cell, pass.block, &format!("tile {i}, a first pass at {stage}"));
                while let Some(stood) = enc.stage_of(cell).filter(|&s| s < full) {
                    let up = enc.upgrade(cell, stood + 1).expect("an upgrade");
                    check(&mut enc, cell, up, &format!("tile {i}, an upgrade to {}", stood + 1));
                }
            }
            // Changed under the decoder: a difference where it has more zeros.
            let mut changed = px.clone();
            for p in changed[..TILE * 4 * 16].chunks_mut(4) {
                p[1] = p[1].wrapping_add(40);
            }
            let pass = enc.first(cell, &changed, TILE * 4, TILE, TILE, 0);
            check(&mut enc, cell, pass.block, &format!("tile {i}, changed"));
            let up = enc.upgrade(cell, full).expect("to full");
            check(&mut enc, cell, up, &format!("tile {i}, changed, to full"));
            assert!(psnr(&enc.shown(cell).unwrap(), &changed) > 30.0, "tile {i} at full: {:.1} dB", psnr(&enc.shown(cell).unwrap(), &changed));
        }
    }

    /// [`dwt_level`] a line at a time, by [`dwt_line`].
    fn line_level(buf: &mut [i32], temp: &mut [i32], n: usize) {
        let (nl, nh) = (band_l(n), band_h(n));
        let (input, rest) = temp.split_at_mut(n * n);
        input.copy_from_slice(&buf[..n * n]);
        // Vertical: each column to a low and a high column, as rows of `rows`.
        let rows = &mut rest[..n * n];
        let (mut line, mut low, mut high) = (vec![0; n], vec![0; nl], vec![0; nh]);
        for x in 0..n {
            for y in 0..n {
                line[y] = input[y * n + x];
            }
            dwt_line(&line, &mut low, &mut high);
            for k in 0..nl {
                rows[k * n + x] = low[k];
            }
            for k in 0..nh {
                rows[(nl + k) * n + x] = high[k];
            }
        }
        // Horizontal: each low row to LL and HL, each high row to LH and HH.
        let (hl, lh, hh, ll) = (0, nl * nh, nl * nh + nh * nl, nl * nh + nh * nl + nh * nh);
        for r in 0..nl {
            dwt_line(&rows[r * n..(r + 1) * n], &mut low, &mut high);
            buf[ll + r * nl..ll + (r + 1) * nl].copy_from_slice(&low);
            buf[hl + r * nh..hl + (r + 1) * nh].copy_from_slice(&high);
        }
        for r in 0..nh {
            dwt_line(&rows[(nl + r) * n..(nl + r + 1) * n], &mut low, &mut high);
            buf[lh + r * nl..lh + (r + 1) * nl].copy_from_slice(&low);
            buf[hh + r * nh..hh + (r + 1) * nh].copy_from_slice(&high);
        }
    }

    /// The wavelet down every line at once is the one a line at a time.
    #[test]
    fn the_wavelet_is_its_lines() {
        for n in [64usize, 33, 17] {
            let picture: Vec<i32> = (0..n * n).map(|i| ((i * 7919 + 13) % 8191) as i32 - 4096).collect();
            let mut temp = vec![0; COEFFS * 2];
            let (mut ours, mut lines) = (picture.clone(), picture);
            dwt_level(&mut ours, &mut temp, n);
            line_level(&mut lines, &mut temp, n);
            assert_eq!(ours, lines, "a level of {n}");
        }
    }

    #[test]
    fn the_wavelet_inverts() {
        for n in [64usize, 33, 17] {
            let x: Vec<i32> = (0..n).map(|i| ((i * 37 + 11) % 97) as i32 * 13 - 600).collect();
            let (mut low, mut high) = (vec![0; band_l(n)], vec![0; band_h(n)]);
            dwt_line(&x, &mut low, &mut high);
            let mut back = vec![0; n];
            idwt_line(&low, &high, &mut back);
            for (i, (a, b)) in x.iter().zip(&back).enumerate() {
                assert!((a - b).abs() <= 1, "line of {n}, sample {i}: {a} back as {b}");
            }
        }
    }

    #[test]
    fn a_tile_comes_back_at_full() {
        let px = tile(|x, y| [(x * 4) as u8, (y * 4) as u8, ((x ^ y) * 4) as u8]);
        let mut enc = Encoder::new(64, tables());
        enc.first((0, 0), &px, TILE * 4, TILE, TILE, 2);
        let shown = enc.shown((0, 0)).unwrap();
        assert!(psnr(&px, &shown) > 40.0, "{}", psnr(&px, &shown));
    }

    #[test]
    fn upgrades_reach_what_a_full_first_pass_shows() {
        let px = tile(|x, y| if (x / 8 + y / 8) % 2 == 0 { [250, 250, 250] } else { [20, 30, 200] });
        let mut full = Encoder::new(64, tables());
        full.first((0, 0), &px, TILE * 4, TILE, TILE, 2);
        let mut staged = Encoder::new(64, tables());
        let coarse = staged.first((0, 0), &px, TILE * 4, TILE, TILE, 0);
        let p0 = psnr(&px, &staged.shown((0, 0)).unwrap());
        let up1 = staged.upgrade((0, 0), 1).unwrap();
        let p1 = psnr(&px, &staged.shown((0, 0)).unwrap());
        let up2 = staged.upgrade((0, 0), 2).unwrap();
        assert!(staged.upgrade((0, 0), 2).is_none());
        assert_eq!(staged.shown((0, 0)), full.shown((0, 0)));
        assert!(p0 < p1, "{p0} {p1}");
        assert!(coarse.block.len() < full.tiles[&(0, 0)].target.len(), "a coarse pass is small: {}", coarse.block.len());
        assert!(up1.len() + up2.len() > 0);
        // The bits add up: what the stages hold is what full holds.
        assert_eq!(staged.held((0, 0)).unwrap().held, full.held((0, 0)).unwrap().held);
    }

    #[test]
    fn a_changed_tile_goes_as_a_difference() {
        // A textured tile, whose coefficients are mostly nonzero.
        let px = tile(|x, y| {
            let h = ((x * 2654435761usize) ^ (y * 40503)) % 251;
            [h as u8, (h * 7 % 256) as u8, (x * 4) as u8]
        });
        let mut enc = Encoder::new(64, tables());
        enc.first((0, 0), &px, TILE * 4, TILE, TILE, 2);
        let mut changed = px.clone();
        for p in changed[..64 * 4 * 4].chunks_mut(4) {
            p[1] = p[1].wrapping_add(40);
        }
        let pass = enc.first((0, 0), &changed, TILE * 4, TILE, TILE, 0);
        assert!(pass.diff);
        enc.upgrade((0, 0), 2).unwrap();
        let mut fresh = Encoder::new(64, tables());
        fresh.first((0, 0), &changed, TILE * 4, TILE, TILE, 2);
        assert_eq!(enc.held((0, 0)).unwrap().held, fresh.held((0, 0)).unwrap().held);
    }

    /// First passes coded ahead on threads are the ones coded in order, and
    /// leave the decoder holding the same; one whose tile is touched in
    /// between is coded again.
    #[test]
    fn passes_coded_ahead_are_the_ones_in_order() {
        let tiles = [
            tile(|x, y| {
                let h = ((x * 2654435761usize) ^ (y * 40503)) % 251;
                [h as u8, (h * 7 % 256) as u8, (x * 4) as u8]
            }),
            tile(|_, _| [30, 90, 200]),
            tile(|x, y| [(x * 4) as u8, (y * 4) as u8, ((x ^ y) * 4) as u8]),
            tile(|x, y| [((x + y) * 2) as u8, 128, (y * 4) as u8]),
        ];
        let stride = TILE * 4 * tiles.len();
        let mut picture = vec![0u8; stride * TILE];
        for (i, px) in tiles.iter().enumerate() {
            for y in 0..TILE {
                picture[y * stride + i * TILE * 4..][..TILE * 4].copy_from_slice(&px[y * TILE * 4..][..TILE * 4]);
            }
        }
        let (mut ordered, mut ahead) = (Encoder::new(256, tables()), Encoder::new(256, tables()));
        // Held before: the first two tiles, at the full stage, then changed.
        for enc in [&mut ordered, &mut ahead] {
            enc.first((0, 0), &tiles[1], TILE * 4, TILE, TILE, 2);
            enc.first((1, 0), &tiles[2], TILE * 4, TILE, TILE, 0);
        }
        let jobs: Vec<Job> = (0..tiles.len()).map(|i| Job { cell: (i as u16, 0), at: i * TILE * 4, w: TILE, h: TILE }).collect();
        ahead.precode(&jobs, &picture, stride, 1, 1.25, 3);
        // Touched in between: coded again, as in order.
        for enc in [&mut ordered, &mut ahead] {
            enc.upgrade((1, 0), 2);
        }
        for job in &jobs {
            let a = ordered.first_or_full(job.cell, &picture[job.at..], stride, TILE, TILE, 1, 1.25);
            let b = ahead.first_or_full(job.cell, &picture[job.at..], stride, TILE, TILE, 1, 1.25);
            assert_eq!((a.block, a.diff), (b.block, b.diff), "tile {:?}", job.cell);
            let (a, b) = (ordered.held(job.cell).unwrap(), ahead.held(job.cell).unwrap());
            assert_eq!((&a.held, &a.sign, a.shifts, a.full), (&b.held, &b.sign, b.shifts, b.full), "tile {:?}", job.cell);
        }
    }
}

/// A RemoteFX Progressive decoder of the tests' own, written from
/// [MS-RDPEGFX] 3.2.8.1 and [MS-RDPRFX] 3.1.8.1.7 and sharing nothing
/// with the encoder: it reads a stream as [`wlshare_proto::progressive::stream`]
/// makes one, its tables from the region, and keeps every tile's
/// coefficients from one region to the next; the tiles' tests draw with it
/// too.
#[cfg(test)]
pub mod decoder {
    use std::collections::HashMap;

    const N: usize = 4096;
    /// Each band's offset and length in a component, HL1 first, LL3 last.
    const BANDS: [(usize, usize); 10] = [(0, 1023), (1023, 1023), (2046, 961), (3007, 272), (3279, 272), (3551, 256), (3807, 72), (3879, 72), (3951, 64), (4015, 81)];

    /// The ten nibbles of a packed quantizer table, in band order.
    fn nibbles(b: &[u8]) -> [u8; 10] {
        [b[3] >> 4, b[4] & 0xF, b[4] >> 4, b[2] & 0xF, b[2] >> 4, b[3] & 0xF, b[0] >> 4, b[1] & 0xF, b[1] >> 4, b[0] & 0xF]
    }

    struct Bits<'a> {
        data: &'a [u8],
        at: usize,
    }

    impl Bits<'_> {
        fn bit(&mut self) -> u32 {
            let b = self.data.get(self.at / 8).map_or(0, |byte| u32::from(byte >> (7 - self.at % 8)) & 1);
            self.at += 1;
            b
        }
        fn take(&mut self, n: u32) -> u32 {
            (0..n).fold(0, |v, _| (v << 1) | self.bit())
        }
    }

    /// Golomb-Rice with its adaptive parameter.
    fn gr(bits: &mut Bits<'_>, kr: &mut u32, krp: &mut u32) -> u32 {
        let mut vk = 0;
        while bits.bit() == 1 {
            vk += 1;
        }
        let code = (vk << *kr) | bits.take(*kr);
        if vk == 0 {
            *krp = krp.saturating_sub(2);
        } else if vk != 1 {
            *krp = (*krp + vk).min(80);
        }
        *kr = *krp >> 3;
        code
    }

    fn rlgr1(data: &[u8]) -> Vec<i32> {
        let mut bits = Bits { data, at: 0 };
        let mut out = Vec::with_capacity(N);
        let (mut kp, mut kr, mut krp) = (8u32, 1u32, 8u32);
        while out.len() < N && bits.at < data.len() * 8 {
            let k = kp >> 3;
            if k > 0 {
                if bits.bit() == 0 {
                    out.extend(std::iter::repeat_n(0, 1 << k));
                    kp = (kp + 4).min(80);
                    continue;
                }
                let run = bits.take(k) as usize;
                out.extend(std::iter::repeat_n(0, run));
                let negative = bits.bit() == 1;
                let magnitude = gr(&mut bits, &mut kr, &mut krp) as i32 + 1;
                out.push(if negative { -magnitude } else { magnitude });
                kp = kp.saturating_sub(6);
            } else {
                let code = gr(&mut bits, &mut kr, &mut krp);
                out.push(if code % 2 == 1 { -((code as i32 + 1) / 2) } else { code as i32 / 2 });
                kp = if code == 0 { (kp + 3).min(80) } else { kp.saturating_sub(3) };
            }
        }
        out.resize(N, 0);
        out
    }

    /// The simplified run-length code of an upgrade's new coefficients.
    struct Srl<'a> {
        bits: Bits<'a>,
        kp: u32,
        zeros: u32,
        value: bool,
    }

    impl Srl<'_> {
        fn next(&mut self, n: u32) -> i32 {
            loop {
                if self.zeros > 0 {
                    self.zeros -= 1;
                    return 0;
                }
                if self.value {
                    self.value = false;
                    let negative = self.bits.bit() == 1;
                    self.kp = self.kp.saturating_sub(6);
                    let mut magnitude = 1;
                    while magnitude < (1 << n) - 1 && self.bits.bit() == 0 {
                        magnitude += 1;
                    }
                    return if negative { -magnitude } else { magnitude };
                }
                let k = self.kp >> 3;
                if self.bits.bit() == 0 {
                    self.zeros = 1 << k;
                    self.kp = (self.kp + 4).min(80);
                } else {
                    self.zeros = self.bits.take(k);
                    self.value = true;
                }
            }
        }
    }

    struct Tile {
        coeffs: [Vec<i16>; 3],
        sign: [Vec<i8>; 3],
        shifts: [[u8; 10]; 3],
    }

    #[derive(Default)]
    pub struct Decoder {
        tiles: HashMap<(u16, u16), Tile>,
    }

    fn le16(b: &[u8], at: usize) -> usize {
        usize::from(u16::from_le_bytes([b[at], b[at + 1]]))
    }

    impl Decoder {
        /// Every tile of `stream`'s region, as `B, G, R, X`, 64×64.
        pub fn decode(&mut self, stream: &[u8]) -> Vec<((u16, u16), Vec<u8>)> {
            let mut shown = Vec::new();
            let mut at = 0;
            while at < stream.len() {
                let kind = u16::from_le_bytes([stream[at], stream[at + 1]]);
                let len = u32::from_le_bytes(stream[at + 2..at + 6].try_into().unwrap()) as usize;
                if kind == 0xCCC4 {
                    shown.extend(self.region(&stream[at + 6..at + len]));
                }
                at += len;
            }
            assert_eq!(at, stream.len(), "the blocks fill the stream");
            shown
        }

        fn region(&mut self, r: &[u8]) -> Vec<((u16, u16), Vec<u8>)> {
            assert_eq!(r[0], 64);
            let (rects, quants, stages) = (le16(r, 1), usize::from(r[3]), usize::from(r[4]));
            let mut at = 12 + rects * 8;
            let quant: Vec<[u8; 10]> = (0..quants).map(|i| nibbles(&r[at + i * 5..])).collect();
            at += quants * 5;
            let stage: Vec<[[u8; 10]; 3]> = (0..stages).map(|i| [0, 1, 2].map(|c| nibbles(&r[at + i * 16 + 1 + c * 5..]))).collect();
            at += stages * 16;
            let mut shown = Vec::new();
            while at < r.len() {
                let (kind, len) = (u16::from_le_bytes([r[at], r[at + 1]]), u32::from_le_bytes(r[at + 2..at + 6].try_into().unwrap()) as usize);
                let t = &r[at + 6..at + len];
                let q = [quant[usize::from(t[0])], quant[usize::from(t[1])], quant[usize::from(t[2])]];
                let cell = (le16(t, 3) as u16, le16(t, 5) as u16);
                match kind {
                    0xCCC6 => self.first(cell, t, &stage),
                    0xCCC7 => self.upgrade(cell, t, &stage),
                    other => panic!("a tile block of {other:#x}"),
                }
                shown.push((cell, self.picture(cell, q)));
                at += len;
            }
            shown
        }

        fn first(&mut self, cell: (u16, u16), t: &[u8], stage: &[[[u8; 10]; 3]]) {
            let (difference, quality) = (t[7] & 1 == 1, t[8]);
            let shifts = if quality == 0xFF { [[0; 10]; 3] } else { stage[usize::from(quality)] };
            let lens = [le16(t, 9), le16(t, 11), le16(t, 13)];
            let tile = self.tiles.entry(cell).or_insert_with(|| Tile { coeffs: [vec![0; N], vec![0; N], vec![0; N]], sign: [vec![0; N], vec![0; N], vec![0; N]], shifts });
            let mut from = 17;
            for c in 0..3 {
                let mut v = rlgr1(&t[from..from + lens[c]]);
                from += lens[c];
                let (ll, len) = BANDS[9];
                for i in ll + 1..ll + len {
                    v[i] += v[i - 1];
                }
                for (band, &(at, len)) in BANDS.iter().enumerate() {
                    let held = tile.coeffs[c][at..at + len].iter_mut().zip(&mut tile.sign[c][at..at + len]);
                    for ((coeff, sign), &v) in held.zip(&v[at..at + len]) {
                        let value = (v << shifts[c][band]) as i16;
                        *coeff = if difference { coeff.wrapping_add(value) } else { value };
                        *sign = v.signum() as i8;
                    }
                }
            }
            tile.shifts = shifts;
        }

        fn upgrade(&mut self, cell: (u16, u16), t: &[u8], stage: &[[[u8; 10]; 3]]) {
            let quality = t[7];
            let to = if quality == 0xFF { [[0; 10]; 3] } else { stage[usize::from(quality)] };
            let lens: Vec<usize> = (0..6).map(|i| le16(t, 8 + i * 2)).collect();
            let tile = self.tiles.get_mut(&cell).expect("an upgrade of a tile the decoder holds");
            let mut from = 20;
            for c in 0..3 {
                let srl = &t[from..from + lens[c * 2]];
                let raw = &t[from + lens[c * 2]..from + lens[c * 2] + lens[c * 2 + 1]];
                from += lens[c * 2] + lens[c * 2 + 1];
                let (mut srl, mut raw) = (Srl { bits: Bits { data: srl, at: 0 }, kp: 8, zeros: 0, value: false }, Bits { data: raw, at: 0 });
                for (band, &(at, len)) in BANDS.iter().enumerate() {
                    let (was, now) = (tile.shifts[c][band], to[c][band]);
                    let n = u32::from(was - now);
                    if n == 0 {
                        continue;
                    }
                    for i in at..at + len {
                        let add = if band == 9 {
                            (raw.take(n) as i32) << now
                        } else if tile.sign[c][i] == 0 {
                            let v = srl.next(n);
                            tile.sign[c][i] = v.signum() as i8;
                            v << now
                        } else {
                            let v = (raw.take(n) as i32) << now;
                            if tile.sign[c][i] < 0 { -v } else { v }
                        };
                        tile.coeffs[c][i] = tile.coeffs[c][i].wrapping_add(add as i16);
                    }
                }
            }
            tile.shifts = to;
        }

        /// The tile's picture: dequantized, the inverse wavelet, YCbCr to
        /// `B, G, R, X`.
        fn picture(&self, cell: (u16, u16), quant: [[u8; 10]; 3]) -> Vec<u8> {
            let tile = &self.tiles[&cell];
            let planes: Vec<Vec<i32>> = (0..3)
                .map(|c| {
                    let mut v = vec![0i32; N];
                    for (band, &(at, len)) in BANDS.iter().enumerate() {
                        for (v, &coeff) in v[at..at + len].iter_mut().zip(&tile.coeffs[c][at..at + len]) {
                            *v = i32::from((i32::from(coeff) << (quant[c][band] - 1)) as i16);
                        }
                    }
                    for (n, at) in [(17, 3807), (33, 3007), (64, 0)] {
                        inverse_level(&mut v[at..], n);
                    }
                    v
                })
                .collect();
            let mut px = vec![0u8; N * 4];
            for i in 0..N {
                let y = i64::from(planes[0][i]) + 4096;
                let (cb, cr) = (i64::from(planes[1][i]), i64::from(planes[2][i]));
                let to = |v: i64| (v >> 21).clamp(0, 255) as u8;
                px[i * 4] = to((y << 16) + cb * 115992);
                px[i * 4 + 1] = to((y << 16) - cb * 22527 - cr * 46819);
                px[i * 4 + 2] = to((y << 16) + cr * 91916);
            }
            px
        }
    }

    fn half(v: i32) -> i32 {
        if v < 0 { -(-v / 2) } else { v / 2 }
    }

    /// One line back from its low and high halves, the reduce-extrapolate
    /// wavelet's inverse.
    fn inverse_line(low: &[i32], high: &[i32], n: usize) -> Vec<i32> {
        let nh = high.len();
        let mut x = vec![0; n];
        for k in 0..=nh {
            let before = if k > 0 { high[k - 1] } else { 0 };
            x[2 * k] = match (k, k == nh) {
                (0, _) => low[0] - high[0],
                (_, false) => low[k] - half(before + high[k]),
                (_, true) if n.is_multiple_of(2) => low[k] - half(before),
                _ => low[k] - before,
            };
        }
        for k in 0..nh {
            x[2 * k + 1] = 2 * high[k] + half(x[2 * k] + x[2 * k + 2]);
        }
        if n.is_multiple_of(2) {
            x[n - 1] = half(x[n - 2] + low[nh + 1]);
        }
        x
    }

    /// One level back over the bands at the front of `v`: HL, LH, HH, LL.
    fn inverse_level(v: &mut [i32], n: usize) {
        let nl = n / 2 + 1;
        let nh = n - nl;
        let (hl, lh, hh, ll) = (0, nl * nh, 2 * nl * nh, 2 * nl * nh + nh * nh);
        let mut rows = vec![0; n * n];
        for r in 0..n {
            let (low, high) = if r < nl { (&v[ll + r * nl..][..nl], &v[hl + r * nh..][..nh]) } else { (&v[lh + (r - nl) * nl..][..nl], &v[hh + (r - nl) * nh..][..nh]) };
            rows[r * n..(r + 1) * n].copy_from_slice(&inverse_line(low, high, n));
        }
        for x in 0..n {
            let low: Vec<i32> = (0..nl).map(|r| rows[r * n + x]).collect();
            let high: Vec<i32> = (nl..n).map(|r| rows[r * n + x]).collect();
            for (y, value) in inverse_line(&low, &high, n).into_iter().enumerate() {
                v[y * n + x] = value;
            }
        }
    }
}
