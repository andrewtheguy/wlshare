//! The fast-LAN picture planned beside `WLSV` (docs/roadmap.md), measured
//! against it on captures ([`wlshare_rfb::capture`]): every frame coded both
//! ways, and what each cost the encoder and the link.
//!
//! ```text
//! motion-bench [--threads N] [--quality Q] [--wlsv-chroma 444|420]
//!              [--region-chroma 420|444] [--wlsv-speed usual|fastest]
//!              [--region-speed fastest|usual] [--first-sight png|video]
//!              [--bands changed|whole] [--cleanups N] [--csv FILE] CAPTURE...
//! ```
//!
//! A capture ending in `.zst` is read through `zstd -dc`, which must then be
//! on the path.
//!
//! **`WLSV`** is coded as `vp9cap streams` codes it: one stream per size, each
//! frame its rectangles at its dial, a keyframe where one was asked. A stream
//! the walk left coarse at the end is settled once at the capture's highest
//! dial, as a session settles a quiet desktop.
//!
//! **Tiles** is remotex's Motion (its `src/motion.rs` and `src/regions.rs`
//! before `a312b711`), by its rules: 64-pixel tiles from (0,0); a tile moving
//! when it changed in 4 of the last 8 tenths of a second; moving tiles joined
//! 4-connected, a component under 5 tiles left to PNG, boxes merged unless the
//! merge would cover more than twice the moving tiles, at most 4 streams;
//! geometry retuned at most every 500 ms, a region that shrinks keeping its
//! stream, one that grows rebuilt, one with no motion for a second ended. A
//! report of damage is cut into 64-row bands, and from one a stream splits
//! the runs of its changed tiles beside it are sent as PNG. Every tile a
//! stream carried is owed an exact PNG once it has been still for 500 ms, sent
//! from a 250 ms tick, `--cleanups` (40) tiles a tick. Where it is the planned
//! picture's rather than Motion's, each measured on the 1080p and 4K captures
//! against `WLSV` at 4:2:0:
//!
//! - a region's stream is told the damage inside it, as `WLSV` is, rather than
//!   coded whole every frame;
//! - its chroma is `--region-chroma`, 4:2:0 by default, and its speed
//!   `--region-speed`, libvpx's 9 (`fastest`) by default, since what a stream
//!   leaves is settled exact: speed 9 took the tiles from 0.73–0.98 of
//!   `WLSV`'s CPU on motion to 0.44–0.72, the picture while it ran within
//!   half a dB of speed 7's, where `--region-speed usual` is libvpx's 7, as
//!   `--wlsv-speed` is `WLSV`'s (`usual` by default);
//! - a picture that starts over — a size's first frame, a keyframe asked — is
//!   sent as PNG at once, in its bands, where Motion streamed it first and
//!   sent every tile as PNG after: on a link with room the stream is work for
//!   nothing, and it took a still desktop's first picture from 1.5–1.7 of
//!   `WLSV`'s CPU to 0.12–0.13 (`--first-sight video` is Motion's);
//! - a band no stream touches is sent as the runs of its changed tiles, as a
//!   band a stream splits is, rather than whole (`--bands whole` is Motion's):
//!   3–15% fewer bytes where the damage is loose, a terminal's 6–8% more for
//!   its small damage cut into more PNGs, the CPU the same within 5%;
//! - a PNG is the gateway's tile coder: a palette, unfiltered, where the
//!   pixels have at most 256 colours, and RGB with each row's filter chosen
//!   otherwise, at `Fast`;
//! - one round is one captured frame, and the tick runs on capture time; after
//!   the last frame it runs on, with nothing changing, until nothing is owed
//!   (60 s at most).
//!
//! **Cost** is the process's CPU time over each coder's own work, both coders
//! in one pass over the capture: reading it, `zstd`, and the decoders that
//! check the result all run outside. Each region's encoder has `--threads`
//! (4) as `WLSV`'s has, and the regions and the PNGs are coded one after
//! another, so the CPU time is the measure and not the wall clock. As every
//! measurement of the encoder, run it on an idle machine and alone.
//!
//! **Checked**: both pictures are decoded as a page would — `WLSV` by libvpx,
//! a PNG by the `png` crate, each region by a libvpx decoder of its own — and
//! the last compared with the capture's: the PSNR over B, G and R, and for the
//! tiles how many pixels differ, which once settled is none.
//!
//! The output is a Markdown table, a row per capture per coder: its CPU time
//! all of it, the tiles' ticks and tail included, and the time a frame took
//! the frames' alone; what the page held while the capture ran, as the PSNR
//! of the mean squared error over every frame and the share of frames it held
//! exactly; and where it ended. `--csv` adds
//! one line per frame: `capture,frame,ms,changed,wlsv_us,wlsv_bytes,tiles_us,
//! tiles_bytes,png_bytes,vp9_bytes,streams,tick_us,tick_bytes`, a tick's
//! columns what the ticks since the frame before cost.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use cpu_time::ProcessTime;
use wlshare_rfb::capture::Reader;
use wlshare_rfb::vp9::{Chroma, Speed, Vp9Encoder};
use wlshare_vp9::{Decoder, Stream};

type Error = Box<dyn std::error::Error>;

const USAGE: &str = "usage: motion-bench [--threads N] [--quality Q] [--wlsv-chroma 444|420] [--region-chroma 420|444] [--wlsv-speed usual|fastest] [--region-speed fastest|usual] [--first-sight png|video] [--bands changed|whole] [--cleanups N] [--csv FILE] CAPTURE...";

struct Args {
    captures: Vec<PathBuf>,
    threads: usize,
    quality: Option<u8>,
    wlsv_chroma: Chroma,
    region_chroma: Chroma,
    wlsv_speed: Speed,
    region_speed: Speed,
    /// A picture that starts over is first seen as a stream, as Motion's was,
    /// rather than sent as PNG.
    first_video: bool,
    /// A band no stream touches is sent whole, as Motion's was, rather than
    /// as its changed tiles.
    whole_bands: bool,
    cleanups: usize,
    csv: Option<PathBuf>,
}

fn chroma(value: &str) -> Result<Chroma, String> {
    match value {
        "444" => Ok(Chroma::Yuv444),
        "420" => Ok(Chroma::Yuv420),
        other => Err(format!("{other}: a chroma is 444 or 420")),
    }
}

fn speed(value: &str) -> Result<Speed, String> {
    match value {
        "usual" => Ok(Speed::Usual),
        "fastest" => Ok(Speed::Fastest),
        other => Err(format!("{other}: a speed is usual or fastest")),
    }
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut parsed = Args { captures: Vec::new(), threads: 4, quality: None, wlsv_chroma: Chroma::Yuv444, region_chroma: Chroma::Yuv420, wlsv_speed: Speed::Usual, region_speed: Speed::Fastest, first_video: false, whole_bands: false, cleanups: MAX_CLEANUPS_PER_TICK, csv: None };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} takes a value"));
        match arg.as_str() {
            "--threads" => parsed.threads = value()?.parse().ok().filter(|&n| n > 0).ok_or("--threads takes a count of at least 1")?,
            "--quality" => parsed.quality = Some(value()?.parse().ok().filter(|q| (1..=100).contains(q)).ok_or("--quality takes 1 to 100")?),
            "--wlsv-chroma" => parsed.wlsv_chroma = chroma(&value()?)?,
            "--region-chroma" => parsed.region_chroma = chroma(&value()?)?,
            "--wlsv-speed" => parsed.wlsv_speed = speed(&value()?)?,
            "--region-speed" => parsed.region_speed = speed(&value()?)?,
            "--first-sight" => {
                parsed.first_video = match value()?.as_str() {
                    "video" => true,
                    "png" => false,
                    other => return Err(format!("{other}: a first sight is video or png")),
                }
            }
            "--bands" => {
                parsed.whole_bands = match value()?.as_str() {
                    "whole" => true,
                    "changed" => false,
                    other => return Err(format!("{other}: bands are whole or changed")),
                }
            }
            "--cleanups" => parsed.cleanups = value()?.parse().ok().filter(|&n| n > 0).ok_or("--cleanups takes a count of at least 1")?,
            "--csv" => parsed.csv = Some(PathBuf::from(value()?)),
            flag if flag.starts_with("--") => return Err(format!("{flag}: not a flag")),
            _ => parsed.captures.push(PathBuf::from(arg)),
        }
    }
    if parsed.captures.is_empty() {
        return Err("a capture to read".into());
    }
    Ok(parsed)
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("motion-bench: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let mut csv = match args.csv.as_deref().map(File::create).transpose() {
        Ok(file) => file.map(BufWriter::new),
        Err(e) => {
            eprintln!("motion-bench: the CSV: {e}");
            std::process::exit(1);
        }
    };
    if let Some(csv) = &mut csv {
        let _ = writeln!(csv, "capture,frame,ms,changed,wlsv_us,wlsv_bytes,tiles_us,tiles_bytes,png_bytes,vp9_bytes,streams,tick_us,tick_bytes");
    }
    println!("| capture | coder | frames | seconds | CPU ms | ms/frame | p95 ms/frame | MB | Mbit/s | while it ran | frames exact | ends at | detail |");
    println!("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|");
    let mut failed = false;
    for capture in &args.captures {
        if let Err(e) = bench(capture, &args, csv.as_mut()) {
            eprintln!("motion-bench: {}: {e}", capture.display());
            failed = true;
        }
    }
    if let Some(csv) = &mut csv {
        let _ = csv.flush();
    }
    if failed {
        std::process::exit(1);
    }
}

// ── The capture in ───────────────────────────────────────────────────────────

/// A capture open for reading, with the `zstd` it is read through, if any.
struct Source {
    reader: Reader<Box<dyn Read>>,
    zstd: Option<Child>,
}

impl Source {
    fn open(path: &Path) -> Result<Self, Error> {
        let (input, zstd): (Box<dyn Read>, _) = if path.extension().is_some_and(|e| e == "zst") {
            let file = File::open(path)?;
            let mut child = Command::new("zstd").arg("-dc").stdin(file).stdout(Stdio::piped()).spawn().map_err(|e| format!("running zstd: {e}"))?;
            (Box::new(child.stdout.take().expect("piped")), Some(child))
        } else {
            (Box::new(File::open(path)?), None)
        };
        Ok(Self { reader: Reader::new(Box::new(BufReader::with_capacity(1 << 20, input)) as Box<dyn Read>)?, zstd })
    }

    /// After the capture's end was read: whether `zstd` read all of its own.
    fn finish(mut self) -> Result<(), Error> {
        if let Some(mut child) = self.zstd.take()
            && !child.wait()?.success()
        {
            return Err("zstd could not decompress it".into());
        }
        Ok(())
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        if let Some(child) = &mut self.zstd {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ── Geometry ─────────────────────────────────────────────────────────────────

/// A tile's side: ZRLE's, and Motion's grid from the desktop's top-left.
const TILE: u16 = 64;

/// A rectangle of the desktop, its edges inclusive, as Motion's were.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    left: u16,
    top: u16,
    right: u16,
    bottom: u16,
}

impl Rect {
    /// The whole of a `w`×`h` desktop.
    fn whole((w, h): (u16, u16)) -> Self {
        Self { left: 0, top: 0, right: w - 1, bottom: h - 1 }
    }

    /// A capture's rectangle on a `w`×`h` desktop, clipped to it; `None` for
    /// one with nothing inside.
    fn clipped(r: wlshare_vp9::Rect, (w, h): (u16, u16)) -> Option<Self> {
        let right = (u32::from(r.x) + u32::from(r.width)).min(u32::from(w));
        let bottom = (u32::from(r.y) + u32::from(r.height)).min(u32::from(h));
        (u32::from(r.x) < right && u32::from(r.y) < bottom).then(|| Self { left: r.x, top: r.y, right: (right - 1) as u16, bottom: (bottom - 1) as u16 })
    }

    fn w(self) -> u16 {
        self.right - self.left + 1
    }

    fn h(self) -> u16 {
        self.bottom - self.top + 1
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let r = Self { left: self.left.max(other.left), top: self.top.max(other.top), right: self.right.min(other.right), bottom: self.bottom.min(other.bottom) };
        (r.left <= r.right && r.top <= r.bottom).then_some(r)
    }

    fn contains(self, other: Self) -> bool {
        self.left <= other.left && self.top <= other.top && other.right <= self.right && other.bottom <= self.bottom
    }

    /// This rectangle as wlshare-vp9 takes one, from `origin`'s top-left.
    fn within(self, origin: Self) -> wlshare_vp9::Rect {
        wlshare_vp9::Rect { x: self.left - origin.left, y: self.top - origin.top, width: self.w(), height: self.h() }
    }
}

/// The tiles `rect` touches, each with the part of it inside that tile, row by row.
fn tiles_of(rect: Rect) -> impl Iterator<Item = ((u16, u16), Rect)> {
    (rect.top / TILE..=rect.bottom / TILE).flat_map(move |row| {
        (rect.left / TILE..=rect.right / TILE).map(move |col| {
            let tile = Rect { left: col * TILE, top: row * TILE, right: col.saturating_mul(TILE).saturating_add(TILE - 1), bottom: row.saturating_mul(TILE).saturating_add(TILE - 1) };
            ((col, row), tile.intersect(rect).expect("a tile the rectangle touches"))
        })
    })
}

/// A rectangle of the tile grid, in tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellBox {
    c0: u16,
    r0: u16,
    c1: u16,
    r1: u16,
}

impl CellBox {
    fn of(cell: (u16, u16)) -> Self {
        Self { c0: cell.0, r0: cell.1, c1: cell.0, r1: cell.1 }
    }

    fn area(&self) -> u32 {
        u32::from(self.c1 - self.c0 + 1) * u32::from(self.r1 - self.r0 + 1)
    }

    fn union(&self, other: &Self) -> Self {
        Self { c0: self.c0.min(other.c0), r0: self.r0.min(other.r0), c1: self.c1.max(other.c1), r1: self.r1.max(other.r1) }
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.c0 <= other.c1 && other.c0 <= self.c1 && self.r0 <= other.r1 && other.r0 <= self.r1
    }

    fn contains(&self, (c, r): (u16, u16)) -> bool {
        self.c0 <= c && c <= self.c1 && self.r0 <= r && r <= self.r1
    }

    /// This box in pixels, clipped to a `w`×`h` desktop.
    fn to_rect(self, (w, h): (u16, u16)) -> Option<Rect> {
        let left = self.c0.checked_mul(TILE)?;
        let top = self.r0.checked_mul(TILE)?;
        if left >= w || top >= h {
            return None;
        }
        Some(Rect { left, top, right: self.c1.saturating_mul(TILE).saturating_add(TILE - 1).min(w - 1), bottom: self.r1.saturating_mul(TILE).saturating_add(TILE - 1).min(h - 1) })
    }
}

// ── Motion's decisions: which tiles move, and which become regions ───────────

/// One slot of churn.
const CHURN_SLOT: Duration = Duration::from_millis(100);
/// Slots of history a tile keeps: an 800 ms window.
const CHURN_WINDOW: u64 = 8;
/// The churn at which a tile is moving.
const CHURN_MOVING: u32 = 4;
/// The most streams at once.
const MAX_STREAMS: usize = 4;
/// The least time between two changes of geometry while a stream is live.
const RETUNE: Duration = Duration::from_millis(500);
/// How long a region has nothing moving in it before its stream ends.
const STREAM_IDLE: Duration = Duration::from_millis(1000);
/// The most tiles a merge may cover, as a multiple of the moving tiles it joins.
const MERGE_WASTE: u32 = 2;
/// The fewest moving tiles a component needs to be worth a stream.
const MIN_STREAM_CELLS: u32 = 5;
/// How long a tile a stream carried is still before it is owed as PNG.
const CLEANUP_IDLE: Duration = Duration::from_millis(500);
/// How often the cleanups are looked for.
const CLEANUP_TICK: Duration = Duration::from_millis(250);
/// Cleanups a tick, by default.
const MAX_CLEANUPS_PER_TICK: usize = 40;
/// The rows a report of damage is cut into.
const BAND_ROWS: u16 = 64;
/// How long the tick runs on after a capture's last frame, at most.
const TAIL: Duration = Duration::from_secs(60);

/// One tile's history: bit 0 the slot it last changed in, shifted a place a slot.
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
        (now.saturating_sub(origin).as_millis() / CHURN_SLOT.as_millis()) as u64
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

    fn moving(&mut self, now: Duration) -> Vec<(u16, u16)> {
        let Some(origin) = self.origin else {
            return Vec::new();
        };
        let slot = Self::slot(origin, now);
        let mut moving = Vec::new();
        self.churn.retain(|key, cell| {
            let elapsed = slot.saturating_sub(cell.last_slot);
            cell.history = if elapsed >= CHURN_WINDOW { 0 } else { cell.history << elapsed };
            cell.last_slot = slot;
            if cell.history.count_ones() >= CHURN_MOVING {
                moving.push(*key);
            }
            cell.history != 0
        });
        moving.sort_unstable();
        moving
    }

    fn clear(&mut self) {
        self.origin = None;
        self.churn.clear();
    }
}

/// A candidate region: its box, and how many tiles in it are moving.
#[derive(Clone, Copy, Debug)]
struct Component {
    bbox: CellBox,
    moving: u32,
}

/// The 4-connected components of `cells` worth a stream.
fn components(cells: &[(u16, u16)]) -> Vec<Component> {
    let remaining: HashSet<(u16, u16)> = cells.iter().copied().collect();
    let mut sorted: Vec<(u16, u16)> = remaining.iter().copied().collect();
    sorted.sort_unstable();
    let mut seen: HashSet<(u16, u16)> = HashSet::new();
    let mut components = Vec::new();
    for start in sorted {
        if !seen.insert(start) {
            continue;
        }
        let mut bbox = CellBox::of(start);
        let mut moving = 0;
        let mut stack = vec![start];
        while let Some((c, r)) = stack.pop() {
            moving += 1;
            bbox = bbox.union(&CellBox::of((c, r)));
            for next in [(c.wrapping_sub(1), r), (c + 1, r), (c, r.wrapping_sub(1)), (c, r + 1)] {
                if remaining.contains(&next) && seen.insert(next) {
                    stack.push(next);
                }
            }
        }
        components.push(Component { bbox, moving });
    }
    components.retain(|c| c.moving >= MIN_STREAM_CELLS);
    components
}

type OverlapQueue = BinaryHeap<Reverse<(usize, usize, u32, u32)>>;
type MergeQueue = BinaryHeap<Reverse<(u32, usize, usize, u32, u32)>>;

/// At most `max` pairwise disjoint boxes from the components, as Motion merged
/// them: overlaps first, merged unless the union is past the waste, then the
/// cheapest merges while there are too many, then the smallest dropped.
fn merge(components: Vec<Component>, max: usize) -> Vec<CellBox> {
    let mut overlaps = OverlapQueue::new();
    let mut merges = MergeQueue::new();
    let mut components: Vec<Option<Component>> = components.into_iter().map(Some).collect();
    let mut generations = vec![0; components.len()];
    let mut active = components.len();
    for i in 0..components.len() {
        for j in i + 1..components.len() {
            queue_pair(i, j, &components, &generations, &mut overlaps, &mut merges);
        }
    }
    while active != 0 {
        let overlapping = loop {
            let Some(Reverse((i, j, gi, gj))) = overlaps.pop() else { break None };
            if current(&components, &generations, i, j, gi, gj) {
                break Some((i, j));
            }
        };
        if let Some((i, j)) = overlapping {
            let (left, right) = (components[i].expect("current"), components[j].expect("current"));
            let merged = left.bbox.union(&right.bbox);
            let moving = left.moving + right.moving;
            let nested = merged == left.bbox || merged == right.bbox;
            if nested || merged.area() <= MERGE_WASTE * moving {
                components[j] = None;
                components[i] = Some(Component { bbox: merged, moving });
                generations[i] += 1;
                active -= 1;
                queue_component(i, &components, &generations, &mut overlaps, &mut merges);
            } else {
                let loser = if (right.moving, right.bbox.area()) < (left.moving, left.bbox.area()) { j } else { i };
                components[loser] = None;
                active -= 1;
            }
            continue;
        }
        if active <= max {
            break;
        }
        let best = loop {
            let Some(Reverse((_, i, j, gi, gj))) = merges.pop() else { break None };
            if current(&components, &generations, i, j, gi, gj) {
                break Some((i, j));
            }
        };
        match best {
            Some((i, j)) => {
                let left = components[i].expect("current");
                let right = components[j].take().expect("current");
                components[i] = Some(Component { bbox: left.bbox.union(&right.bbox), moving: left.moving + right.moving });
                generations[i] += 1;
                active -= 1;
                queue_component(i, &components, &generations, &mut overlaps, &mut merges);
            }
            None => {
                let smallest = components.iter().enumerate().filter_map(|(i, c)| c.map(|c| (i, c))).min_by_key(|(_, c)| (c.moving, c.bbox.area())).map(|(i, _)| i).expect("more than max");
                components[smallest] = None;
                active -= 1;
            }
        }
    }
    components.into_iter().flatten().map(|c| c.bbox).collect()
}

fn queue_component(at: usize, components: &[Option<Component>], generations: &[u32], overlaps: &mut OverlapQueue, merges: &mut MergeQueue) {
    for other in 0..components.len() {
        if other != at && components[other].is_some() {
            queue_pair(at.min(other), at.max(other), components, generations, overlaps, merges);
        }
    }
}

fn queue_pair(i: usize, j: usize, components: &[Option<Component>], generations: &[u32], overlaps: &mut OverlapQueue, merges: &mut MergeQueue) {
    let (left, right) = (components[i].expect("live"), components[j].expect("live"));
    if left.bbox.overlaps(&right.bbox) {
        overlaps.push(Reverse((i, j, generations[i], generations[j])));
        return;
    }
    let merged = left.bbox.union(&right.bbox);
    if merged.area() <= MERGE_WASTE * (left.moving + right.moving) {
        merges.push(Reverse((merged.area() - left.bbox.area() - right.bbox.area(), i, j, generations[i], generations[j])));
    }
}

fn current(components: &[Option<Component>], generations: &[u32], i: usize, j: usize, gi: u32, gj: u32) -> bool {
    components[i].is_some() && components[j].is_some() && generations[i] == gi && generations[j] == gj
}

// ── The pixels ───────────────────────────────────────────────────────────────

/// Whether `rect` of `a` and `b`, two `B, G, R, X` pictures `stride` apart, differ.
fn differs(a: &[u8], b: &[u8], stride: usize, rect: Rect) -> bool {
    let (from, to) = (usize::from(rect.left) * 4, (usize::from(rect.right) + 1) * 4);
    (rect.top..=rect.bottom).any(|y| {
        let row = usize::from(y) * stride;
        a[row + from..row + to] != b[row + from..row + to]
    })
}

/// Copy `rect` of `from` into `to`, two `B, G, R, X` pictures `stride` apart.
fn blit(to: &mut [u8], from: &[u8], stride: usize, rect: Rect) {
    let (left, right) = (usize::from(rect.left) * 4, (usize::from(rect.right) + 1) * 4);
    for y in rect.top..=rect.bottom {
        let row = usize::from(y) * stride;
        to[row + left..row + right].copy_from_slice(&from[row + left..row + right]);
    }
}

/// `rect` of a `B, G, R, X` picture as packed RGB.
fn crop_rgb(pixels: &[u8], stride: usize, rect: Rect, out: &mut Vec<u8>) {
    out.clear();
    for y in rect.top..=rect.bottom {
        let row = usize::from(y) * stride;
        for px in pixels[row + usize::from(rect.left) * 4..row + (usize::from(rect.right) + 1) * 4].as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
}

/// The colours of packed RGB, at most 256, and each pixel's index; `None`
/// past 256. The gateway's tiles' table, a pixel the same as the one before
/// taking no lookup.
fn palette(rgb: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    const PALETTE: usize = 256;
    const SLOTS: usize = PALETTE * 4;
    let mut table = [(0u32, 0u8); SLOTS];
    let mut colours = Vec::new();
    let mut indices = Vec::with_capacity(rgb.len() / 3);
    let mut last: Option<(u32, u8)> = None;
    for pixel in rgb.as_chunks::<3>().0 {
        let key = u32::from(pixel[0]) << 16 | u32::from(pixel[1]) << 8 | u32::from(pixel[2]);
        if let Some((colour, index)) = last
            && colour == key
        {
            indices.push(index);
            continue;
        }
        let mut slot = (key.wrapping_mul(0x9E37_79B1) >> 22) as usize % SLOTS;
        let index = loop {
            match table[slot] {
                (0, _) => {
                    let count = colours.len() / 3;
                    if count == PALETTE {
                        return None;
                    }
                    colours.extend_from_slice(pixel);
                    table[slot] = (key + 1, count as u8);
                    break count as u8;
                }
                (held, index) if held == key + 1 => break index,
                _ => slot = (slot + 1) % SLOTS,
            }
        };
        last = Some((key, index));
        indices.push(index);
    }
    Some((colours, indices))
}

/// Packed RGB as the gateway's tile PNG: a palette unfiltered where it fits
/// one, RGB with each row's filter chosen otherwise, at `Fast`.
fn encode_png(w: u16, h: u16, rgb: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(rgb.len() / 4 + 256);
    let mut encoder = png::Encoder::new(&mut out, u32::from(w), u32::from(h));
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let indexed = palette(rgb);
    match &indexed {
        Some((colours, _)) => {
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_palette(colours.clone());
            encoder.set_filter(png::Filter::NoFilter);
        }
        None => {
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_filter(png::Filter::Adaptive);
        }
    }
    let mut writer = encoder.write_header()?;
    match &indexed {
        Some((_, indices)) => writer.write_image_data(indices)?,
        None => writer.write_image_data(rgb)?,
    }
    writer.finish()?;
    Ok(out)
}

// ── The tiles and the regions ────────────────────────────────────────────────

/// What goes to the page.
enum Sent {
    /// An exact rectangle, as PNG: damage, or with `cleanup` a tile a stream
    /// left lossy.
    Png { rect: Rect, png: Vec<u8>, cleanup: bool },
    /// A region stream's frame.
    Unit { serial: u64, rect: Rect, frame: Vec<u8>, keyframe: bool },
}

/// One live region stream.
struct Live {
    /// Which stream this is, never reused: the page's decoder is keyed by it.
    serial: u64,
    rect: Rect,
    cells: Vec<(u16, u16)>,
    stream: Stream,
    /// The damage inside the region since its last frame, from its top-left.
    pending: Vec<wlshare_vp9::Rect>,
    dirty: bool,
    keyframe_owed: bool,
    carried: bool,
    moving_at: Duration,
    /// The first sight of a picture that starts over.
    first: bool,
}

/// The planned picture of one desktop size: the mirror, the churn, the
/// streams and what they owe.
struct Tiles {
    size: (u16, u16),
    stride: usize,
    mirror: Vec<u8>,
    churn: Churn,
    /// Tiles the page cannot be counted on to hold.
    unknown: HashSet<(u16, u16)>,
    first_owed: bool,
    live: Vec<Live>,
    covered: HashSet<(u16, u16)>,
    /// Tile → when its lossy copy was last sent.
    debts: HashMap<(u16, u16), Duration>,
    retuned_at: Option<Duration>,
    quality: u8,
    chroma: Chroma,
    speed: Speed,
    first_video: bool,
    whole_bands: bool,
    threads: usize,
    serial: u64,
    scratch: Vec<u8>,
}

impl Tiles {
    fn new(size: (u16, u16), quality: u8, chroma: Chroma, speed: Speed, args: &Args) -> Self {
        let stride = usize::from(size.0) * 4;
        let mut tiles = Self {
            size,
            stride,
            mirror: vec![0; stride * usize::from(size.1)],
            churn: Churn::default(),
            unknown: HashSet::new(),
            first_owed: true,
            live: Vec::new(),
            covered: HashSet::new(),
            debts: HashMap::new(),
            retuned_at: None,
            quality,
            chroma,
            speed,
            first_video: args.first_video,
            whole_bands: args.whole_bands,
            threads: args.threads,
            serial: 0,
            scratch: Vec::new(),
        };
        tiles.forget();
        tiles
    }

    /// The page holds nothing: every stream and debt dropped, the churn too,
    /// and the next damage seen first as one stream over the desktop.
    fn forget(&mut self) {
        self.live.clear();
        self.covered.clear();
        self.debts.clear();
        self.retuned_at = None;
        self.churn.clear();
        self.first_owed = true;
        self.unknown = tiles_of(Rect::whole(self.size)).map(|(cell, _)| cell).collect();
    }

    fn set_quality(&mut self, quality: u8) {
        if quality != self.quality {
            self.quality = quality;
            for live in &mut self.live {
                live.stream.set_quality(quality);
            }
        }
    }

    /// Whether the page holds the picture exactly, with nothing streaming.
    fn settled(&self) -> bool {
        self.live.is_empty() && self.debts.is_empty()
    }

    /// The pixel rectangle of a tile.
    fn cell_rect(&self, cell: (u16, u16)) -> Rect {
        CellBox::of(cell).to_rect(self.size).expect("a tile of the desktop")
    }

    /// One captured frame: its damage, and the round at its end.
    fn frame(&mut self, pixels: &[u8], rects: &[Rect], now: Duration, sent: &mut Vec<Sent>) -> Result<(), Error> {
        if std::mem::take(&mut self.first_owed) && self.first_video {
            self.first_sight(now)?;
        }
        for &rect in rects {
            self.damage(pixels, rect, now, sent)?;
        }
        self.round(now, sent)
    }

    fn damage(&mut self, pixels: &[u8], rect: Rect, now: Duration, sent: &mut Vec<Sent>) -> Result<(), Error> {
        let stride = self.stride;
        let mut changed: HashSet<(u16, u16)> = tiles_of(rect).filter(|(_, part)| differs(&self.mirror, pixels, stride, *part)).map(|(cell, _)| cell).collect();
        for (cell, part) in tiles_of(rect) {
            if self.unknown.contains(&cell) {
                changed.insert(cell);
                if part == self.cell_rect(cell) {
                    self.unknown.remove(&cell);
                }
            }
        }
        for top in (rect.top..=rect.bottom).step_by(usize::from(BAND_ROWS)) {
            let band = Rect { top, bottom: top.saturating_add(BAND_ROWS - 1).min(rect.bottom), ..rect };
            blit(&mut self.mirror, pixels, stride, band);
            for live in &mut self.live {
                if let Some(part) = live.rect.intersect(band) {
                    live.dirty = true;
                    live.pending.push(part.within(live.rect));
                }
            }
            let cells: Vec<((u16, u16), Rect)> = tiles_of(band).collect();
            for (cell, _) in &cells {
                if changed.contains(cell) {
                    self.churn.observe(*cell, now);
                }
            }
            let split = !self.covered.is_empty() && cells.iter().any(|(cell, _)| self.covered.contains(cell));
            if !split && self.whole_bands {
                self.send_png(band, false, sent)?;
                continue;
            }
            let mut runs: Vec<Rect> = Vec::new();
            for (cell, part) in cells {
                if self.covered.contains(&cell) || !changed.contains(&cell) {
                    continue;
                }
                match runs.last_mut() {
                    Some(run) if run.top == part.top && run.right.checked_add(1) == Some(part.left) => run.right = part.right,
                    _ => runs.push(part),
                }
            }
            for run in runs {
                self.send_png(run, false, sent)?;
            }
        }
        Ok(())
    }

    /// Send `rect` of the mirror exactly, which pays the debt of every tile it
    /// covers whole.
    fn send_png(&mut self, rect: Rect, cleanup: bool, sent: &mut Vec<Sent>) -> Result<(), Error> {
        crop_rgb(&self.mirror, self.stride, rect, &mut self.scratch);
        let png = encode_png(rect.w(), rect.h(), &self.scratch)?;
        for (cell, _) in tiles_of(rect) {
            if rect.contains(self.cell_rect(cell)) {
                self.debts.remove(&cell);
            }
        }
        sent.push(Sent::Png { rect, png, cleanup });
        Ok(())
    }

    /// The frame's end: the regions laid out again from the churn, and every
    /// stream with pixels waiting coded.
    fn round(&mut self, now: Duration, sent: &mut Vec<Sent>) -> Result<(), Error> {
        let moving = self.churn.moving(now);
        self.retune(&moving, now)?;
        let stride = self.stride;
        for live in &mut self.live {
            live.carried = false;
            if !live.dirty {
                continue;
            }
            let at = usize::from(live.rect.top) * stride + usize::from(live.rect.left) * 4;
            live.stream.read_bgrx(&self.mirror[at..], stride, Some(&live.pending))?;
            live.pending.clear();
            let mut frame = Vec::new();
            let keyframe = match live.stream.encode(live.keyframe_owed, &mut frame) {
                Ok(keyframe) => keyframe,
                // The stream owes its whole picture to the next frame.
                Err(wlshare_vp9::Error::NoFrame) => continue,
                Err(e) => return Err(e.into()),
            };
            live.dirty = false;
            live.keyframe_owed = false;
            live.carried = true;
            sent.push(Sent::Unit { serial: live.serial, rect: live.rect, frame, keyframe });
        }
        for live in &self.live {
            if live.carried {
                for cell in &live.cells {
                    self.debts.insert(*cell, now);
                }
            }
        }
        if self.unknown.is_empty() {
            let before = self.live.len();
            self.live.retain(|live| !live.first || live.dirty);
            if self.live.len() != before {
                self.recover();
            }
        }
        Ok(())
    }

    fn recover(&mut self) {
        self.covered = self.live.iter().flat_map(|live| live.cells.iter().copied()).collect();
    }

    fn retune(&mut self, moving: &[(u16, u16)], now: Duration) -> Result<(), Error> {
        if !self.live.is_empty() && self.retuned_at.is_some_and(|at| now.saturating_sub(at) < RETUNE) {
            return Ok(());
        }
        self.retuned_at = Some(now);
        let worthy = components(moving);
        let busy: HashSet<(u16, u16)> = moving.iter().copied().filter(|cell| worthy.iter().any(|c| c.bbox.contains(*cell))).collect();
        for live in &mut self.live {
            if !live.first && live.cells.iter().any(|cell| busy.contains(cell)) {
                live.moving_at = now;
            }
        }
        let size = self.size;
        let wanted: Vec<Rect> = merge(worthy, MAX_STREAMS).into_iter().filter_map(|bbox| bbox.to_rect(size)).collect();
        let (mut next, mut old): (Vec<Live>, Vec<Live>) = std::mem::take(&mut self.live).into_iter().partition(|live| {
            if live.first {
                return false;
            }
            let mut touched = wanted.iter().filter(|rect| live.rect.intersect(**rect).is_some());
            touched.clone().next().is_some() && touched.all(|rect| live.rect.contains(*rect))
        });
        for live in &mut next {
            live.moving_at = now;
        }
        for rect in wanted {
            if next.iter().any(|live| live.rect.contains(rect)) {
                continue;
            }
            if next.len() < MAX_STREAMS {
                old.retain(|live| live.rect.intersect(rect).is_none());
                next.push(self.build(rect, now)?);
            }
        }
        for live in old {
            let idle = now.saturating_sub(live.moving_at) >= STREAM_IDLE;
            let overlaps = next.iter().any(|kept| kept.rect.intersect(live.rect).is_some());
            if !idle && !overlaps && next.len() < MAX_STREAMS {
                next.push(live);
            }
        }
        self.live = next;
        self.recover();
        Ok(())
    }

    /// A stream over `rect`, every tile of it owed from now.
    fn build(&mut self, rect: Rect, now: Duration) -> Result<Live, Error> {
        let mut stream = Stream::new(rect.w(), rect.h(), self.quality, self.chroma, self.threads)?;
        stream.set_speed(self.speed);
        let cells: Vec<(u16, u16)> = tiles_of(rect).map(|(cell, _)| cell).collect();
        for cell in &cells {
            self.debts.insert(*cell, now);
        }
        self.serial += 1;
        Ok(Live { serial: self.serial, rect, cells, stream, pending: Vec::new(), dirty: true, keyframe_owed: true, carried: false, moving_at: now, first: false })
    }

    fn first_sight(&mut self, now: Duration) -> Result<(), Error> {
        let mut live = self.build(Rect::whole(self.size), now)?;
        live.first = true;
        self.live.push(live);
        self.recover();
        Ok(())
    }

    /// The cleanup tick: the streams whose motion stopped ended, and up to
    /// `max` tiles still for [`CLEANUP_IDLE`] sent exactly, oldest first, as
    /// runs along a row.
    fn tick(&mut self, now: Duration, max: usize, sent: &mut Vec<Sent>) -> Result<(), Error> {
        let before = self.live.len();
        self.live.retain(|live| now.saturating_sub(live.moving_at) < STREAM_IDLE);
        if self.live.len() != before {
            self.recover();
        }
        let mut ready: Vec<((u16, u16), Duration)> = self.debts.iter().filter(|(cell, at)| !self.covered.contains(*cell) && now.saturating_sub(**at) >= CLEANUP_IDLE).map(|(cell, at)| (*cell, *at)).collect();
        ready.sort_unstable_by_key(|((col, row), at)| (*at, *row, *col));
        ready.truncate(max);
        let mut cells: Vec<(u16, u16)> = ready.into_iter().map(|(cell, _)| cell).collect();
        for cell in &cells {
            self.debts.remove(cell);
        }
        cells.sort_unstable_by_key(|(col, row)| (*row, *col));
        let mut runs: Vec<CellBox> = Vec::new();
        for (col, row) in cells {
            match runs.last_mut() {
                Some(run) if run.r0 == row && run.c1.checked_add(1) == Some(col) => run.c1 = col,
                _ => runs.push(CellBox::of((col, row))),
            }
        }
        for run in runs {
            if let Some(rect) = run.to_rect(self.size) {
                self.send_png(rect, true, sent)?;
            }
        }
        Ok(())
    }
}

// ── The page, which checks what each sends ───────────────────────────────────

/// What a page draws of the tiles and the regions.
struct Page {
    stride: usize,
    canvas: Vec<u8>,
    decoders: HashMap<u64, Decoder>,
    scratch: Vec<u8>,
}

impl Page {
    fn new((w, h): (u16, u16)) -> Self {
        let stride = usize::from(w) * 4;
        Self { stride, canvas: vec![0; stride * usize::from(h)], decoders: HashMap::new(), scratch: Vec::new() }
    }

    fn draw(&mut self, sent: &Sent) -> Result<(), Error> {
        match sent {
            Sent::Png { rect, png, .. } => {
                let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
                decoder.set_transformations(png::Transformations::EXPAND);
                let mut reader = decoder.read_info()?;
                let mut rgb = vec![0; reader.output_buffer_size().ok_or("a PNG too large")?];
                let info = reader.next_frame(&mut rgb)?;
                if (info.width, info.height, info.color_type) != (u32::from(rect.w()), u32::from(rect.h()), png::ColorType::Rgb) {
                    return Err(format!("a PNG of {}x{} {:?} for {}x{}", info.width, info.height, info.color_type, rect.w(), rect.h()).into());
                }
                let mut pixels = rgb.as_chunks::<3>().0.iter();
                for y in rect.top..=rect.bottom {
                    let row = usize::from(y) * self.stride;
                    for px in self.canvas[row + usize::from(rect.left) * 4..row + (usize::from(rect.right) + 1) * 4].as_chunks_mut::<4>().0 {
                        let c = pixels.next().expect("a pixel for every one");
                        px[..3].copy_from_slice(&[c[2], c[1], c[0]]);
                    }
                }
            }
            Sent::Unit { serial, rect, frame, .. } => {
                if !self.decoders.contains_key(serial) {
                    self.decoders.insert(*serial, Decoder::new(2)?);
                }
                let decoded = self.decoders.get_mut(serial).expect("just made").decode(frame)?;
                let (w, h) = (usize::from(rect.w()), usize::from(rect.h()));
                if decoded.size() != (w as u32, h as u32) {
                    return Err(format!("a region of {}x{} decoded as {:?}", w, h, decoded.size()).into());
                }
                self.scratch.resize(w * h * 4, 0);
                decoded.write_bgrx(&mut self.scratch, w * 4)?;
                for (y, from) in (rect.top..=rect.bottom).zip(self.scratch.chunks_exact(w * 4)) {
                    let row = usize::from(y) * self.stride + usize::from(rect.left) * 4;
                    for (to, from) in self.canvas[row..row + w * 4].as_chunks_mut::<4>().0.iter_mut().zip(from.as_chunks::<4>().0) {
                        to[..3].copy_from_slice(&from[..3]);
                    }
                }
            }
        }
        Ok(())
    }

    /// Drop the decoders of streams no longer live.
    fn keep(&mut self, live: &[Live]) {
        self.decoders.retain(|serial, _| live.iter().any(|l| l.serial == *serial));
    }
}

/// How many pixels of two `B, G, R, X` pictures differ in B, G or R, and the
/// mean squared error over those three.
fn compare(a: &[u8], b: &[u8]) -> (u64, f64) {
    let (mut off, mut squared) = (0u64, 0u64);
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let mut differs = false;
        for c in 0..3 {
            let d = i64::from(pa[c]) - i64::from(pb[c]);
            squared += (d * d) as u64;
            differs |= d != 0;
        }
        off += u64::from(differs);
    }
    (off, squared as f64 / (a.len() / 4 * 3) as f64)
}

/// A mean squared error as PSNR, "exact" for none.
fn db(mse: f64) -> String {
    if mse == 0.0 { "exact".to_owned() } else { format!("{:.1} dB", 10.0 * (255.0f64 * 255.0 / mse).log10()) }
}

// ── WLSV ─────────────────────────────────────────────────────────────────────

/// `WLSV` at one size, and its decoder.
struct Wlsv {
    encoder: Vp9Encoder,
    decoder: Decoder,
    started: bool,
    picture: Vec<u8>,
    out: Vec<u8>,
}

impl Wlsv {
    /// The encoder and what making it cost, as a region stream's making is
    /// the tiles' cost.
    fn new((w, h): (u16, u16), quality: u8, chroma: Chroma, speed: Speed, threads: usize) -> Result<(Self, Duration), Error> {
        let decoder = Decoder::new(2)?;
        let cpu = ProcessTime::now();
        let mut encoder = Vp9Encoder::with_threads(w, h, quality, chroma, threads)?;
        encoder.set_speed(speed);
        let made = cpu.elapsed();
        Ok((Self { encoder, decoder, started: false, picture: vec![0; usize::from(w) * 4 * usize::from(h)], out: Vec::new() }, made))
    }

    /// Decode what the last encode appended, if anything.
    fn check(&mut self) -> Result<(), Error> {
        if self.out.len() > 4 {
            let (w, _) = self.encoder.size();
            self.decoder.decode(&self.out[4..])?.write_bgrx(&mut self.picture, usize::from(w) * 4)?;
        }
        Ok(())
    }
}

// ── The bench ────────────────────────────────────────────────────────────────

/// What one coder cost over a capture.
#[derive(Default)]
struct Tally {
    frames: u64,
    cpu: Duration,
    per_frame: Vec<Duration>,
    bytes: u64,
    /// The tiles': damage PNGs, cleanup PNGs, region frames.
    png: u64,
    cleanup: u64,
    vp9: u64,
    keyframes: u64,
    streams: u64,
    tail_cpu: Duration,
    tail: Duration,
    settled: bool,
    /// The page against the source after each frame: the squared error
    /// summed, and the frames it held exactly.
    mse: f64,
    exact: u64,
}

impl Tally {
    /// What the page held after a frame.
    fn held(&mut self, page: &[u8], source: &[u8]) {
        let (off, mse) = compare(page, source);
        self.mse += mse;
        self.exact += u64::from(off == 0);
    }

    fn add(&mut self, sent: &Sent) {
        match sent {
            Sent::Png { png, cleanup: false, .. } => self.png += png.len() as u64,
            Sent::Png { png, cleanup: true, .. } => self.cleanup += png.len() as u64,
            Sent::Unit { frame, keyframe, .. } => {
                self.vp9 += frame.len() as u64;
                self.keyframes += u64::from(*keyframe);
            }
        }
        self.bytes += match sent {
            Sent::Png { png, .. } => png.len(),
            Sent::Unit { frame, .. } => frame.len(),
        } as u64;
    }
}

/// One size of a capture, coded both ways.
struct Run {
    wlsv: Wlsv,
    tiles: Tiles,
    page: Page,
    next_tick: Duration,
}

fn bench(path: &Path, args: &Args, mut csv: Option<&mut BufWriter<File>>) -> Result<(), Error> {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().trim_end_matches(".zst").trim_end_matches(".wlcap").to_owned());
    let mut source = Source::open(path)?;
    let (mut wlsv, mut tiles) = (Tally::default(), Tally::default());
    let mut run: Option<Run> = None;
    let (mut first_at, mut last_at, mut ceiling) = (None, Duration::ZERO, 0u8);
    let mut sent = Vec::new();
    let mut rects = Vec::new();
    while let Some(frame) = source.reader.next()? {
        let quality = args.quality.unwrap_or(frame.quality);
        ceiling = ceiling.max(quality);
        let now = frame.at;
        first_at.get_or_insert(now);
        last_at = now;
        let stride = usize::from(frame.size.0) * 4;
        let mut setup = Duration::ZERO;
        if run.as_ref().is_none_or(|r| r.tiles.size != frame.size) {
            let (encoder, cost) = Wlsv::new(frame.size, quality, args.wlsv_chroma, args.wlsv_speed, args.threads)?;
            setup = cost;
            run = Some(Run {
                wlsv: encoder,
                tiles: Tiles::new(frame.size, quality, args.region_chroma, args.region_speed, args),
                page: Page::new(frame.size),
                next_tick: now + CLEANUP_TICK,
            });
        }
        let run = run.as_mut().expect("just made");

        // The ticks due before this frame.
        let (mut tick_cpu, mut tick_bytes) = (Duration::ZERO, 0u64);
        while run.next_tick <= now {
            sent.clear();
            let cpu = ProcessTime::now();
            run.tiles.tick(run.next_tick, args.cleanups, &mut sent)?;
            tick_cpu += cpu.elapsed();
            for s in &sent {
                tiles.add(s);
                tick_bytes += match s {
                    Sent::Png { png, .. } => png.len() as u64,
                    Sent::Unit { frame, .. } => frame.len() as u64,
                };
                run.page.draw(s)?;
            }
            run.page.keep(&run.tiles.live);
            run.next_tick += CLEANUP_TICK;
        }
        tiles.cpu += tick_cpu;

        // WLSV.
        let cpu = ProcessTime::now();
        let w = &mut run.wlsv;
        if w.encoder.quality() != quality {
            w.encoder.set_quality(quality);
        }
        w.out.clear();
        w.encoder.read(frame.pixels, stride, if w.started { frame.changed } else { None })?;
        let made = w.encoder.encode(!w.started || frame.keyframe, &mut w.out)?;
        let wlsv_cpu = cpu.elapsed() + setup;
        w.started = true;
        wlsv.frames += 1;
        wlsv.cpu += wlsv_cpu;
        wlsv.per_frame.push(wlsv_cpu);
        let wlsv_bytes = if made { w.out.len() as u64 - 4 } else { 0 };
        wlsv.bytes += wlsv_bytes;
        if made && is_keyframe(&w.out[4..]) {
            wlsv.keyframes += 1;
        }
        w.check()?;

        // The tiles.
        rects.clear();
        match frame.changed {
            Some(changed) => rects.extend(changed.iter().filter_map(|r| Rect::clipped(*r, frame.size))),
            None => rects.push(Rect::whole(frame.size)),
        }
        sent.clear();
        let cpu = ProcessTime::now();
        if frame.keyframe && !run.tiles.first_owed {
            run.tiles.forget();
        }
        run.tiles.set_quality(quality);
        let built = run.tiles.serial;
        run.tiles.frame(frame.pixels, &rects, now, &mut sent)?;
        let tiles_cpu = cpu.elapsed();
        tiles.streams += run.tiles.serial - built;
        tiles.frames += 1;
        tiles.cpu += tiles_cpu;
        tiles.per_frame.push(tiles_cpu);
        let (mut png_bytes, mut vp9_bytes) = (0u64, 0u64);
        for s in &sent {
            tiles.add(s);
            match s {
                Sent::Png { png, .. } => png_bytes += png.len() as u64,
                Sent::Unit { frame, .. } => vp9_bytes += frame.len() as u64,
            }
            run.page.draw(s)?;
        }
        run.page.keep(&run.tiles.live);
        wlsv.held(&run.wlsv.picture, &run.tiles.mirror);
        tiles.held(&run.page.canvas, &run.tiles.mirror);

        if let Some(csv) = csv.as_deref_mut() {
            writeln!(
                csv,
                "{name},{},{},{},{},{wlsv_bytes},{},{},{png_bytes},{vp9_bytes},{},{},{tick_bytes}",
                frame.index,
                (now - first_at.expect("set")).as_millis(),
                frame.changed_pixels(),
                wlsv_cpu.as_micros(),
                tiles_cpu.as_micros(),
                png_bytes + vp9_bytes,
                run.tiles.live.len(),
                tick_cpu.as_micros(),
            )?;
        }
    }
    source.finish()?;
    let mut run = run.ok_or("the capture holds no frames")?;

    // WLSV's settle, where the walk left it coarse.
    if run.wlsv.encoder.coarsest() < ceiling {
        let cpu = ProcessTime::now();
        run.wlsv.out.clear();
        let made = run.wlsv.encoder.settle(ceiling, false, &mut run.wlsv.out)?;
        wlsv.tail_cpu = cpu.elapsed();
        wlsv.cpu += wlsv.tail_cpu;
        if made {
            wlsv.bytes += run.wlsv.out.len() as u64 - 4;
        }
        run.wlsv.check()?;
    }

    // The tiles' tail: the tick on, nothing changing, until nothing is owed.
    let mut now = run.next_tick;
    while !run.tiles.settled() && now <= last_at + TAIL {
        sent.clear();
        let cpu = ProcessTime::now();
        run.tiles.tick(now, args.cleanups, &mut sent)?;
        tiles.tail_cpu += cpu.elapsed();
        for s in &sent {
            tiles.add(s);
            run.page.draw(s)?;
        }
        run.page.keep(&run.tiles.live);
        now += CLEANUP_TICK;
    }
    tiles.cpu += tiles.tail_cpu;
    tiles.settled = run.tiles.settled();
    tiles.tail = now.saturating_sub(last_at);

    let seconds = last_at.saturating_sub(first_at.unwrap_or_default()).as_secs_f64();
    let (_, wlsv_mse) = compare(&run.wlsv.picture, &run.tiles.mirror);
    let (off, tiles_mse) = compare(&run.page.canvas, &run.tiles.mirror);
    let wlsv_detail = format!("{} keyframes{}", wlsv.keyframes, if wlsv.tail_cpu > Duration::ZERO { format!(", settled at {ceiling} for {:.1} ms", ms(wlsv.tail_cpu)) } else { String::new() });
    row(&name, &format!("WLSV {}{}", chroma_name(args.wlsv_chroma), speed_name(args.wlsv_speed)), &wlsv, seconds, &db(wlsv_mse), &wlsv_detail);
    let ends = if off == 0 { "exact".to_owned() } else { format!("{off} px off, {}", db(tiles_mse)) };
    let tiles_detail = format!(
        "PNG {:.2} MB, cleanups {:.2} MB, VP9 {:.2} MB in {} streams, {} keyframes; {} in {:.2} s after, {:.1} ms",
        mb(tiles.png),
        mb(tiles.cleanup),
        mb(tiles.vp9),
        tiles.streams,
        tiles.keyframes,
        if tiles.settled { "settled" } else { "NOT settled" },
        tiles.tail.as_secs_f64(),
        ms(tiles.tail_cpu),
    );
    row(&name, &format!("tiles, regions {}{}{}{}", chroma_name(args.region_chroma), speed_name(args.region_speed), if args.first_video { ", first as video" } else { "" }, if args.whole_bands { ", whole bands" } else { "" }), &tiles, seconds, &ends, &tiles_detail);
    Ok(())
}

/// Whether a VP9 frame is a keyframe, as its header says.
fn is_keyframe(frame: &[u8]) -> bool {
    wlshare_vp9::frame_header(frame).is_some_and(|header| header.keyframe)
}

fn chroma_name(chroma: Chroma) -> &'static str {
    match chroma {
        Chroma::Yuv444 => "4:4:4",
        Chroma::Yuv420 => "4:2:0",
    }
}

/// What a coder's name says of its speed: nothing for libvpx's usual 7.
fn speed_name(speed: Speed) -> &'static str {
    match speed {
        Speed::Usual => "",
        Speed::Fastest => ", speed 9",
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1e6
}

fn row(name: &str, coder: &str, tally: &Tally, seconds: f64, ends: &str, detail: &str) {
    let mut per_frame = tally.per_frame.clone();
    per_frame.sort_unstable();
    let p95 = per_frame.get((per_frame.len() * 95 / 100).min(per_frame.len().saturating_sub(1))).copied().unwrap_or_default();
    let mean = if tally.frames == 0 { 0.0 } else { ms(per_frame.iter().sum()) / tally.frames as f64 };
    let rate = if seconds > 0.0 { format!("{:.1}", tally.bytes as f64 * 8.0 / seconds / 1e6) } else { "–".to_owned() };
    let held = if tally.frames == 0 { "–".to_owned() } else { db(tally.mse / tally.frames as f64) };
    let exact = if tally.frames == 0 { 0.0 } else { tally.exact as f64 * 100.0 / tally.frames as f64 };
    println!("| {name} | {coder} | {} | {seconds:.1} | {:.0} | {mean:.2} | {:.2} | {:.2} | {rate} | {held} | {exact:.0}% | {ends} | {detail} |", tally.frames, ms(tally.cpu), ms(p95), mb(tally.bytes));
}
