//! What the stills and the motion of a session would cost coded as RemoteFX
//! Progressive tiles ([`progressive`]), beside what they cost as the tiles
//! encoding's PNGs: what a first pass that holds bits back costs, what the
//! refinement to full costs after it, and what a changed tile costs as a
//! difference from what the client holds.
//!
//! ```text
//! progressive-stills [--quant HEX] [--stages HEX,...] CAPTURE...
//! ```
//!
//! Each capture is played as `tile-recurrence` plays it, with Motion's rule
//! for what moves: a 64×64 tile that changed in 4 of the last 8 tenths of a
//! second is moving, a 4-connected group of at least 5 moving tiles is a
//! stream's, and every other changed tile is a still, counted by why it is
//! sent (`fresh`, `warming`, `small`, `stopped`). Every still is coded alone
//! as a fast PNG, as a held list's.
//!
//! Then the session is played through three progressive encoders, one per
//! first stage — the host's stage of quality 50, of 75, and full — each
//! sending every still as a first pass at its stage, a difference from what
//! the client holds where that has more zeros, and then the upgrades to
//! full; and every tile a stream would carry as a first pass at its stage
//! each frame it changes, upgraded to full once it stops. The tables are a
//! Windows host's, as `zdesk`'s link probe recorded them, unless `--quant`
//! (five bytes) and `--stages` (fifteen a stage, coarsest first, full last)
//! name others.
//!
//! Each still's and each moving tile's picture is compared with the capture's
//! after the first pass and after the last: the PSNR over B, G and R. The
//! time is each coder's own, this machine's.
//!
//! One JSON object per capture on stdout.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wlshare_vp9::capture::Reader;

#[allow(dead_code)]
#[path = "../src/rfb/progressive.rs"]
mod progressive;

use progressive::{Encoder, Quant, Stage, Tables};

type Error = Box<dyn std::error::Error>;

const TILE: usize = 64;
const CHURN_SLOT_MS: u128 = 100;
const CHURN_WINDOW: u64 = 8;
const CHURN_MOVING: u32 = 4;
const MIN_STREAM_CELLS: usize = 5;

/// The tables `windows-ent-sandbox` sends, recorded on 2026-10-27 over a
/// LAN and behind a page on 1 Mbit/s alike: one quantization table for the
/// three components, and the stages of quality 50 and 75 before full.
const HOST_QUANT: &str = "66768899a9";
const HOST_STAGES: [&str; 2] = ["111122121111112212111111221201", "101111010010111201001011121100"];

fn main() {
    if let Err(e) = run() {
        eprintln!("progressive-stills: {e}");
        std::process::exit(1);
    }
}

fn hex(s: &str) -> Result<Vec<u8>, Error> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("{s}: {e}").into())).collect()
}

fn run() -> Result<(), Error> {
    let mut quant = HOST_QUANT.to_owned();
    let mut stages: Vec<String> = HOST_STAGES.iter().map(|s| (*s).to_owned()).collect();
    let mut captures = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--quant" => quant = args.next().ok_or("--quant needs five bytes")?,
            "--stages" => stages = args.next().ok_or("--stages needs tables")?.split(',').map(str::to_owned).collect(),
            _ => captures.push(arg),
        }
    }
    if captures.is_empty() {
        return Err("usage: progressive-stills [--quant HEX] [--stages HEX,...] CAPTURE...".into());
    }
    let quant = Quant::from_bytes(hex(&quant)?.as_slice().try_into().map_err(|_| "--quant takes five bytes")?);
    let mut table = Vec::new();
    for (i, s) in stages.iter().enumerate() {
        let b = hex(s)?;
        if b.len() != 15 {
            return Err("a stage is fifteen bytes".into());
        }
        // Labelled as the host labels its two, else spaced out to full.
        let quality = if stages.len() == 2 { [50, 75][i] } else { (100 * (i + 1) / (stages.len() + 1)) as u8 };
        let mut with_quality = vec![quality];
        with_quality.extend_from_slice(&b);
        table.push(Stage::from_bytes(&with_quality));
    }
    table.push(Stage::FULL);
    let tables = Tables { quant: [quant; 3], stages: table };
    for capture in captures {
        let stats = play(Path::new(&capture), &tables)?;
        println!("{}", stats.json(&capture, &tables));
    }
    Ok(())
}

fn open(path: &Path) -> Result<Reader<Box<dyn Read>>, Error> {
    let input: Box<dyn Read> = if path.extension().is_some_and(|e| e == "zst") {
        let file = File::open(path)?;
        let child = Command::new("zstd").arg("-dc").stdin(file).stdout(Stdio::piped()).spawn().map_err(|e| format!("running zstd: {e}"))?;
        Box::new(child.stdout.ok_or("zstd has no stdout")?)
    } else {
        Box::new(File::open(path)?)
    };
    Ok(Reader::new(Box::new(BufReader::with_capacity(1 << 20, input)) as Box<dyn Read>)?)
}

fn tile_hash(pixels: &[u8], width: usize, x: usize, y: usize, w: usize, h: usize) -> (u64, bool) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut first = None;
    let mut flat = true;
    for row in y..y + h {
        let start = (row * width + x) * 4;
        for px in pixels[start..start + w * 4].as_chunks::<4>().0 {
            let word = u32::from_le_bytes(*px) & 0x00ff_ffff;
            word.hash(&mut hasher);
            match first {
                None => first = Some(word),
                Some(f) if f != word => flat = false,
                _ => {}
            }
        }
    }
    (hasher.finish(), flat)
}

fn crop_rgb(pixels: &[u8], width: usize, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(w * h * 3);
    for row in y..y + h {
        let start = (row * width + x) * 4;
        for px in pixels[start..start + w * 4].as_chunks::<4>().0 {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    rgb
}

/// The squared error over B, G and R between the capture's `w`×`h` at
/// `(x, y)` and a decoded tile of `tw` pixels a row, `B, G, R, X`.
#[allow(clippy::too_many_arguments)]
fn tile_se(pixels: &[u8], width: usize, x: usize, y: usize, w: usize, h: usize, shown: &[u8], tw: usize) -> f64 {
    let mut se = 0f64;
    for row in 0..h {
        let src = &pixels[((y + row) * width + x) * 4..][..w * 4];
        let dst = &shown[row * tw * 4..][..w * 4];
        for (a, b) in src.chunks(4).zip(dst.chunks(4)) {
            for c in 0..3 {
                let d = f64::from(a[c]) - f64::from(b[c]);
                se += d * d;
            }
        }
    }
    se
}

fn png_bytes(w: usize, h: usize, rgb: &[u8]) -> Result<usize, Error> {
    let mut colours: Vec<[u8; 3]> = Vec::new();
    let mut indices = Vec::with_capacity(w * h);
    let mut indexed = true;
    for px in rgb.as_chunks::<3>().0 {
        match colours.iter().position(|c| c == px) {
            Some(i) => indices.push(i as u8),
            None if colours.len() < 256 => {
                colours.push(*px);
                indices.push((colours.len() - 1) as u8);
            }
            None => {
                indexed = false;
                break;
            }
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w as u32, h as u32);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    if indexed {
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_palette(colours.concat());
        encoder.set_filter(png::Filter::NoFilter);
    } else {
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_filter(png::Filter::Adaptive);
    }
    let mut writer = encoder.write_header()?;
    if indexed { writer.write_image_data(&indices)? } else { writer.write_image_data(rgb)? }
    writer.finish()?;
    Ok(out.len())
}

#[derive(Clone, Copy)]
struct ChurnCell {
    history: u8,
    last_slot: u64,
}

#[derive(Default)]
struct Churn {
    origin: Option<Duration>,
    churn: HashMap<(u16, u16), ChurnCell>,
}

impl Churn {
    fn slot(origin: Duration, now: Duration) -> u64 {
        (now.saturating_sub(origin).as_millis() / CHURN_SLOT_MS) as u64
    }

    fn observe(&mut self, key: (u16, u16), now: Duration) {
        let origin = *self.origin.get_or_insert(now);
        let slot = Self::slot(origin, now);
        let cell = self.churn.entry(key).or_insert(ChurnCell { history: 0, last_slot: slot });
        let elapsed = slot.saturating_sub(cell.last_slot);
        cell.history = if elapsed >= CHURN_WINDOW { 0 } else { cell.history << elapsed };
        cell.history |= 1;
        cell.last_slot = slot;
    }

    fn changes(&self, key: (u16, u16)) -> u32 {
        self.churn.get(&key).map_or(0, |c| c.history.count_ones())
    }

    fn moving(&mut self, now: Duration) -> HashSet<(u16, u16)> {
        let Some(origin) = self.origin else { return HashSet::new() };
        let slot = Self::slot(origin, now);
        let mut moving = HashSet::new();
        self.churn.retain(|key, cell| {
            let elapsed = slot.saturating_sub(cell.last_slot);
            cell.history = if elapsed >= CHURN_WINDOW { 0 } else { cell.history << elapsed };
            cell.last_slot = slot;
            if cell.history.count_ones() >= CHURN_MOVING {
                moving.insert(*key);
            }
            cell.history != 0
        });
        moving
    }
}

fn carried(moving: &HashSet<(u16, u16)>) -> HashSet<(u16, u16)> {
    let mut out = HashSet::new();
    let mut seen = HashSet::new();
    for &start in moving {
        if !seen.insert(start) {
            continue;
        }
        let mut group = vec![start];
        let mut stack = vec![start];
        while let Some((c, r)) = stack.pop() {
            for next in [(c.wrapping_sub(1), r), (c + 1, r), (c, r.wrapping_sub(1)), (c, r + 1)] {
                if moving.contains(&next) && seen.insert(next) {
                    group.push(next);
                    stack.push(next);
                }
            }
        }
        if group.len() >= MIN_STREAM_CELLS {
            out.extend(group);
        }
    }
    out
}

#[derive(Clone, Copy)]
enum Why {
    Fresh,
    Warming,
    Small,
    Stopped,
}

const WHY: [&str; 4] = ["fresh", "warming", "small", "stopped"];

/// A picture's error summed over tiles, for one PSNR over all of them.
#[derive(Default, Clone, Copy)]
struct Fidelity {
    se: f64,
    samples: f64,
}

impl Fidelity {
    fn add(&mut self, se: f64, w: usize, h: usize) {
        self.se += se;
        self.samples += (w * h * 3) as f64;
    }

    fn psnr(&self) -> f64 {
        if self.se == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 * self.samples / self.se).log10() }
    }
}

/// The stills' bytes as a held list's PNGs.
#[derive(Default, Clone, Copy)]
struct Classic {
    tiles: u64,
    flat: u64,
    png: u64,
}

impl Classic {
    fn json(&self) -> String {
        format!("{{\"tiles\":{},\"flat\":{},\"png\":{}}}", self.tiles, self.flat, self.png)
    }
}

/// What one progressive encoder sent of a kind of tile.
#[derive(Default, Clone, Copy)]
struct Sent {
    tiles: u64,
    /// First passes, and of them differences.
    first: u64,
    diffs: u64,
    /// The upgrades to full after them.
    upgrades: u64,
    after_first: Fidelity,
    after_full: Fidelity,
}

impl Sent {
    fn json(&self) -> String {
        format!(
            "{{\"tiles\":{},\"first\":{},\"diffs\":{},\"upgrades\":{},\"psnr_first\":{:.2},\"psnr_full\":{:.2}}}",
            self.tiles,
            self.first,
            self.diffs,
            self.upgrades,
            self.after_first.psnr(),
            self.after_full.psnr()
        )
    }
}

/// One progressive encoder's run: its first stage, the stills by why, the
/// moving tiles' passes, and its time.
struct Progressive {
    stage: usize,
    encoder: Encoder,
    stills: Sent,
    why: [Sent; 4],
    moving: Sent,
    cpu: Duration,
}

impl Progressive {
    fn json(&self, tables: &Tables) -> String {
        let why: Vec<String> = WHY.iter().zip(&self.why).map(|(name, sent)| format!("\"{name}\":{}", sent.json())).collect();
        format!(
            "{{\"stage\":{},\"stills\":{},\"why\":{{{}}},\"moving\":{},\"cpu_ms\":{:.0}}}",
            tables.stages[self.stage].quality,
            self.stills.json(),
            why.join(","),
            self.moving.json(),
            self.cpu.as_secs_f64() * 1e3
        )
    }
}

#[derive(Default)]
struct Stats {
    frames: u64,
    seconds: f64,
    whole_tiles: u64,
    stills: Classic,
    why: [Classic; 4],
    cpu_png: Duration,
    progressive: Vec<Progressive>,
}

impl Stats {
    fn json(&self, capture: &str, tables: &Tables) -> String {
        let why: Vec<String> = WHY.iter().zip(&self.why).map(|(name, c)| format!("\"{name}\":{}", c.json())).collect();
        let progressive: Vec<String> = self.progressive.iter().map(|p| p.json(tables)).collect();
        format!(
            "{{\"capture\":\"{}\",\"frames\":{},\"seconds\":{:.1},\"whole_tiles\":{},\"stills\":{},\"why\":{{{}}},\"cpu_ms\":{{\"png\":{:.0}}},\"progressive\":[{}]}}",
            capture,
            self.frames,
            self.seconds,
            self.whole_tiles,
            self.stills.json(),
            why.join(","),
            self.cpu_png.as_secs_f64() * 1e3,
            progressive.join(",")
        )
    }
}

/// Send a tile through `p` as a first pass at its stage and, unless it is
/// moving, the upgrades to full, counting it under `sent`.
#[allow(clippy::too_many_arguments)]
fn send(p: &mut Progressive, sent_kinds: &mut [&mut Sent], pixels: &[u8], width: usize, cell: (u16, u16), geometry: (usize, usize, usize, usize), refine: bool) {
    let (x, y, w, h) = geometry;
    let last = p.encoder.tables.stages.len() - 1;
    let started = Instant::now();
    let pass = p.encoder.first(cell, &pixels[(y * width + x) * 4..], width * 4, w, h, p.stage);
    let shown = p.encoder.shown(cell).expect("coded");
    let se_first = tile_se(pixels, width, x, y, w, h, &shown, TILE);
    let mut upgrades = 0;
    let mut se_full = se_first;
    if refine && p.stage < last {
        for stage in p.stage + 1..=last {
            upgrades += p.encoder.upgrade(cell, stage).map_or(0, |b| b.len());
        }
        let shown = p.encoder.shown(cell).expect("coded");
        se_full = tile_se(pixels, width, x, y, w, h, &shown, TILE);
    }
    p.cpu += started.elapsed();
    for sent in sent_kinds {
        sent.tiles += 1;
        sent.first += pass.block.len() as u64;
        sent.diffs += u64::from(pass.diff);
        sent.upgrades += upgrades as u64;
        sent.after_first.add(se_first, w, h);
        sent.after_full.add(se_full, w, h);
    }
}

/// Upgrade a tile that stopped moving to full.
fn settle(p: &mut Progressive, pixels: &[u8], width: usize, cell: (u16, u16), geometry: (usize, usize, usize, usize)) {
    let (x, y, w, h) = geometry;
    let last = p.encoder.tables.stages.len() - 1;
    let started = Instant::now();
    let mut upgrades = 0;
    for stage in p.stage + 1..=last {
        upgrades += p.encoder.upgrade(cell, stage).map_or(0, |b| b.len());
    }
    let shown = p.encoder.shown(cell).expect("coded");
    let se = tile_se(pixels, width, x, y, w, h, &shown, TILE);
    p.cpu += started.elapsed();
    let sent = &mut p.why[Why::Stopped as usize];
    sent.tiles += 1;
    sent.upgrades += upgrades as u64;
    sent.after_full.add(se, w, h);
    p.stills.upgrades += upgrades as u64;
}

fn play(path: &Path, tables: &Tables) -> Result<Stats, Error> {
    let mut reader = open(path)?;
    let mut stats = Stats::default();
    // What the client holds in each tile: `None` under a stream.
    let mut held: HashMap<(u16, u16), Option<u64>> = HashMap::new();
    let mut streamed: HashSet<(u16, u16)> = HashSet::new();
    let mut churn = Churn::default();
    let mut size = (0u16, 0u16);
    let mut last_at = Duration::ZERO;

    while let Some(frame) = reader.next()? {
        stats.frames += 1;
        last_at = frame.at;
        let (width, height) = (usize::from(frame.size.0), usize::from(frame.size.1));
        let pixels = frame.pixels;
        let whole = frame.keyframe || frame.size != size || stats.frames == 1;
        size = frame.size;
        let tiles_x = width.div_ceil(TILE);
        let tiles_y = height.div_ceil(TILE);
        let geometry = |tx: u16, ty: u16| {
            let x = usize::from(tx) * TILE;
            let y = usize::from(ty) * TILE;
            (x, y, (width - x).min(TILE), (height - y).min(TILE))
        };

        if whole {
            held.clear();
            streamed.clear();
            churn = Churn::default();
            if stats.progressive.is_empty() || stats.progressive[0].encoder.cols() != tiles_x {
                stats.progressive = (0..tables.stages.len())
                    .map(|stage| Progressive { stage, encoder: Encoder::new(frame.size.0, tables.clone()), stills: Sent::default(), why: [Sent::default(); 4], moving: Sent::default(), cpu: Duration::ZERO })
                    .collect();
            }
            for ty in 0..tiles_y as u16 {
                for tx in 0..tiles_x as u16 {
                    let (x, y, w, h) = geometry(tx, ty);
                    let (hash, _) = tile_hash(pixels, width, x, y, w, h);
                    held.insert((tx, ty), Some(hash));
                    stats.whole_tiles += 1;
                    // The whole picture is sent full, and the client holds it.
                    for p in &mut stats.progressive {
                        p.encoder.forget_tile((tx, ty));
                        let last = p.encoder.tables.stages.len() - 1;
                        p.encoder.first((tx, ty), &pixels[(y * width + x) * 4..], width * 4, w, h, last);
                    }
                }
            }
            continue;
        }

        let mut candidates: HashSet<(u16, u16)> = HashSet::new();
        match frame.changed {
            None => {
                for ty in 0..tiles_y as u16 {
                    for tx in 0..tiles_x as u16 {
                        candidates.insert((tx, ty));
                    }
                }
            }
            Some(rects) => {
                for r in rects {
                    let x0 = usize::from(r.x).min(width);
                    let y0 = usize::from(r.y).min(height);
                    let x1 = (x0 + usize::from(r.width)).min(width);
                    let y1 = (y0 + usize::from(r.height)).min(height);
                    if x0 >= x1 || y0 >= y1 {
                        continue;
                    }
                    for ty in y0 / TILE..(y1 - 1) / TILE + 1 {
                        for tx in x0 / TILE..(x1 - 1) / TILE + 1 {
                            candidates.insert((tx as u16, ty as u16));
                        }
                    }
                }
            }
        }
        let mut changed: Vec<((u16, u16), u64, bool)> = Vec::new();
        for &key in &candidates {
            let (x, y, w, h) = geometry(key.0, key.1);
            let (hash, flat) = tile_hash(pixels, width, x, y, w, h);
            if held.get(&key).copied().flatten() == Some(hash) {
                continue;
            }
            churn.observe(key, frame.at);
            changed.push((key, hash, flat));
        }
        let all_moving = churn.moving(frame.at);
        let moving = carried(&all_moving);

        let mut stills: Vec<((u16, u16), bool, Why)> = Vec::new();
        for (key, hash, flat) in changed {
            if moving.contains(&key) {
                held.insert(key, None);
                streamed.insert(key);
                for p in &mut stats.progressive {
                    let mut moving = std::mem::take(&mut p.moving);
                    send(p, &mut [&mut moving], pixels, width, key, geometry(key.0, key.1), false);
                    p.moving = moving;
                }
            } else {
                streamed.remove(&key);
                let why = if all_moving.contains(&key) {
                    Why::Small
                } else if churn.changes(key) > 1 {
                    Why::Warming
                } else {
                    Why::Fresh
                };
                stills.push((key, flat, why));
                held.insert(key, Some(hash));
            }
        }
        // Tiles a stream carried that stopped and did not change this frame
        // are owed the rest of what they show.
        let stopped: Vec<(u16, u16)> = streamed.iter().filter(|k| !moving.contains(k)).copied().collect();
        for key in stopped {
            streamed.remove(&key);
            let (x, y, w, h) = geometry(key.0, key.1);
            let (hash, flat) = tile_hash(pixels, width, x, y, w, h);
            held.insert(key, Some(hash));
            stills.push((key, flat, Why::Stopped));
        }

        stills.sort_unstable_by_key(|((tx, ty), _, _)| (*ty, *tx));
        for &(key, flat, why) in &stills {
            let (x, y, w, h) = geometry(key.0, key.1);
            let rgb = crop_rgb(pixels, width, x, y, w, h);
            let started = Instant::now();
            let png = png_bytes(w, h, &rgb)?;
            stats.cpu_png += started.elapsed();
            for c in [&mut stats.stills, &mut stats.why[why as usize]] {
                c.tiles += 1;
                c.flat += u64::from(flat);
                c.png += png as u64;
            }
            for p in &mut stats.progressive {
                if matches!(why, Why::Stopped) {
                    settle(p, pixels, width, key, (x, y, w, h));
                    continue;
                }
                let (mut all, mut this) = (std::mem::take(&mut p.stills), std::mem::take(&mut p.why[why as usize]));
                send(p, &mut [&mut all, &mut this], pixels, width, key, (x, y, w, h), true);
                p.stills = all;
                p.why[why as usize] = this;
            }
        }
    }
    stats.seconds = last_at.as_secs_f64();
    Ok(stats)
}
