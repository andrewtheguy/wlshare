//! How often a still the tiles encoding sends is a tile the client was shown
//! before: what a client-side tile cache, as RDP's EGFX keeps one, would save
//! on a session.
//!
//! ```text
//! tile-recurrence [--slots N,N,...] [--keep] CAPTURE...
//! ```
//!
//! Each capture is played as the tiles encoder reads it, with Motion's rule
//! for what moves: a 64×64 tile that changed in 4 of the last 8 tenths of a
//! second is moving, a 4-connected group of at least 5 moving tiles is a
//! stream's while it moves and each of its tiles is owed a still when it
//! stops, and every other changed tile is a still at once. Each still's
//! content is hashed and looked for in a cache of every still the client was
//! sent, kept by least recent use at each of `--slots` sizes and unbounded. A
//! hit is `onscreen` where the same content stands in another tile of the
//! picture the client holds — what a COPY could draw — and `offscreen` where
//! it does not, which only a cache serves. Flat tiles, of one colour, are
//! never cached, as the encoder stores none, and are counted apart among the
//! stills, since they are cheap anyway. Bytes are each still coded alone as a
//! fast PNG, a held list's, so they overstate the per-image overhead of a run
//! of tiles coded as one.
//! A whole picture — the first frame, or a keyframe asked — is counted as
//! `whole` and not as stills, and starts the cache over with its tiles, as
//! the encoder forgot its own before it kept it. With `--keep` the cache
//! outlives it, as the encoder's does now, and each cache counts the tiles
//! of every whole picture after the first, flat ones apart, that it holds:
//! `restart_tiles` of them in all, each a draw of a slot and not a still,
//! and their bytes as a fast PNG, of `restart_png`, every tile of those
//! pictures so coded.
//!
//! Each still is also counted by why it is sent: `fresh`, a tile that had not
//! changed in the 0.8 s before; `warming`, one that had, but in fewer than 4
//! of the 8 tenths; `small`, a moving tile in a group too small for a stream;
//! and `stopped`, one a stream carried that stopped. `warming` and `small`
//! are motion the stills carry, what a codec that sends a tile's difference
//! from what the client holds, as RemoteFX Progressive's sub-band diffing
//! does, would code instead. For every still the client holds exact pixels
//! under — not one a stream drew — `delta` counts the tile's pixels, those
//! that changed, and the change as a PNG of the per-channel difference modulo
//! 256 beside the still's own PNG: what a held list's lossless difference
//! would cost.
//!
//! One JSON object per capture on stdout.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use wlcap::Reader;

// The one item of the daemon's RFB module the capture file names.
mod rfb {
    include!("../src/rfb/rect.rs");
}
#[allow(dead_code)]
#[path = "../src/wlcap.rs"]
mod wlcap;

type Error = Box<dyn std::error::Error>;

const TILE: usize = 64;
const CHURN_SLOT_MS: u128 = 100;
const CHURN_WINDOW: u64 = 8;
const CHURN_MOVING: u32 = 4;
/// The fewest moving tiles a 4-connected group needs to be a stream's.
const MIN_STREAM_CELLS: usize = 5;

fn main() {
    if let Err(e) = run() {
        eprintln!("tile-recurrence: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let mut slots = vec![1024usize, 4096, 16384];
    let mut keep = false;
    let mut captures = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--slots" => slots = args.next().ok_or("--slots needs a value")?.split(',').map(|s| s.parse()).collect::<Result<_, _>>()?,
            "--keep" => keep = true,
            _ => captures.push(arg),
        }
    }
    if captures.is_empty() {
        return Err("usage: tile-recurrence [--slots N,N,...] [--keep] CAPTURE...".into());
    }
    for capture in captures {
        let stats = play(Path::new(&capture), &slots, keep)?;
        println!("{}", stats.json(&capture, &slots));
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

/// A tile's content, hashed over B, G and R, and whether it is one colour.
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

fn tile_rgb(pixels: &[u8], width: usize, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(w * h * 3);
    for row in y..y + h {
        let start = (row * width + x) * 4;
        for px in pixels[start..start + w * 4].as_chunks::<4>().0 {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    rgb
}

/// The still's bytes as a fast PNG, palette where it has at most 256 colours,
/// as the encoder codes one.
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

    /// In how many of the last 8 tenths `key` changed, as `moving` left it.
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

/// The moving tiles a stream carries: those in a 4-connected group of at
/// least [`MIN_STREAM_CELLS`]; a smaller group is left to the stills.
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

/// A cache of tile contents by least recent use, at most `slots` of them.
struct Lru {
    slots: usize,
    stamp: u64,
    by_hash: HashMap<u64, u64>,
    by_use: BTreeSet<(u64, u64)>,
}

impl Lru {
    fn new(slots: usize) -> Self {
        Self { slots, stamp: 0, by_hash: HashMap::new(), by_use: BTreeSet::new() }
    }

    fn clear(&mut self) {
        self.by_hash.clear();
        self.by_use.clear();
    }

    /// Whether `hash` was held; held after either way, as the most recent.
    fn touch(&mut self, hash: u64) -> bool {
        self.stamp += 1;
        if let Some(old) = self.by_hash.insert(hash, self.stamp) {
            self.by_use.remove(&(old, hash));
            self.by_use.insert((self.stamp, hash));
            return true;
        }
        self.by_use.insert((self.stamp, hash));
        if self.by_hash.len() > self.slots
            && let Some(&oldest) = self.by_use.iter().next()
        {
            self.by_use.remove(&oldest);
            self.by_hash.remove(&oldest.1);
        }
        false
    }
}

#[derive(Default, Clone, Copy)]
struct Count {
    tiles: u64,
    flat: u64,
    png: u64,
    /// Bytes of the tiles that are not flat.
    png_nonflat: u64,
}

impl Count {
    fn add(&mut self, flat: bool, png: usize) {
        self.tiles += 1;
        self.png += png as u64;
        if flat {
            self.flat += 1;
        } else {
            self.png_nonflat += png as u64;
        }
    }

    fn json(&self) -> String {
        format!(
            "{{\"tiles\":{},\"flat\":{},\"png\":{},\"png_nonflat\":{}}}",
            self.tiles, self.flat, self.png, self.png_nonflat
        )
    }
}

/// Why a still is sent, as the module's doc says.
#[derive(Clone, Copy)]
enum Why {
    Fresh,
    Warming,
    Small,
    Stopped,
}

const WHY: [&str; 4] = ["fresh", "warming", "small", "stopped"];

/// Stills against the exact pixels the client held under them.
#[derive(Default, Clone, Copy)]
struct Delta {
    tiles: u64,
    pixels: u64,
    changed: u64,
    png: u64,
    png_delta: u64,
}

impl Delta {
    fn json(&self) -> String {
        format!(
            "{{\"tiles\":{},\"pixels\":{},\"changed\":{},\"png\":{},\"png_delta\":{}}}",
            self.tiles, self.pixels, self.changed, self.png, self.png_delta
        )
    }
}

#[derive(Default)]
struct Stats {
    frames: u64,
    seconds: f64,
    whole_tiles: u64,
    /// The tiles of every whole picture after the first that are not flat.
    restart_tiles: u64,
    /// Every tile of those pictures as a fast PNG, with `--keep`.
    restart_png: u64,
    stills: Count,
    /// The stills by [`Why`], and of each those against exact pixels.
    why: [(Count, Delta); 4],
    /// Per cache size, then unbounded last: hits on screen, hits off screen.
    hits: Vec<(Count, Count)>,
    /// Per cache size, then unbounded last: of `restart_tiles`, those it
    /// held, and of `restart_png`, theirs.
    restart_hits: Vec<(u64, u64)>,
}

impl Stats {
    fn json(&self, capture: &str, slots: &[usize]) -> String {
        let mut caches = Vec::new();
        for (i, (on, off)) in self.hits.iter().enumerate() {
            let name = slots.get(i).map_or("unbounded".to_owned(), |n| n.to_string());
            let (tiles, png) = self.restart_hits[i];
            caches.push(format!("\"{name}\":{{\"onscreen\":{},\"offscreen\":{},\"restart\":{{\"tiles\":{tiles},\"png\":{png}}}}}", on.json(), off.json()));
        }
        let why: Vec<String> =
            WHY.iter().zip(&self.why).map(|(name, (count, delta))| format!("\"{name}\":{{\"stills\":{},\"delta\":{}}}", count.json(), delta.json())).collect();
        format!(
            "{{\"capture\":\"{}\",\"frames\":{},\"seconds\":{:.1},\"whole_tiles\":{},\"restart_tiles\":{},\"restart_png\":{},\"stills\":{},\"why\":{{{}}},\"cache\":{{{}}}}}",
            capture, self.frames, self.seconds, self.whole_tiles, self.restart_tiles, self.restart_png, self.stills.json(), why.join(","), caches.join(",")
        )
    }
}

fn play(path: &Path, slots: &[usize], keep: bool) -> Result<Stats, Error> {
    let mut reader = open(path)?;
    let mut stats = Stats { hits: vec![(Count::default(), Count::default()); slots.len() + 1], restart_hits: vec![(0, 0); slots.len() + 1], ..Stats::default() };
    let mut caches: Vec<Lru> = slots.iter().map(|&n| Lru::new(n)).chain(std::iter::once(Lru::new(usize::MAX))).collect();
    // What the client holds in each tile, exact: `None` under a stream.
    let mut held: HashMap<(u16, u16), Option<u64>> = HashMap::new();
    // Its pixels, where they are exact.
    let mut held_rgb: HashMap<(u16, u16), Vec<u8>> = HashMap::new();
    let mut refcount: HashMap<u64, u32> = HashMap::new();
    let mut streamed: HashSet<(u16, u16)> = HashSet::new();
    let mut churn = Churn::default();
    // Each distinct content's bytes, coded once.
    let mut coded: HashMap<u64, usize> = HashMap::new();
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
            held_rgb.clear();
            refcount.clear();
            streamed.clear();
            churn = Churn::default();
            if !keep {
                for cache in &mut caches {
                    cache.clear();
                }
            }
            let restart = stats.frames > 1;
            for ty in 0..tiles_y as u16 {
                for tx in 0..tiles_x as u16 {
                    let (x, y, w, h) = geometry(tx, ty);
                    let (hash, flat) = tile_hash(pixels, width, x, y, w, h);
                    held.insert((tx, ty), Some(hash));
                    let rgb = tile_rgb(pixels, width, x, y, w, h);
                    let png = if keep && restart { png_bytes(w, h, &rgb)? as u64 } else { 0 };
                    stats.restart_png += png;
                    held_rgb.insert((tx, ty), rgb);
                    *refcount.entry(hash).or_default() += 1;
                    if !flat {
                        stats.restart_tiles += u64::from(restart);
                        for (cache, (tiles, bytes)) in caches.iter_mut().zip(&mut stats.restart_hits) {
                            if cache.touch(hash) && restart {
                                *tiles += 1;
                                *bytes += png;
                            }
                        }
                    }
                    stats.whole_tiles += 1;
                }
            }
            continue;
        }

        // The tiles the damage touches, each hashed once.
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

        let mut stills: Vec<((u16, u16), u64, bool, Why)> = Vec::new();
        for (key, hash, flat) in changed {
            if moving.contains(&key) {
                if let Some(Some(old)) = held.insert(key, None)
                    && let Some(n) = refcount.get_mut(&old)
                {
                    *n -= 1;
                    if *n == 0 {
                        refcount.remove(&old);
                    }
                }
                held_rgb.remove(&key);
                streamed.insert(key);
            } else {
                streamed.remove(&key);
                let why = if all_moving.contains(&key) {
                    Why::Small
                } else if churn.changes(key) > 1 {
                    Why::Warming
                } else {
                    Why::Fresh
                };
                stills.push((key, hash, flat, why));
            }
        }
        // Tiles a stream carried that stopped and did not change this frame
        // are owed a still of what they show.
        let stopped: Vec<(u16, u16)> = streamed.iter().filter(|k| !moving.contains(k)).copied().collect();
        for key in stopped {
            streamed.remove(&key);
            let (x, y, w, h) = geometry(key.0, key.1);
            let (hash, flat) = tile_hash(pixels, width, x, y, w, h);
            stills.push((key, hash, flat, Why::Stopped));
        }

        for (key, hash, flat, why) in stills {
            let (x, y, w, h) = geometry(key.0, key.1);
            let rgb = tile_rgb(pixels, width, x, y, w, h);
            let png = match coded.get(&hash) {
                Some(&b) => b,
                None => {
                    let b = png_bytes(w, h, &rgb)?;
                    coded.insert(hash, b);
                    b
                }
            };
            stats.stills.add(flat, png);
            let (count, delta) = &mut stats.why[why as usize];
            count.add(flat, png);
            if let Some(old) = held_rgb.get(&key) {
                let diff: Vec<u8> = rgb.iter().zip(old).map(|(new, old)| new.wrapping_sub(*old)).collect();
                delta.tiles += 1;
                delta.pixels += (w * h) as u64;
                delta.changed += diff.as_chunks::<3>().0.iter().filter(|px| **px != [0, 0, 0]).count() as u64;
                delta.png += png as u64;
                delta.png_delta += png_bytes(w, h, &diff)? as u64;
            }
            held_rgb.insert(key, rgb);
            let onscreen = refcount.get(&hash).is_some_and(|&n| n > 0);
            if !flat {
                for (cache, (on, off)) in caches.iter_mut().zip(&mut stats.hits) {
                    if cache.touch(hash) {
                        if onscreen { on.add(flat, png) } else { off.add(flat, png) }
                    }
                }
            }
            if let Some(Some(old)) = held.insert(key, Some(hash))
                && let Some(n) = refcount.get_mut(&old)
            {
                *n -= 1;
                if *n == 0 {
                    refcount.remove(&old);
                }
            }
            *refcount.entry(hash).or_default() += 1;
        }
    }
    stats.seconds = last_at.as_secs_f64();
    Ok(stats)
}
