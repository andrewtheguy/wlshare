//! The tiles encoding's server side: the desktop as exact PNG tiles where it
//! is still, and a video stream of its own for each region that moves, for a
//! client on a link with room to spare — or, on a list whose dial walks, every
//! still a lossy WebP at the walk's dial, for a client that wants the walk
//! more than a picture that ends exact.
//!
//! The encoding — the rectangles and their kinds — is
//! [`wlshare_proto::tiles`]'s. It sits beside [`crate::rfb::vp9`]'s rather than in
//! place of it: the VP9 encoding, one stream over the whole desktop walked on
//! the link, is the picture for a link that is short of bytes, and this one
//! spends bytes the link has for less of the encoder's time and a picture that
//! ends exact. Its rules are the zDesk gateway's Motion's, brought here because
//! only here are the compositor's pixels and damage coded once. A region's
//! stream is H.264, by the host's hardware encoder or by libx264.
//!
//! **The dial walks, where the list lets it.** The dial the list names is a
//! ceiling, and the session's walk of the link ([`wlshare_vp9::walk`]) moves
//! the encoder below it ([`TilesEncoder::set_pace`]) as it moves the VP9
//! encoding's. Each stream follows it from an IDR, since neither H.264
//! encoder takes a new constant quantizer on a running stream. That IDR, as a
//! stream's START, is no keyframe for the walk: it is the load a desktop that
//! moves keeps sending, which the walk is there to judge, where the picture
//! whole again is a burst once; and an update of the tick alone, what the
//! desktop stopping owes, is no verdict ([`Update`]). Every still is a lossy
//! WebP at the dial ([`wlshare_proto::tiles::KIND_WEBP`]), at the ceiling too,
//! and coded once. Nothing is owed exact: a WebP sent below the ceiling is owed again at the ceiling, as a tile a
//! stream carried is, and the debts are paid while the walk is not slowed: a link that is behind is not
//! handed finer pictures of what it has already seen. A list that holds the
//! dial never walks, and every still is exact.
//!
//! **Which tiles move.** The desktop is in 64×64 tiles from its top-left. The
//! damage a read is told of is compared, tile by tile, with what the client
//! was sent, so a redraw of what it holds sends nothing. A tile is moving when
//! it changed in 4 of the last 8 tenths of a second. Moving tiles join into
//! 4-connected components; one under 5 tiles is left to PNG, boxes merge
//! unless the merged box would be more than twice the moving tiles, and at
//! most 4 regions stream at once. The regions are laid out again at most every
//! 500 ms: one that shrinks keeps its stream, one that grows starts another,
//! one with no motion for a second ends.
//!
//! **What moved is a copy.** Where the damage moved pixels the client holds
//! — a scroll, a window dragged — the rectangle they now fill goes as a copy
//! of where they were ([`wlshare_proto::tiles::KIND_COPY`]), found by looking
//! for runs of the changed tiles in the picture before, ahead of the stills,
//! and none of its tiles is a still or churn. A copy reads only what the
//! client holds by then, and every tile it draws on is owed what its source
//! is.
//!
//! **What it was shown is a cached tile.** The client keeps a cache of
//! [`CACHE_SLOTS`] tiles ([`wlshare_proto::tiles::KIND_STORE`]), and the
//! encoder the record of it ([`Cache`]): every whole tile of a still that
//! goes out, flat ones apart, is stored in a slot — the least recently used
//! once all are full — and a still whose whole tile the cache keeps, by the
//! hash of its pixels, goes as a draw of that slot
//! ([`wlshare_proto::tiles::KIND_CACHED`]) instead, the run coded around it.
//! A window switched back to, a menu opened again or a page scrolled back is
//! then a few bytes a tile. A tile stored below the ceiling is owed it when
//! drawn, as a copy of a coarse still is, and is not drawn in place of a
//! still at the ceiling; sent there, it is stored again in its slot. A
//! picture that starts over ([`TilesEncoder::forget`]) forgets the cache.
//!
//! **What a stream codes.** A region is an encoder over its rectangle alone,
//! told the damage inside it as the VP9 encoding's whole-desktop stream is, so
//! the encoder's time follows the area that moves. It is 4:2:0 at the dial the
//! client's list names, held there: what a stream loses of a picture lasts
//! only while the region moves, since it settles. The encoder is the
//! [`RegionCoder`] the daemon hands this one, its host's H.264 encoder: the
//! hardware's, where a region costs the processor its conversion alone, or
//! else libx264 at its fastest preset. A coder that refuses a stream, or
//! fails one midway, fails the encode: a region has no other codec.
//!
//! **It settles.** Every tile a stream carried, or a walk sent below the
//! ceiling, is owed a still at the ceiling — a PNG on a held list — once it
//! has been still for half a second, looked for every quarter of a second
//! ([`TilesEncoder::tick_at`]), the longest owed first, at most 320 tiles or
//! 256 KiB at a time — 40 tiles while the walk is below the ceiling, where
//! the link is short of bytes — and sent after the
//! stream's END, so a desktop that stops is exact on a held list and at the
//! ceiling on a walked one. A picture that
//! starts over — an encoder's first, or one a client asks for whole
//! ([`TilesEncoder::forget`]) — goes whole at once, in bands of 64 rows; from
//! then on a band is sent as the runs of its changed tiles along each row.
//!
//! The measurements these rules were chosen by are the daemon crate's
//! `examples/motion-bench.rs`, which plays the kept captures through this
//! encoder, on the daemon's H.264, and through the VP9 encoding's.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet};
use std::hash::Hasher;
use std::time::Duration;

use thiserror::Error;

use wlshare_proto::tiles::{CACHE_SLOTS, TileHeader, TilesPicture};

use crate::rfb::vp9::Rect;

/// Why a region coder could not open or code a stream: its encoder's reason,
/// in its own words.
pub type RegionError = Box<dyn std::error::Error + Send + Sync>;

/// The coder of region streams the daemon hands a [`TilesEncoder`]: its
/// host's H.264 encoder, the hardware's where the host has one and libx264
/// otherwise, each frame one Annex B access unit, the parameter sets ahead of
/// each keyframe's slices, the picture the region's size rounded up to even
/// sides. The encoder opens a stream of it for each region, and a failure to
/// open or to code is its encode's.
pub trait RegionCoder: Send {
    /// A stream over a `width`×`height` region at `quality`, the 1–100 dial,
    /// which codes nothing until read into.
    fn open(&mut self, width: u16, height: u16, quality: u8) -> Result<Box<dyn RegionStream>, RegionError>;
}

/// One region's stream as a [`RegionCoder`] codes it, in the shape of
/// [`wlshare_vp9::Stream`]: read, then encode.
pub trait RegionStream: Send {
    /// Move the dial: the next frame starts the stream over at a keyframe.
    fn set_quality(&mut self, quality: u8);
    /// Read `pixels` — `B, G, R, X`, the region's size, rows `stride` bytes
    /// apart — as the picture the next frame carries, where `changed` says
    /// it differs from the read before, or anywhere for `None`; the first
    /// read is the whole picture whatever `changed` says.
    fn read_bgrx(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> Result<(), RegionError>;
    /// Encode the picture as last read and append the frame to `out`, a
    /// keyframe where `keyframe` asks for one and for the stream's first
    /// either way: `true` for a frame appended, and `false` where the coder
    /// made none of the picture, nothing appended, and the whole picture
    /// owed to the next frame.
    fn encode(&mut self, keyframe: bool, out: &mut Vec<u8>) -> Result<bool, RegionError>;
}

/// Why a picture could not be read or encoded.
#[derive(Debug, Error)]
pub enum TilesError {
    #[error("a {0}x{1} desktop has no pixels")]
    Empty(u16, u16),
    /// The region coder refused a stream, or a stream its pixels or its frame.
    #[error("coding a region: {0}")]
    Region(RegionError),
    #[error(transparent)]
    Png(#[from] png::EncodingError),
    #[error("coding a WebP: {0}")]
    Webp(String),
    #[error("a {width}x{height} picture at a stride of {stride} does not fit in {bytes} bytes")]
    Buffer { width: u16, height: u16, stride: usize, bytes: usize },
    #[error("a rectangle of {0} bytes is over the {max}-byte ceiling", max = wlshare_proto::MAX_BODY)]
    TooLong(usize),
    #[error("an update of {0} rectangles is more than one can count")]
    TooMany(usize),
}

// ── Geometry ─────────────────────────────────────────────────────────────────

/// A tile's side, ZRLE's, from the desktop's top-left: the encoding's.
const TILE: u16 = wlshare_proto::tiles::TILE;

/// The RemoteFX Progressive encoder, measured here as a Windows host draws
/// without H.264: `WLSHARE_TILES_PROGRESSIVE=1` runs no stream and codes
/// every changed tile, moving or still, as a first pass at the stage of
/// quality 50, a difference where that is smaller, or whole where that
/// costs little more; the settle refines a still tile to 75 and then to
/// full. The dial is not read: a slow link gets fewer frames.
#[allow(dead_code)]
#[path = "../../examples/progressive/mod.rs"]
mod progressive;

/// A Windows host's tables, as zDesk's link probe recorded them on
/// 2026-10-27: one quantization table and the stages of quality 50 and 75.
fn progressive_tables() -> progressive::Tables {
    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }
    let quant = progressive::Quant::from_bytes(hex("66768899a9").try_into().unwrap());
    let stage = |quality: u8, table: &str| {
        let mut b = vec![quality];
        b.extend(hex(table));
        progressive::Stage::from_bytes(&b)
    };
    let mut tables = progressive::Tables { quant: [quant; 3], stages: vec![stage(50, "111122121111112212111111221201"), stage(75, "101111010010111201001011121100"), progressive::Stage::FULL] };
    // Measured: `WLSHARE_TILES_PROGRESSIVE_FINE=1` makes the full table a bit
    // finer in every band above RemoteFX's least quantizer, 6, and holds that
    // bit back at 50 and 75, so the first passes are coded as the host's and
    // only the full stage is finer.
    if std::env::var("WLSHARE_TILES_PROGRESSIVE_FINE").as_deref() == Ok("1") {
        let finer: Vec<u8> = quant.0.iter().map(|&q| u8::from(q > 6)).collect();
        for q in &mut tables.quant {
            for (v, d) in q.0.iter_mut().zip(&finer) {
                *v -= d;
            }
        }
        let last = tables.stages.len() - 1;
        for stage in &mut tables.stages[..last] {
            for q in &mut stage.shifts {
                for (v, d) in q.0.iter_mut().zip(&finer) {
                    *v += d;
                }
            }
        }
    }
    tables
}

/// A progressive first pass whole costs at most this many times the one at
/// its stage.
const PROGRESSIVE_SLACK: f32 = 1.25;

/// What a progressive tile is sent as: its first pass, or the upgrades from
/// the stage it stands at.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProgressivePass {
    First,
    Upgrade,
}

/// A rectangle of the desktop, its edges inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Area {
    left: u16,
    top: u16,
    right: u16,
    bottom: u16,
}

impl Area {
    /// The whole of a `w`×`h` desktop.
    fn whole((w, h): (u16, u16)) -> Self {
        Self { left: 0, top: 0, right: w - 1, bottom: h - 1 }
    }

    /// `r` on a `w`×`h` desktop, clipped to it; `None` for one with nothing inside.
    fn clipped(r: Rect, (w, h): (u16, u16)) -> Option<Self> {
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

    /// This rectangle as a stream takes one, from `origin`'s top-left.
    fn within(self, origin: Self) -> Rect {
        Rect { x: self.left - origin.left, y: self.top - origin.top, width: self.w(), height: self.h() }
    }
}

/// The tiles `area` touches, each with the part of it inside that tile, row by row.
fn tiles_of(area: Area) -> impl Iterator<Item = ((u16, u16), Area)> {
    (area.top / TILE..=area.bottom / TILE).flat_map(move |row| {
        (area.left / TILE..=area.right / TILE).map(move |col| {
            let tile = Area { left: col * TILE, top: row * TILE, right: col.saturating_mul(TILE).saturating_add(TILE - 1), bottom: row.saturating_mul(TILE).saturating_add(TILE - 1) };
            ((col, row), tile.intersect(area).expect("a tile the rectangle touches"))
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
    fn to_area(self, (w, h): (u16, u16)) -> Option<Area> {
        let left = self.c0.checked_mul(TILE)?;
        let top = self.r0.checked_mul(TILE)?;
        if left >= w || top >= h {
            return None;
        }
        Some(Area { left, top, right: self.c1.saturating_mul(TILE).saturating_add(TILE - 1).min(w - 1), bottom: self.r1.saturating_mul(TILE).saturating_add(TILE - 1).min(h - 1) })
    }
}

// ── Which tiles move, and which become regions ───────────────────────────────

/// One slot of churn.
const CHURN_SLOT: Duration = Duration::from_millis(100);
/// Slots of history a tile keeps: an 800 ms window.
const CHURN_WINDOW: u64 = 8;
/// The churn at which a tile is moving.
const CHURN_MOVING: u32 = 4;
/// The most streams at once.
const MAX_STREAMS: usize = 4;
/// The least time between two layouts of the regions while a stream is live.
const RETUNE: Duration = Duration::from_millis(500);
/// How long a region has nothing moving in it before its stream ends.
const STREAM_IDLE: Duration = Duration::from_millis(1000);
/// The most tiles a merge may cover, as a multiple of the moving tiles it joins.
const MERGE_WASTE: u32 = 2;
/// The fewest moving tiles a component needs to be worth a stream.
const MIN_STREAM_CELLS: u32 = 5;
/// How long a tile a stream carried is still before it is owed its still.
const CLEANUP_IDLE: Duration = Duration::from_millis(500);
/// How often what is owed is looked for.
pub const CLEANUP_TICK: Duration = Duration::from_millis(250);
/// How long the desktop is still before what is owed is paid while the walk
/// is coarse: the link that slowed it hears of nothing more once the desktop
/// is quiet, so the walk would never say otherwise, and the picture would stay
/// lossy. Longer than [`CLEANUP_IDLE`], so a pause in typing is not chased
/// while the link is behind.
const COARSE_QUIET: Duration = Duration::from_millis(2000);
/// The most tiles a tick codes on a held list or while the walk stands at the
/// ceiling: about 40 ms of WebP.
const SETTLE_TILES: usize = 320;
/// The most tiles a tick sends while the walk is below the ceiling, where the
/// link is short of bytes.
const SETTLE_TILES_BELOW: usize = 40;
/// The bytes after which a tick sends no more: 8 Mbit/s at a tick every
/// quarter of a second.
const SETTLE_BYTES: usize = 256 * 1024;
/// The rows a report of damage is cut into.
const BAND_ROWS: u16 = 64;

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

    /// In how many of the last [`CHURN_WINDOW`] slots `key` changed, as of `now`.
    fn recent(&self, key: (u16, u16), now: Duration) -> u32 {
        let (Some(origin), Some(cell)) = (self.origin, self.churn.get(&key)) else { return 0 };
        let elapsed = Self::slot(origin, now).saturating_sub(cell.last_slot);
        if elapsed >= CHURN_WINDOW { 0 } else { (cell.history << elapsed).count_ones() }
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

/// At most `max` pairwise disjoint boxes from the components: overlaps first,
/// merged unless the union is past the waste, then the cheapest merges while
/// there are too many, then the smallest dropped.
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

/// Whether `area` of `a` and `b`, two `B, G, R, X` pictures, differ.
fn differs(a: &[u8], a_stride: usize, b: &[u8], b_stride: usize, area: Area) -> bool {
    let (from, to) = (usize::from(area.left) * 4, (usize::from(area.right) + 1) * 4);
    (area.top..=area.bottom).any(|y| {
        let (ra, rb) = (usize::from(y) * a_stride, usize::from(y) * b_stride);
        a[ra + from..ra + to] != b[rb + from..rb + to]
    })
}

/// Copy `area` of `from` into `to`, two `B, G, R, X` pictures.
fn blit(to: &mut [u8], to_stride: usize, from: &[u8], from_stride: usize, area: Area) {
    let (left, right) = (usize::from(area.left) * 4, (usize::from(area.right) + 1) * 4);
    for y in area.top..=area.bottom {
        let (rt, rf) = (usize::from(y) * to_stride, usize::from(y) * from_stride);
        to[rt + left..rt + right].copy_from_slice(&from[rf + left..rf + right]);
    }
}

/// A whole tile's pixels as a cache keys them: a hash of `area` of a
/// `B, G, R, X` picture, and whether it is one colour, which is never stored.
fn tile_key(pixels: &[u8], stride: usize, area: Area) -> (u64, bool) {
    let (from, to) = (usize::from(area.left) * 4, (usize::from(area.right) + 1) * 4);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let first = &pixels[usize::from(area.top) * stride + from..usize::from(area.top) * stride + to];
    let mut flat = first.as_chunks::<4>().0.iter().all(|px| px == first.first_chunk::<4>().expect("a pixel"));
    for y in area.top..=area.bottom {
        let row = &pixels[usize::from(y) * stride + from..usize::from(y) * stride + to];
        hasher.write(row);
        flat &= row == first;
    }
    (hasher.finish(), flat)
}

/// `area` of a `B, G, R, X` picture as packed RGB.
fn crop_rgb(pixels: &[u8], stride: usize, area: Area, out: &mut Vec<u8>) {
    out.clear();
    for y in area.top..=area.bottom {
        let row = usize::from(y) * stride;
        for px in pixels[row + usize::from(area.left) * 4..row + (usize::from(area.right) + 1) * 4].as_chunks::<4>().0 {
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
}

/// The colours of packed RGB, at most 256, and each pixel's index; `None`
/// past 256. A pixel the same as the one before takes no lookup.
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

/// Packed RGB as a PNG appended to `out`: a palette unfiltered where the pixels
/// have at most 256 colours — `indexed`, [`palette`]'s word on them — RGB with
/// each row's filter chosen otherwise, at `png`'s fast compression. What the
/// zDesk gateway codes its own tiles as.
fn encode_png(w: u16, h: u16, rgb: &[u8], indexed: Option<(Vec<u8>, Vec<u8>)>, out: &mut Vec<u8>) -> Result<(), png::EncodingError> {
    let mut encoder = png::Encoder::new(out, u32::from(w), u32::from(h));
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
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
    writer.finish()
}

/// libwebp's method for a still: 0 is its fastest search, since a still is
/// coded on the update's way out.
const WEBP_METHOD: i32 = 0;

/// The widest a WebP is, in pixels: a still wider is sent in pieces.
const WEBP_MAX_WIDTH: u16 = 16_383;

/// `rgb`, `w`×`h` packed, appended to `out` as a lossy WebP at `quality`, the
/// 1–100 dial being libwebp's own scale: a VP8 keyframe in a RIFF container,
/// 4:2:0, which every browser decodes. libwebp is linked statically, built
/// for the target's baseline, and its encoder told to use one thread.
fn encode_webp(w: u16, h: u16, rgb: &[u8], quality: u8, out: &mut Vec<u8>) -> Result<(), TilesError> {
    use libwebp_sys as webp;
    let refused = |what: &str| TilesError::Webp(what.to_owned());
    let mut config = webp::WebPConfig::new().map_err(|()| refused("libwebp refused its own configuration"))?;
    config.quality = f32::from(quality);
    config.method = WEBP_METHOD;
    config.thread_level = 0;
    let mut picture = webp::WebPPicture::new().map_err(|()| refused("libwebp refused its own picture"))?;
    picture.width = i32::from(w);
    picture.height = i32::from(h);
    picture.use_argb = 0;
    // SAFETY: `rgb` holds `w`×`h` packed pixels, which `crop_rgb` made it;
    // the picture's planes are libwebp's own, allocated by the import and
    // freed below; the writer is this frame's, read within its size and
    // cleared below, and the picture points at it only for the encode.
    unsafe {
        if webp::WebPPictureImportRGB(&mut picture, rgb.as_ptr(), i32::from(w) * 3) == 0 {
            webp::WebPPictureFree(&mut picture);
            return Err(refused("libwebp could not take the picture"));
        }
        let mut writer = std::mem::MaybeUninit::<webp::WebPMemoryWriter>::uninit();
        webp::WebPMemoryWriterInit(writer.as_mut_ptr());
        let mut writer = writer.assume_init();
        picture.writer = Some(webp::WebPMemoryWrite);
        picture.custom_ptr = (&raw mut writer).cast();
        let coded = webp::WebPEncode(&config, &mut picture) != 0;
        let error = picture.error_code;
        if coded {
            out.extend_from_slice(std::slice::from_raw_parts(writer.mem, writer.size));
        }
        webp::WebPMemoryWriterClear(&mut writer);
        webp::WebPPictureFree(&mut picture);
        if !coded {
            return Err(TilesError::Webp(format!("libwebp's {error:?}")));
        }
    }
    Ok(())
}

/// What one [`TilesEncoder::encode`] appended: how many rectangles; whether
/// any of them is a still of what the client held nothing of — the whole
/// picture over again, which crosses a link as a keyframe does; and whether
/// all of them are the tick's ([`TilesEncoder::tick_at`]), what was owed once
/// the desktop stopped and none of what a read brought.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Update {
    pub rects: u16,
    pub keyframe: bool,
    pub tick: bool,
}

// ── What moved ───────────────────────────────────────────────────────────────

/// The fewest changed tiles no stream carries that a read looks among for
/// pixels the client holds elsewhere: below it a copy saves less than the
/// looking costs.
const COPY_MIN_TILES: usize = 4;
/// The fewest pixels a copy is sent for.
const COPY_MIN_PIXELS: u32 = 4096;
/// The churn at which a tile is not looked for elsewhere: what a copy draws is
/// no churn, so a tile that changed in place this often is a video or an
/// animation, which nothing was copied to, and the looking would be for
/// nothing every frame.
const COPY_CHURN: u32 = 2;
/// A probe's pixels: a run along a row of the new picture, looked for in the
/// old one.
const PROBE: u16 = 16;
/// The most probes a read takes.
const MAX_PROBES: usize = 64;
/// The most offsets a read tries, the one the most probes found first.
const MAX_OFFSETS: usize = 3;
/// The most places a probe is found at and still counted: one found at more
/// is in something that repeats, a texture or a flat fill with an edge, and
/// says nothing of where anything moved. A run of text is found on many of a
/// page's lines and still counts, its places spread over offsets the scroll's
/// gathers more probes than.
const MAX_PROBE_HITS: usize = 32;
/// The most copies a read sends.
const MAX_COPIES: usize = 8;
/// How far from the tiles it looks among a read looks for where they were, in
/// pixels: further than a window is dragged or a page scrolled between two
/// updates, short of the whole of a large desktop.
const COPY_REACH: u16 = 512;
/// The rows a rectangle's widening is checked on: every this many, and the
/// probe's and the last.
const GROW_SAMPLE: usize = 16;
/// How much larger than the tiles looked among the old picture they are looked
/// for in may be: past it, a few tiles changed across a large desktop, beside
/// a stream, and the looking would cost more than a copy of them could save.
const COPY_WINDOW_RATIO: u32 = 32;
/// The most rows apart the old picture is looked in: a probe is found only on
/// a row looked in, so each sits on a row a probe before it did not, and
/// every offset has probes of its own on rows looked in. Only as many apart
/// as there are probes for two of them on each.
const MAX_SCAN_STEP: u16 = 4;
/// The base of a run's rolling hash: odd, and wide enough to spread a pixel's
/// bits over the hash.
const HASH_BASE: u64 = 0x0000_0100_0000_01b3;

/// A rectangle the client draws from what it holds: `dst` from the same size
/// at `src`, and whether it is owed a still at the ceiling: where its source
/// is, and on a walked list always, as what it draws is a lossy still coded
/// elsewhere, a patchwork of the runs that were sent, and a still sent where
/// it now is codes it better.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CopyRect {
    dst: Area,
    src: (u16, u16),
    owed: bool,
}

impl CopyRect {
    /// The source as a rectangle of the desktop.
    fn source(self) -> Area {
        Area { left: self.src.0, top: self.src.1, right: self.src.0 + (self.dst.right - self.dst.left), bottom: self.src.1 + (self.dst.bottom - self.dst.top) }
    }
}

/// A run of the new picture looked for in the old: where it starts, the area
/// of the damage it is in, and its hash.
struct Probe {
    x: u16,
    y: u16,
    area: usize,
    hash: u64,
}

/// A pixel of a `B, G, R, X` picture, as one word.
fn pixel(pixels: &[u8], stride: usize, x: u16, y: u16) -> u32 {
    let at = usize::from(y) * stride + usize::from(x) * 4;
    u32::from_le_bytes(pixels[at..at + 4].try_into().expect("four bytes"))
}

/// Which of a filter's 65,536 bits a run's hash sets: its top 16 bits once
/// mixed, since a run's last pixels are only in the low bits of [`run_hash`],
/// and a probe one pixel off a page's background would otherwise share the
/// background's bit.
fn filter_bit(hash: u64) -> usize {
    (hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 48) as usize
}

/// The hash of the [`PROBE`] pixels from `(x, y)`, each pixel a digit of base
/// [`HASH_BASE`], the first the most significant, so that the next run's is
/// this one's with a digit taken off the front and one put on the end.
fn run_hash(pixels: &[u8], stride: usize, x: u16, y: u16) -> u64 {
    (x..x + PROBE).fold(0, |hash, x| hash.wrapping_mul(HASH_BASE).wrapping_add(u64::from(pixel(pixels, stride, x, y))))
}

// ── What the client keeps ────────────────────────────────────────────────────

/// One slot of the client's cache of tiles, as the encoder records it.
#[derive(Clone, Copy)]
struct Kept {
    hash: u64,
    /// Stored exact, or at the ceiling: a tile drawn from it is owed nothing.
    settled: bool,
    /// When it was last stored or drawn from; the least recent is evicted first.
    used: u64,
}

/// The encoder's record of the client's cache of tiles
/// ([`wlshare_proto::tiles::KIND_STORE`]): which tile each slot keeps, by
/// the hash of its pixels, and which slot a tile is in. [`CACHE_SLOTS`]
/// slots, filled in order and then the least recently used over again. The
/// client keeps only what it is told to, so this record decides everything
/// and the client agrees with it by construction.
#[derive(Default)]
struct Cache {
    slots: HashMap<u16, Kept>,
    by_hash: HashMap<u64, u16>,
    /// Every slot by when it was last used, the least recent first.
    by_use: BTreeSet<(u64, u16)>,
    stamp: u64,
}

impl Cache {
    fn clear(&mut self) {
        self.slots.clear();
        self.by_hash.clear();
        self.by_use.clear();
    }

    /// The slot that keeps the tile hashed `hash`, and whether it is settled.
    fn find(&self, hash: u64) -> Option<(u16, bool)> {
        let slot = *self.by_hash.get(&hash)?;
        Some((slot, self.slots[&slot].settled))
    }

    /// `slot` was drawn from: it is the most recently used.
    fn touch(&mut self, slot: u16) {
        self.stamp += 1;
        if let Some(kept) = self.slots.get_mut(&slot) {
            self.by_use.remove(&(kept.used, slot));
            kept.used = self.stamp;
            self.by_use.insert((kept.used, slot));
        }
    }

    /// Keep the tile hashed `hash`, `settled` or not, in a slot: the one it is
    /// in already, else the first never filled, else the least recently used,
    /// whose tile is forgotten. The slot, which the client is told to store it in.
    fn store(&mut self, hash: u64, settled: bool) -> u16 {
        self.stamp += 1;
        let slot = match self.by_hash.get(&hash) {
            Some(&slot) => slot,
            None if self.slots.len() < usize::from(CACHE_SLOTS) => self.slots.len() as u16,
            None => {
                let &(used, slot) = self.by_use.iter().next().expect("a full cache has a least recently used slot");
                self.by_use.remove(&(used, slot));
                let evicted = self.slots.remove(&slot).expect("the slot it keeps");
                self.by_hash.remove(&evicted.hash);
                slot
            }
        };
        if let Some(kept) = self.slots.get(&slot) {
            self.by_use.remove(&(kept.used, slot));
        }
        self.slots.insert(slot, Kept { hash, settled, used: self.stamp });
        self.by_hash.insert(hash, slot);
        self.by_use.insert((self.stamp, slot));
        slot
    }
}

// ── The encoder ──────────────────────────────────────────────────────────────

/// One tile of a still's run: the tile, the part of it the still covers, and
/// where that is the whole tile, its key for the cache ([`tile_key`]).
type StillTile = ((u16, u16), Area, Option<(u64, bool)>);

/// One live region stream.
struct Live {
    /// The id its rectangles carry, which no other live stream has.
    id: u8,
    area: Area,
    cells: Vec<(u16, u16)>,
    stream: Box<dyn RegionStream>,
    /// The damage inside the region no frame has carried, from its top-left.
    pending: Vec<Rect>,
    dirty: bool,
    /// Its START went out, and so its END is owed when it ends.
    started: bool,
    carried: bool,
    moving_at: Duration,
}

impl Live {
    /// The damage no frame carried, on the desktop: what the client holds of
    /// it is the stream's last frame from before it.
    fn unsent(&self) -> impl Iterator<Item = Area> + '_ {
        let Area { left, top, .. } = self.area;
        self.pending.iter().map(move |r| Area { left: left + r.x, top: top + r.y, right: left + r.x + r.width - 1, bottom: top + r.y + r.height - 1 })
    }
}

/// The rectangles of one update as they are appended, and how many.
struct Out<'a> {
    out: &'a mut Vec<u8>,
    count: usize,
    /// A still of what the client did not hold was appended.
    keyframe: bool,
}

impl Out<'_> {
    /// Begin a rectangle over `area` with `header`, its length yet to be
    /// filled in, and return where the header is.
    fn header(&mut self, area: Area, header: TileHeader) -> usize {
        self.out.extend_from_slice(&crate::rfb::msg::rect_header(area.left, area.top, area.w(), area.h(), wlshare_proto::ENCODING_TILES));
        let at = self.out.len();
        self.out.extend_from_slice(&header.to_bytes());
        self.count += 1;
        at
    }

    /// Fill in the length of `header`, begun at `at`, as what follows it, or
    /// take back the rectangle begun at `from` if it is past the ceiling.
    fn length(&mut self, from: usize, at: usize, header: TileHeader) -> Result<(), TilesError> {
        let bytes = header.to_bytes().len();
        let len = self.out.len() - at - bytes;
        if len > wlshare_proto::MAX_BODY {
            self.out.truncate(from);
            self.count -= 1;
            return Err(TilesError::TooLong(len));
        }
        self.out[at..at + bytes].copy_from_slice(&header.with_len(len).to_bytes());
        Ok(())
    }

    fn end(&mut self, id: u8, area: Area) {
        self.header(area, TileHeader::End { stream: id });
    }
}

/// One connection's tiles picture, server side, at one desktop size: what the
/// client was sent, which tiles move, the regions' streams and the tiles they
/// owe. A desktop of another size needs another of these.
///
/// Its two halves are two calls, as the VP9 encoding's are: [`Self::read`]
/// takes the damage out of the framebuffer, under its lock, into a picture of
/// its own, and [`Self::encode`] codes what is owed from there once the lock
/// is let go. Time is a [`Duration`] from any origin the caller keeps to.
pub struct TilesEncoder {
    size: (u16, u16),
    stride: usize,
    /// The picture as the encoder last read it, which every PNG and stream is
    /// coded from and every read is compared with.
    mirror: Vec<u8>,
    churn: Churn,
    /// Tiles the client cannot be counted on to hold.
    unknown: HashSet<(u16, u16)>,
    /// The stills the next encode sends are of a picture the client held
    /// nothing of: it starts over, and the update crosses the link whole.
    restarting: bool,
    live: Vec<Live>,
    /// The tiles a live stream carries.
    covered: HashSet<(u16, u16)>,
    /// Tile → when a stream last carried it, or a walk sent its still below
    /// the ceiling, which is owed a still at the ceiling.
    debts: HashMap<(u16, u16), Duration>,
    /// What the client keeps of the tiles it was shown.
    cache: Cache,
    retuned_at: Option<Duration>,
    /// The stills the reads since the last encode owe.
    stills: Vec<Area>,
    /// The copies the reads since the last encode found, which go ahead of
    /// the stills, in the order they were found.
    copies: Vec<CopyRect>,
    /// Streams that ended outside an encode, owed their END.
    ended: Vec<(u8, Area)>,
    /// A read came since the last encode, which owes a round of the streams.
    read: bool,
    next_tick: Duration,
    /// The dial the client's list names: what the streams and the stills are
    /// coded at while the link keeps up, and the finest the walk asks for.
    ceiling: u8,
    /// The list lets the dial walk: every still is a WebP. Held, every still
    /// is a PNG.
    adaptive: bool,
    /// The walk's dial: what the streams and the stills are coded at.
    quality: u8,
    /// The walk has slowed the frames: no debt is paid until it has not.
    coarse: bool,
    /// When a read last found a tile changed.
    changed_at: Duration,
    /// The coder every stream is opened on.
    coder: Box<dyn RegionCoder>,
    scratch: Vec<u8>,
    /// The progressive encoder, where every tile is a progressive tile.
    progressive: Option<progressive::Encoder>,
    /// Progressive tiles sent and their bytes: first passes at 50, at 75,
    /// whole, as differences, upgrades; printed when the encoder goes.
    progressive_sent: [(u64, u64); 5],
    /// How many threads code an update's progressive first passes:
    /// `WLSHARE_TILES_PROGRESSIVE_THREADS`, or every core.
    progressive_threads: usize,
}

impl Drop for TilesEncoder {
    fn drop(&mut self) {
        if self.progressive.is_some() {
            let [first50, first75, whole, diff, upgrade] = self.progressive_sent;
            eprintln!("progressive tiles, count and bytes: first at 50 {first50:?}, first at 75 {first75:?}, whole {whole:?}, differences {diff:?}, upgrades {upgrade:?}");
        }
    }
}

impl TilesEncoder {
    /// An encoder for a `width`×`height` desktop whose streams are `coder`'s,
    /// of the picture the client's list asks for: its ceiling (1–100), where
    /// the walk starts, and whether the dial walks. The client holds nothing
    /// of it yet.
    pub fn new(width: u16, height: u16, picture: TilesPicture, coder: Box<dyn RegionCoder>) -> Result<Self, TilesError> {
        if width == 0 || height == 0 {
            return Err(TilesError::Empty(width, height));
        }
        let stride = usize::from(width) * 4;
        let mut encoder = Self {
            size: (width, height),
            stride,
            mirror: vec![0; stride * usize::from(height)],
            churn: Churn::default(),
            unknown: HashSet::new(),
            restarting: false,
            live: Vec::new(),
            covered: HashSet::new(),
            debts: HashMap::new(),
            cache: Cache::default(),
            retuned_at: None,
            stills: Vec::new(),
            copies: Vec::new(),
            ended: Vec::new(),
            read: false,
            next_tick: Duration::ZERO,
            ceiling: picture.quality,
            adaptive: picture.adaptive,
            quality: picture.quality,
            coarse: false,
            changed_at: Duration::ZERO,
            coder,
            scratch: Vec::new(),
            progressive: None,
            progressive_sent: [(0, 0); 5],
            progressive_threads: std::env::var("WLSHARE_TILES_PROGRESSIVE_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from)),
        };
        if std::env::var("WLSHARE_TILES_PROGRESSIVE").as_deref() == Ok("1") {
            encoder.progressive = Some(progressive::Encoder::new(width, progressive_tables()));
        }
        encoder.forget();
        Ok(encoder)
    }

    /// The desktop size this encoder codes.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// The walk's dial: what the streams are coded at, which
    /// `examples/motion-bench.rs` reads.
    #[allow(dead_code)]
    pub fn quality(&self) -> u8 {
        self.quality
    }

    /// Where the walk stands: `quality` is the dial every stream is coded at
    /// from here, each running stream starting over at a keyframe, and every
    /// still's WebP; `coarse` says the walk has slowed the frames, which holds
    /// the stills owed at the ceiling until it has not.
    pub fn set_pace(&mut self, quality: u8, coarse: bool) {
        self.coarse = coarse;
        if quality != self.quality {
            self.quality = quality;
            for live in &mut self.live {
                live.stream.set_quality(quality);
            }
        }
    }

    /// A new picture, a ceiling or a walk the list did not name before: what
    /// the client holds was coded against the old one, and nothing of it is
    /// owed what the new one would owe, so the picture starts over as
    /// [`Self::forget`] starts it, every live stream's END owed first, at the
    /// new ceiling from here.
    pub fn set_picture(&mut self, picture: TilesPicture) {
        self.ceiling = picture.quality;
        self.adaptive = picture.adaptive;
        self.quality = picture.quality;
        self.forget();
    }

    /// The client holds nothing: every stream ends, with its END at the next
    /// encode, nothing is owed, the cache is forgotten, and the next read is
    /// sent whole.
    pub fn forget(&mut self) {
        for live in self.live.drain(..) {
            if live.started {
                self.ended.push((live.id, live.area));
            }
        }
        self.covered.clear();
        self.debts.clear();
        self.cache.clear();
        self.retuned_at = None;
        self.churn.clear();
        self.stills.clear();
        self.copies.clear();
        self.restarting = true;
        self.unknown = tiles_of(Area::whole(self.size)).map(|(cell, _)| cell).collect();
        if let Some(enc) = &mut self.progressive {
            enc.forget();
        }
    }

    /// The stage a progressive first pass is coded at: 75 while the walk is
    /// at the ceiling and not coarse, else 50.
    fn first_stage(&self) -> usize {
        usize::from(!self.coarse && self.quality >= self.ceiling)
    }

    /// Code the first passes of the progressive tiles of `stills` ahead, on
    /// [`Self::progressive_threads`] threads, each tile the client's cache
    /// does not hold now; [`Self::progressive_tile`] takes each as it comes
    /// to it, in order, where nothing has touched the tile since.
    fn precode(&mut self, stills: &[Area]) {
        if self.progressive.is_none() || self.progressive_threads < 2 {
            return;
        }
        let mut seen = HashSet::new();
        let jobs: Vec<progressive::Job> = stills
            .iter()
            .flat_map(|&area| tiles_of(area))
            .filter(|&(cell, part)| {
                let whole = part.w() == TILE && part.h() == TILE && part == self.cell_area(cell);
                seen.insert(cell) && !(whole && self.cache.find(tile_key(&self.mirror, self.stride, part).0).is_some())
            })
            .map(|(cell, _)| {
                let area = self.cell_area(cell);
                progressive::Job { cell, at: usize::from(area.top) * self.stride + usize::from(area.left) * 4, w: usize::from(area.w()), h: usize::from(area.h()) }
            })
            .collect();
        let stage = self.first_stage();
        self.progressive.as_mut().expect("progressive").precode(&jobs, &self.mirror, self.stride, stage, PROGRESSIVE_SLACK, self.progressive_threads);
    }

    /// Send tile `cell` as a progressive tile: a first pass at the stage of
    /// 75 while the walk is at the ceiling and not coarse, as the host's
    /// passes are at 75 while its link has room, else at 50, or whole where
    /// that costs little more; or, settling, the upgrade
    /// to the stage after the one it stands at, a whole first pass where the
    /// client holds no coefficients of it. Owed the next stage until full.
    fn progressive_tile(&mut self, cell: (u16, u16), pass: ProgressivePass, now: Duration, out: &mut Out) -> Result<bool, TilesError> {
        let area = self.cell_area(cell);
        let at = usize::from(area.top) * self.stride + usize::from(area.left) * 4;
        let stage = self.first_stage();
        let enc = self.progressive.as_mut().expect("progressive");
        let full = enc.tables.stages.len() - 1;
        let (w, h) = (usize::from(area.w()), usize::from(area.h()));
        let (block, kind) = match (pass, enc.stage_of(cell)) {
            (ProgressivePass::Upgrade, Some(stage)) => (enc.upgrade(cell, (stage + 1).min(full)), 4),
            (ProgressivePass::Upgrade, None) => (Some(enc.first(cell, &self.mirror[at..], self.stride, w, h, full).block), 2),
            (ProgressivePass::First, _) => {
                let pass = enc.first_or_full(cell, &self.mirror[at..], self.stride, w, h, stage, PROGRESSIVE_SLACK);
                let kind = if pass.diff { 3 } else if enc.stage_of(cell) == Some(full) { 2 } else { stage };
                (Some(pass.block), kind)
            }
        };
        let settled = enc.stage_of(cell) == Some(full);
        if let Some(block) = block {
            self.progressive_sent[kind].0 += 1;
            self.progressive_sent[kind].1 += block.len() as u64;
            let from = out.out.len();
            let header = TileHeader::Webp { len: 0 };
            let header_at = out.header(area, header);
            out.out.extend_from_slice(&block);
            out.length(from, header_at, header)?;
        }
        if settled {
            self.debts.remove(&cell);
        } else {
            self.debts.insert(cell, now);
        }
        Ok(settled)
    }

    /// What the client shows of tile `cell` from the progressive passes it
    /// was sent, 64×64 `B, G, R, X`, or `None` where it holds none: the
    /// encoder's model of the decoder, which zDesk's shows byte for byte;
    /// `examples/tiles-walk-sim.rs` reads it.
    #[allow(dead_code)]
    pub fn progressive_shown(&mut self, cell: (u16, u16)) -> Option<Vec<u8>> {
        self.progressive.as_mut()?.shown(cell)
    }

    /// The picture as it is now — [`Self::size`] of `B, G, R, X` pixels whose
    /// rows are `stride` bytes apart — where `changed` says it changed since
    /// the read before, or anywhere for `None`: the caller's word, as the VP9
    /// encoding takes it. The rectangles are copied into the encoder's own
    /// picture and compared with what the client holds, so `pixels` is the
    /// framebuffer itself, for as long as the copy takes. Nothing is coded: the
    /// next [`Self::encode`] codes what this owes. The first read of an encoder
    /// made or forgotten is the whole picture whatever `changed` says.
    ///
    /// Where the damage moved pixels the client holds — a scroll, a window
    /// dragged — the rectangle they now fill is a copy of where they were
    /// ([`Self::find_copies`]), and none of its tiles is a still.
    pub fn read(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>, now: Duration) -> Result<(), TilesError> {
        let (w, h) = self.size;
        if stride < usize::from(w) * 4 || pixels.len() < (usize::from(h) - 1) * stride + usize::from(w) * 4 {
            return Err(TilesError::Buffer { width: w, height: h, stride, bytes: pixels.len() });
        }
        let whole = [Area::whole(self.size)];
        let areas: Vec<Area> = match changed.filter(|_| self.unknown.is_empty()) {
            Some(rects) => rects.iter().filter_map(|r| Area::clipped(*r, self.size)).collect(),
            None => whole.to_vec(),
        };
        // The tiles of each area that differ from the picture before any of
        // them, compared once: what copies are looked among, and what each
        // area's damage takes as changed.
        let differ: Vec<HashSet<(u16, u16)>> = areas.iter().map(|area| tiles_of(*area).filter(|(_, part)| differs(&self.mirror, self.stride, pixels, stride, *part)).map(|(cell, _)| cell).collect()).collect();
        let copies = self.find_copies(pixels, stride, &areas, &differ, now);
        // A copy draws what the client holds of its source over every tile it
        // touches, changed or not: each is owed what the source is owed, and
        // is paid where the copy draws it whole from a source owed nothing.
        for copy in &copies {
            for (cell, _) in tiles_of(copy.dst) {
                if let Some(enc) = &mut self.progressive {
                    enc.forget_tile(cell);
                }
                if copy.owed {
                    self.debts.insert(cell, now);
                } else if copy.dst.contains(self.cell_area(cell)) {
                    self.debts.remove(&cell);
                }
            }
        }
        for (i, area) in areas.iter().enumerate() {
            self.damage(pixels, stride, *area, &differ[i], &areas[..i], &copies, now);
        }
        self.copies.extend(copies);
        self.read = true;
        Ok(())
    }

    /// The rectangles of `pixels`, the damage `areas` of the new picture, that
    /// are the client's own pixels from elsewhere, each the largest such
    /// rectangle around a run of pixels found there, at most
    /// [`MAX_COPIES`]; `differ`, each area's tiles that changed.
    ///
    /// Only where enough tiles changed that no stream carries and that have not
    /// been changing in place ([`COPY_CHURN`]): up to [`MAX_PROBES`] runs of
    /// those tiles, none of one colour, are looked for along the rows of the
    /// old picture within the damage and [`COPY_REACH`] of them, every
    /// [`MAX_SCAN_STEP`]th at most, by a rolling hash; of those found at
    /// [`MAX_PROBE_HITS`] places at most, the offsets the most were found at
    /// are grown, from each run, into the rectangle whose rows are the old
    /// picture's at that offset, kept where it draws a changed tile whole. A
    /// copy goes out ahead of the update's stills, so it reads only what the
    /// client holds by then: never a tile a stream carries, or one a still not yet sent is
    /// owed for, or the rectangle of a copy before it in the same read, since
    /// it is matched against the picture before any of them; nor does it draw
    /// over a tile a stream carries.
    fn find_copies(&self, pixels: &[u8], stride: usize, areas: &[Area], differ: &[HashSet<(u16, u16)>], now: Duration) -> Vec<CopyRect> {
        if !self.unknown.is_empty() || areas.iter().map(|area| tiles_of(*area).count()).sum::<usize>() < COPY_MIN_TILES {
            return Vec::new();
        }
        let (old, old_stride) = (&self.mirror[..], self.stride);
        let parts: Vec<(usize, Area)> = areas
            .iter()
            .enumerate()
            .flat_map(|(i, area)| tiles_of(*area).map(move |(cell, part)| (i, cell, part)))
            .filter(|(i, cell, _)| !self.covered.contains(cell) && self.churn.recent(*cell, now) < COPY_CHURN && differ[*i].contains(cell))
            .map(|(i, _, part)| (i, part))
            .collect();
        if parts.len() < COPY_MIN_TILES {
            return Vec::new();
        }

        // The probes, spread over the changed tiles: in each, the first of
        // four rows whose run across its middle is not one colour, each a row
        // on from the one before's, so that they fall on rows of every residue
        // of the scan's step.
        let mut probes = Vec::new();
        for &(area, part) in parts.iter().step_by(parts.len().div_ceil(MAX_PROBES)) {
            if part.w() < PROBE {
                continue;
            }
            let x = part.left + (part.w() - PROBE) / 2;
            let busy = |y: u16| (x + 1..x + PROBE).any(|at| pixel(pixels, stride, at, y) != pixel(pixels, stride, x, y));
            let shift = (probes.len() % usize::from(MAX_SCAN_STEP)) as u16;
            if let Some(y) = (0..4).map(|k| (part.top + part.h() * k / 4 + shift).min(part.bottom)).find(|y| busy(*y)) {
                probes.push(Probe { x, y, area, hash: run_hash(pixels, stride, x, y) });
            }
        }
        // Within the damage, which what moved left too, and within reach of the
        // tiles looked among.
        let union = |a: Area, b: Area| Area { left: a.left.min(b.left), top: a.top.min(b.top), right: a.right.max(b.right), bottom: a.bottom.max(b.bottom) };
        let damage = areas.iter().copied().reduce(union).expect("some damage");
        let looked = parts.iter().map(|(_, part)| *part).reduce(union).expect("some parts");
        let reach = Area { left: looked.left.saturating_sub(COPY_REACH), top: looked.top.saturating_sub(COPY_REACH), right: looked.right.saturating_add(COPY_REACH), bottom: looked.bottom.saturating_add(COPY_REACH) };
        let window = damage.intersect(reach).expect("the parts are in the damage");
        let looked_at: u32 = parts.iter().map(|(_, part)| u32::from(part.w()) * u32::from(part.h())).sum();
        if probes.is_empty() || window.w() < PROBE || u32::from(window.w()) * u32::from(window.h()) > COPY_WINDOW_RATIO * looked_at {
            return Vec::new();
        }
        let step = (probes.len() / 8).clamp(1, usize::from(MAX_SCAN_STEP)) as u16;

        // Every run of the old picture looked in, against the probes: a bit
        // per probe of 65,536 passes over nearly every run that is none of
        // them.
        let mut filter = vec![0u64; 1 << 10];
        for probe in &probes {
            let bit = filter_bit(probe.hash);
            filter[bit / 64] |= 1 << (bit % 64);
        }
        let first = (1..PROBE).fold(1u64, |power, _| power.wrapping_mul(HASH_BASE));
        let mut hits: Vec<Vec<(i32, i32)>> = vec![Vec::new(); probes.len()];
        let mut row: Vec<u64> = Vec::with_capacity(usize::from(window.w()));
        for y in (window.top..=window.bottom).filter(|y| y % step == 0) {
            let at = usize::from(y) * old_stride + usize::from(window.left) * 4;
            row.clear();
            row.extend(old[at..at + usize::from(window.w()) * 4].as_chunks::<4>().0.iter().map(|px| u64::from(u32::from_le_bytes(*px))));
            let mut hash = row[..usize::from(PROBE)].iter().fold(0u64, |hash, px| hash.wrapping_mul(HASH_BASE).wrapping_add(*px));
            let last = row.len() - usize::from(PROBE);
            for dx in 0..=last {
                if dx > 0 {
                    // The run before, its first pixel dropped and the next taken on.
                    hash = hash.wrapping_sub(row[dx - 1].wrapping_mul(first)).wrapping_mul(HASH_BASE).wrapping_add(row[dx + usize::from(PROBE) - 1]);
                }
                let bit = filter_bit(hash);
                if filter[bit / 64] & (1 << (bit % 64)) != 0 {
                    let x = window.left + dx as u16;
                    for (probe, hits) in probes.iter().zip(&mut hits).filter(|(probe, hits)| probe.hash == hash && hits.len() <= MAX_PROBE_HITS) {
                        let offset = (i32::from(x) - i32::from(probe.x), i32::from(y) - i32::from(probe.y));
                        if offset != (0, 0) {
                            hits.push(offset);
                        }
                    }
                }
            }
        }
        let mut found: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, offsets) in hits.iter().enumerate().filter(|(_, offsets)| offsets.len() <= MAX_PROBE_HITS) {
            for offset in offsets {
                found.entry(*offset).or_default().push(i);
            }
        }
        let mut offsets: Vec<((i32, i32), Vec<usize>)> = found.into_iter().filter(|(_, seeds)| seeds.len() >= 2).collect();
        offsets.sort_unstable_by_key(|(offset, seeds)| (Reverse(seeds.len()), *offset));
        let most = offsets.first().map_or(0, |(_, seeds)| seeds.len());
        offsets.retain(|(_, seeds)| seeds.len() * 4 >= most);
        offsets.truncate(MAX_OFFSETS);

        // Each offset grown from its probes, one not inside a copy already.
        let pending: HashSet<(u16, u16)> = self.stills.iter().flat_map(|still| tiles_of(*still).map(|(cell, _)| cell)).collect();
        let mut copies: Vec<CopyRect> = Vec::new();
        for (offset, seeds) in offsets {
            for probe in seeds.into_iter().map(|i| &probes[i]) {
                if copies.len() == MAX_COPIES {
                    return copies;
                }
                let at = Area { left: probe.x, top: probe.y, right: probe.x, bottom: probe.y };
                if copies.iter().any(|copy| copy.dst.contains(at)) {
                    continue;
                }
                let Some(dst) = self.grow(pixels, stride, areas[probe.area], offset, probe, &pending, &copies) else { continue };
                if u32::from(dst.w()) * u32::from(dst.h()) < COPY_MIN_PIXELS {
                    continue;
                }
                // Where it draws only some of a tile, the tile is a still
                // anyway: a copy that draws no changed tile whole saves
                // nothing, and cuts the stills' runs short.
                if !parts.iter().any(|(_, part)| dst.contains(*part)) {
                    continue;
                }
                let src = ((i32::from(dst.left) + offset.0) as u16, (i32::from(dst.top) + offset.1) as u16);
                let copy = CopyRect { dst, src, owed: false };
                let owed = self.adaptive || tiles_of(copy.source()).any(|(cell, _)| self.debts.contains_key(&cell));
                copies.push(CopyRect { owed, ..copy });
            }
        }
        copies
    }

    /// The largest rectangle within `area` around `probe` whose pixels are the
    /// old picture's at `offset`, every row of it and of its source clear of
    /// the tiles [`Self::find_copies`] keeps a copy from: grown up and down
    /// along the probe's run, then a column at a time to either side, each
    /// column checked on a sample of those rows ([`GROW_SAMPLE`]), so that a
    /// pixel that happens to match beside an edge does not widen it past the
    /// edge, then up and down from the probe's row at that width, every row
    /// of it checked whole. `None` where the
    /// probe's own run is not the old picture's, a hash's collision, or is in
    /// a tile it may not touch.
    #[allow(clippy::too_many_arguments)]
    fn grow(&self, pixels: &[u8], stride: usize, area: Area, (dx, dy): (i32, i32), probe: &Probe, pending: &HashSet<(u16, u16)>, copies: &[CopyRect]) -> Option<Area> {
        let (w, h) = self.size;
        let (old, old_stride) = (&self.mirror[..], self.stride);
        let from_x = |x: u16| u16::try_from(i32::from(x) + dx).ok().filter(|x| *x < w);
        let from_y = |y: u16| u16::try_from(i32::from(y) + dy).ok().filter(|y| *y < h);
        // Where a copy may draw, and where it may read from: no stream's tile,
        // no tile owed a still not yet sent, nothing an earlier copy draws.
        let drawable = |cell: (u16, u16)| !self.covered.contains(&cell);
        let readable = |cell: (u16, u16)| !self.covered.contains(&cell) && !pending.contains(&cell);
        let clear = |source: Area| copies.iter().all(|copy| copy.dst.intersect(source).is_none());
        let source = |dst: Area| {
            let (Some(left), Some(top), Some(right), Some(bottom)) = (from_x(dst.left), from_y(dst.top), from_x(dst.right), from_y(dst.bottom)) else { return None };
            Some(Area { left, top, right, bottom })
        };
        let cells = |dst: Area, source: Area| tiles_of(dst).all(|(cell, _)| drawable(cell)) && tiles_of(source).all(|(cell, _)| readable(cell));
        let fits = |dst: Area| source(dst).is_some_and(|source| cells(dst, source) && clear(source));
        // A row's tiles, and its source's, are those of every row of the same
        // band and the same band of the source: the last band's answer, for
        // the width it was asked for.
        let fitted = std::cell::Cell::new(None);
        let row = |y: u16, left: u16, right: u16| {
            let dst = Area { left, top: y, right, bottom: y };
            let Some(from) = source(dst) else { return false };
            let key = (y / TILE, from.top / TILE, left, right);
            let fit = match fitted.get() {
                Some((cached, fit)) if cached == key => fit,
                _ => {
                    let fit = cells(dst, from);
                    fitted.set(Some((key, fit)));
                    fit
                }
            };
            fit && clear(from) && {
                let (oy, ol, or) = (from_y(y).expect("fits"), from_x(left).expect("fits"), from_x(right).expect("fits"));
                let new = &pixels[usize::from(y) * stride + usize::from(left) * 4..usize::from(y) * stride + (usize::from(right) + 1) * 4];
                let was = &old[usize::from(oy) * old_stride + usize::from(ol) * 4..usize::from(oy) * old_stride + (usize::from(or) + 1) * 4];
                new == was
            }
        };
        let column = |x: u16, top: u16, bottom: u16, sample: &[u16]| {
            let dst = Area { left: x, top, right: x, bottom };
            fits(dst) && {
                let ox = from_x(x).expect("fits");
                sample.iter().all(|y| pixel(pixels, stride, x, *y) == pixel(old, old_stride, ox, from_y(*y).expect("fits")))
            }
        };
        let (x, y) = (probe.x, probe.y);
        let (mut left, mut right) = (x, x + PROBE - 1);
        if !row(y, left, right) {
            return None;
        }
        let (mut top, mut bottom) = (y, y);
        let rows = |top: &mut u16, bottom: &mut u16, left: u16, right: u16| {
            while *top > area.top && row(*top - 1, left, right) {
                *top -= 1;
            }
            while *bottom < area.bottom && row(*bottom + 1, left, right) {
                *bottom += 1;
            }
        };
        rows(&mut top, &mut bottom, left, right);
        let sample: Vec<u16> = (top..=bottom).step_by(GROW_SAMPLE).chain([y, bottom]).collect();
        while left > area.left && column(left - 1, top, bottom, &sample) {
            left -= 1;
        }
        while right < area.right && column(right + 1, top, bottom, &sample) {
            right += 1;
        }
        let (mut top, mut bottom) = (y, y);
        rows(&mut top, &mut bottom, left, right);
        Some(Area { left, top, right, bottom })
    }

    /// One rectangle of damage: into the picture, the streams told where it
    /// changed what they carry, the churn of the tiles it changed, and the runs
    /// of those no stream carries owed as stills, band by band. A tile it
    /// names whose pixels are what the client holds is no change to anything,
    /// and one whose part of it a copy draws is no still: what it is owed
    /// [`Self::read`] said. No tile a copy draws on or reads from is churn.
    ///
    /// The tiles that changed are `differ`, but where an area `before` it
    /// reached, which went into the picture first and is compared again.
    #[allow(clippy::too_many_arguments)]
    fn damage(&mut self, pixels: &[u8], stride: usize, area: Area, differ: &HashSet<(u16, u16)>, before: &[Area], copies: &[CopyRect], now: Duration) {
        let mut changed: HashSet<(u16, u16)> = tiles_of(area)
            .filter(|(cell, part)| if before.iter().any(|b| b.intersect(*part).is_some()) { differs(&self.mirror, self.stride, pixels, stride, *part) } else { differ.contains(cell) })
            .map(|(cell, _)| cell)
            .collect();
        if !changed.is_empty() {
            self.changed_at = now;
        }
        for (cell, part) in tiles_of(area) {
            if self.unknown.contains(&cell) {
                changed.insert(cell);
                if part == self.cell_area(cell) {
                    self.unknown.remove(&cell);
                }
            }
        }
        // Bands cut where the tile rows meet, so that a tile the damage covers
        // is in one still whole, which the cache keeps.
        let mut top = area.top;
        while top <= area.bottom {
            let band = Area { top, bottom: (top / BAND_ROWS + 1).saturating_mul(BAND_ROWS).saturating_sub(1).min(area.bottom), ..area };
            top = band.bottom.saturating_add(1);
            blit(&mut self.mirror, self.stride, pixels, stride, band);
            // The runs of changed tiles, and of those the ones no stream carries.
            let (mut changes, mut runs): (Vec<Area>, Vec<Area>) = (Vec::new(), Vec::new());
            let extend = |runs: &mut Vec<Area>, part: Area| match runs.last_mut() {
                Some(run) if run.top == part.top && run.right.checked_add(1) == Some(part.left) => run.right = part.right,
                _ => runs.push(part),
            };
            for (cell, part) in tiles_of(band) {
                if !changed.contains(&cell) {
                    continue;
                }
                if copies.iter().any(|copy| copy.dst.contains(part)) {
                    continue;
                }
                // What a scroll uncovered, or a window moved off, is a still
                // and not motion, or the band it keeps changing would stream.
                if !copies.iter().any(|copy| copy.dst.intersect(part).is_some() || copy.source().intersect(part).is_some()) {
                    self.churn.observe(cell, now);
                }
                extend(&mut changes, part);
                if !self.covered.contains(&cell) {
                    extend(&mut runs, part);
                }
            }
            for live in &mut self.live {
                for part in changes.iter().filter_map(|change| live.area.intersect(*change)) {
                    live.dirty = true;
                    live.pending.push(part.within(live.area));
                }
            }
            self.stills.extend(runs);
        }
    }

    /// Append to `out` the rectangles of what is owed at `now`, and return how
    /// many: the ENDs of streams [`Self::forget`] ended, the copies and the
    /// stills the reads owe, a round of the streams after a read — the
    /// regions laid out again and every stream the damage fell on coded —
    /// and, at its time ([`Self::tick_at`]), the streams whose motion stopped
    /// ended and the tiles still long enough sent at the ceiling; last, as
    /// stills, what changed under a stream that ended here that no frame of it
    /// carried. None when nothing is owed, which is no update to send.
    ///
    /// Tens of milliseconds for a desktop that changed all over. An error
    /// takes back the rectangle it was in, and leaves the ones before it.
    pub fn encode(&mut self, now: Duration, out: &mut Vec<u8>) -> Result<Update, TilesError> {
        let mut out = Out { out, count: 0, keyframe: false };
        for (id, area) in std::mem::take(&mut self.ended) {
            out.end(id, area);
        }
        for copy in std::mem::take(&mut self.copies) {
            out.header(copy.dst, TileHeader::Copy { x: copy.src.0, y: copy.src.1 });
        }
        let stills = std::mem::take(&mut self.stills);
        if !stills.is_empty() && std::mem::take(&mut self.restarting) {
            out.keyframe = true;
        }
        self.precode(&stills);
        for area in stills {
            self.still(area, self.quality, now, &mut out)?;
        }
        if std::mem::take(&mut self.read) {
            self.round(now, &mut out)?;
        }
        let brought = out.count;
        if now >= self.next_tick {
            self.next_tick = now + CLEANUP_TICK;
            self.tick(now, &mut out)?;
        }
        // What the streams that ended here had not carried, after their ENDs.
        let stills = std::mem::take(&mut self.stills);
        self.precode(&stills);
        for area in stills {
            self.still(area, self.quality, now, &mut out)?;
        }
        if let Some(enc) = &mut self.progressive {
            enc.drop_ready();
        }
        let rects = u16::try_from(out.count).map_err(|_| TilesError::TooMany(out.count))?;
        Ok(Update { rects, keyframe: out.keyframe, tick: brought == 0 })
    }

    /// Whether anything is owed that no read will bring: a stream is live, or
    /// its end is yet to be sent, or a tile is yet to be sent at the ceiling.
    pub fn owed(&self) -> bool {
        !self.live.is_empty() || !self.ended.is_empty() || !self.debts.is_empty()
    }

    /// When an [`Self::encode`] with no read is next owed something — a
    /// stream to end, a tile to send at the ceiling — or `None` while the
    /// client holds the picture settled and nothing streams. Streams [`Self::forget`] ended
    /// are owed their ends at once.
    pub fn tick_at(&self) -> Option<Duration> {
        if !self.ended.is_empty() {
            return Some(Duration::ZERO);
        }
        self.owed().then_some(self.next_tick)
    }

    /// The pixel rectangle of a tile.
    fn cell_area(&self, cell: (u16, u16)) -> Area {
        CellBox::of(cell).to_area(self.size).expect("a tile of the desktop")
    }

    /// Send `area` of the picture, a run of tiles along one row, as a still
    /// coded at `quality`: on a held list a PNG, exact; on a walked one a
    /// lossy WebP at `quality`. A whole tile of it the client's cache keeps
    /// goes as a draw of its slot instead, and the run is coded around it
    /// ([`Self::coded`]); a slot stored below the ceiling is drawn from only
    /// below it, and owes the tile the ceiling as a copy of a coarse still
    /// does. A PNG, or a WebP at the ceiling, pays the debt of every tile it
    /// covers whole; a WebP below the ceiling owes every tile it touches a
    /// still at the ceiling from now.
    fn still(&mut self, area: Area, quality: u8, now: Duration, out: &mut Out) -> Result<(), TilesError> {
        if self.progressive.is_some() {
            return self.progressive_still(area, now, out);
        }
        let settled = !self.adaptive || quality >= self.ceiling;
        let tiles: Vec<StillTile> = tiles_of(area)
            .map(|(cell, part)| {
                let whole = part.w() == TILE && part.h() == TILE && part == self.cell_area(cell);
                (cell, part, whole.then(|| tile_key(&self.mirror, self.stride, part)))
            })
            .collect();
        // Every slot drawn from is touched before any of the run is coded:
        // the stores of the pieces coded ahead of a draw take the least
        // recently used slots, which the drawn one must not be among, or the
        // client would have written it over before the draw.
        let hits: Vec<Option<(u16, bool)>> = tiles.iter().map(|(_, _, key)| key.and_then(|(hash, _)| self.cache.find(hash)).filter(|(_, kept)| *kept || !settled)).collect();
        for (slot, _) in hits.iter().flatten() {
            self.cache.touch(*slot);
        }
        let mut run = 0;
        for (i, hit) in hits.into_iter().enumerate() {
            let Some((slot, kept)) = hit else { continue };
            let (cell, part, _) = tiles[i];
            self.coded(&tiles[run..i], quality, settled, now, out)?;
            run = i + 1;
            out.header(part, TileHeader::Cached { slot });
            if kept {
                self.debts.remove(&cell);
            } else {
                self.debts.insert(cell, now);
            }
        }
        self.coded(&tiles[run..], quality, settled, now, out)
    }

    /// The progressive still of a run: each tile a first pass, a whole tile
    /// the client's cache keeps a draw of its slot, which leaves the client
    /// holding its pixels and no coefficients.
    fn progressive_still(&mut self, area: Area, now: Duration, out: &mut Out) -> Result<(), TilesError> {
        let tiles: Vec<StillTile> = tiles_of(area)
            .map(|(cell, part)| {
                let whole = part.w() == TILE && part.h() == TILE && part == self.cell_area(cell);
                (cell, part, whole.then(|| tile_key(&self.mirror, self.stride, part)))
            })
            .collect();
        for (cell, part, key) in tiles {
            if let Some((slot, kept)) = key.and_then(|(hash, _)| self.cache.find(hash)) {
                self.cache.touch(slot);
                out.header(part, TileHeader::Cached { slot });
                self.progressive.as_mut().expect("progressive").forget_tile(cell);
                if kept {
                    self.debts.remove(&cell);
                } else {
                    self.debts.insert(cell, now);
                }
                continue;
            }
            let settled = self.progressive_tile(cell, ProgressivePass::First, now, out)?;
            if let Some((hash, false)) = key {
                let slot = self.cache.store(hash, settled);
                out.header(part, TileHeader::Store { slot });
            }
        }
        Ok(())
    }

    /// Code `tiles`, a run of parts of tiles along one row with each whole
    /// tile's key, as one still at `quality`, and store every whole tile of
    /// it that is not one colour in the client's cache, after it.
    fn coded(&mut self, tiles: &[StillTile], quality: u8, settled: bool, now: Duration, out: &mut Out) -> Result<(), TilesError> {
        let (Some(first), Some(last)) = (tiles.first(), tiles.last()) else { return Ok(()) };
        let area = Area { right: last.1.right, ..first.1 };
        // A run wider than a WebP goes as pieces cut where tiles meet, so that
        // each tile is in one of them whole as it was in the run.
        if self.adaptive && area.w() > WEBP_MAX_WIDTH {
            let mut from = 0;
            while from < tiles.len() {
                let to = tiles[from..].iter().position(|(_, part, _)| part.right > tiles[from].1.left.saturating_add(WEBP_MAX_WIDTH - 1)).map_or(tiles.len(), |n| from + n.max(1));
                self.coded(&tiles[from..to], quality, settled, now, out)?;
                from = to;
            }
            return Ok(());
        }
        crop_rgb(&self.mirror, self.stride, area, &mut self.scratch);
        let from = out.out.len();
        let header = if self.adaptive { TileHeader::Webp { len: 0 } } else { TileHeader::Png { len: 0 } };
        let at = out.header(area, header);
        let coded = if self.adaptive {
            encode_webp(area.w(), area.h(), &self.scratch, quality, out.out)
        } else {
            encode_png(area.w(), area.h(), &self.scratch, palette(&self.scratch), out.out).map_err(TilesError::from)
        };
        if let Err(e) = coded {
            out.out.truncate(from);
            out.count -= 1;
            return Err(e);
        }
        out.length(from, at, header)?;
        for (cell, part, key) in tiles {
            if !settled {
                self.debts.insert(*cell, now);
            } else if area.contains(self.cell_area(*cell)) {
                self.debts.remove(cell);
            }
            if let Some((hash, false)) = key {
                let slot = self.cache.store(*hash, settled);
                out.header(*part, TileHeader::Store { slot });
            }
        }
        Ok(())
    }

    /// The regions laid out again from the churn, and every stream with
    /// pixels waiting coded.
    fn round(&mut self, now: Duration, out: &mut Out) -> Result<(), TilesError> {
        let moving = self.churn.moving(now);
        self.retune(&moving, now, out)?;
        let stride = self.stride;
        for live in &mut self.live {
            live.carried = false;
            if !live.dirty {
                continue;
            }
            let at = usize::from(live.area.top) * stride + usize::from(live.area.left) * 4;
            let from = out.out.len();
            // Whether the rectangle was begun, which a failed read never does.
            let mut headed = false;
            let header = if live.started { TileHeader::Frame { stream: live.id, len: 0 } } else { TileHeader::Start { stream: live.id, len: 0 } };

            let coded = live.stream.read_bgrx(&self.mirror[at..], stride, Some(&live.pending)).and_then(|()| {
                headed = true;
                let header_at = out.header(live.area, header);
                Ok((live.stream.encode(!live.started, out.out)?, header_at))
            });
            match coded {
                Ok((true, header_at)) => {
                    out.length(from, header_at, header)?;
                    live.pending.clear();
                    live.dirty = false;
                    live.started = true;
                    live.carried = true;
                }
                // The coder made no frame: the stream owes its whole picture
                // to the next.
                Ok((false, _)) => {
                    out.out.truncate(from);
                    out.count -= 1;
                }
                Err(e) => {
                    if headed {
                        out.out.truncate(from);
                        out.count -= 1;
                    }
                    return Err(TilesError::Region(e));
                }
            }
        }
        for live in &self.live {
            if live.carried {
                for cell in &live.cells {
                    self.debts.insert(*cell, now);
                }
            }
        }
        Ok(())
    }

    fn cover(&mut self) {
        self.covered = self.live.iter().flat_map(|live| live.cells.iter().copied()).collect();
    }

    fn retune(&mut self, moving: &[(u16, u16)], now: Duration, out: &mut Out) -> Result<(), TilesError> {
        if !self.live.is_empty() && self.retuned_at.is_some_and(|at| now.saturating_sub(at) < RETUNE) {
            return Ok(());
        }
        self.retuned_at = Some(now);
        if self.progressive.is_some() {
            return Ok(());
        }
        let worthy = components(moving);
        let busy: HashSet<(u16, u16)> = moving.iter().copied().filter(|cell| worthy.iter().any(|c| c.bbox.contains(*cell))).collect();
        for live in &mut self.live {
            if live.cells.iter().any(|cell| busy.contains(cell)) {
                live.moving_at = now;
            }
        }
        let size = self.size;
        let wanted: Vec<Area> = merge(worthy, MAX_STREAMS).into_iter().filter_map(|bbox| bbox.to_area(size)).collect();
        let (mut next, mut old): (Vec<Live>, Vec<Live>) = std::mem::take(&mut self.live).into_iter().partition(|live| {
            let mut touched = wanted.iter().filter(|area| live.area.intersect(**area).is_some());
            touched.clone().next().is_some() && touched.all(|area| live.area.contains(*area))
        });
        for live in &mut next {
            live.moving_at = now;
        }
        let mut unsent = Vec::new();
        for area in wanted {
            if next.iter().any(|live| live.area.contains(area)) {
                continue;
            }
            if next.len() < MAX_STREAMS {
                for live in old.extract_if(.., |live| live.area.intersect(area).is_some()) {
                    if live.started {
                        out.end(live.id, live.area);
                    }
                    unsent.extend(live.unsent());
                }
                let id = (0..=u8::MAX).find(|id| !next.iter().chain(&old).any(|live| live.id == *id)).expect("at most a few streams live");
                next.push(self.build(id, area, now)?);
            }
        }
        for live in old {
            let idle = now.saturating_sub(live.moving_at) >= STREAM_IDLE;
            let overlaps = next.iter().any(|kept| kept.area.intersect(live.area).is_some());
            if !idle && !overlaps && next.len() < MAX_STREAMS {
                next.push(live);
            } else {
                if live.started {
                    out.end(live.id, live.area);
                }
                unsent.extend(live.unsent());
            }
        }
        self.live = next;
        self.cover();
        self.owe(&unsent);
        Ok(())
    }

    /// What streams that ended did not carry, owed as stills where no stream
    /// live from here does, as runs along a row.
    fn owe(&mut self, unsent: &[Area]) {
        for area in unsent {
            let mut runs: Vec<Area> = Vec::new();
            for (_, part) in tiles_of(*area).filter(|(cell, _)| !self.covered.contains(cell)) {
                match runs.last_mut() {
                    Some(run) if run.top == part.top && run.right.checked_add(1) == Some(part.left) => run.right = part.right,
                    _ => runs.push(part),
                }
            }
            self.stills.extend(runs);
        }
    }

    /// A stream of the region coder over `area`, every tile of it owed from now.
    fn build(&mut self, id: u8, area: Area, now: Duration) -> Result<Live, TilesError> {
        let stream = self.coder.open(area.w(), area.h(), self.quality).map_err(TilesError::Region)?;
        let cells: Vec<(u16, u16)> = tiles_of(area).map(|(cell, _)| cell).collect();
        for cell in &cells {
            self.debts.insert(*cell, now);
        }
        Ok(Live { id, area, cells, stream, pending: Vec::new(), dirty: true, started: false, carried: false, moving_at: now })
    }

    /// The streams whose motion stopped ended, and the tiles still for
    /// [`CLEANUP_IDLE`] sent at the ceiling as runs along a row, the oldest
    /// runs first until [`SETTLE_BYTES`] are sent, of the [`SETTLE_TILES`]
    /// longest owed, or [`SETTLE_TILES_BELOW`] while the walk is below the
    /// ceiling — none while the walk is
    /// coarse, since the link is behind on what it has, until the desktop has
    /// been still for [`COARSE_QUIET`].
    fn tick(&mut self, now: Duration, out: &mut Out) -> Result<(), TilesError> {
        let mut unsent = Vec::new();
        for live in self.live.extract_if(.., |live| now.saturating_sub(live.moving_at) >= STREAM_IDLE) {
            if live.started {
                out.end(live.id, live.area);
            }
            unsent.extend(live.unsent());
        }
        self.cover();
        self.owe(&unsent);
        // Measured: `WLSHARE_TILES_PROGRESSIVE_REFINE=busy` refines progressive
        // tiles on a coarse walk too, a tick's 40 at a time, without waiting
        // for the desktop to be quiet.
        let busy = self.progressive.is_some() && std::env::var("WLSHARE_TILES_PROGRESSIVE_REFINE").as_deref() == Ok("busy");
        if self.coarse && !busy && now.saturating_sub(self.changed_at) < COARSE_QUIET {
            return Ok(());
        }

        let mut ready: Vec<((u16, u16), Duration)> = self.debts.iter().filter(|(cell, at)| !self.covered.contains(*cell) && now.saturating_sub(**at) >= CLEANUP_IDLE).map(|(cell, at)| (*cell, *at)).collect();
        ready.sort_unstable_by_key(|((col, row), at)| (*at, *row, *col));
        ready.truncate(if self.quality < self.ceiling { SETTLE_TILES_BELOW } else { SETTLE_TILES });
        ready.sort_unstable_by_key(|((col, row), _)| (*row, *col));
        // Each run with the time its oldest tile was owed from, and sent
        // oldest first.
        let mut runs: Vec<(CellBox, Duration)> = Vec::new();
        for ((col, row), at) in ready {
            match runs.last_mut() {
                Some((run, oldest)) if run.r0 == row && run.c1.checked_add(1) == Some(col) => {
                    run.c1 = col;
                    *oldest = (*oldest).min(at);
                }
                _ => runs.push((CellBox::of((col, row)), at)),
            }
        }
        runs.sort_by_key(|(run, oldest)| (*oldest, run.r0, run.c0));
        let from = out.out.len();
        for (run, _) in runs {
            if out.out.len() - from >= SETTLE_BYTES {
                break;
            }
            if let Some(area) = run.to_area(self.size) {
                if self.progressive.is_some() {
                    for (cell, _) in tiles_of(area) {
                        self.progressive_tile(cell, ProgressivePass::Upgrade, now, out)?;
                    }
                } else {
                    self.still(area, self.ceiling, now, out)?;
                }
            }
        }
        Ok(())
    }
}

/// The encoder read back as a client would: every rectangle parsed by the
/// client's parser, each PNG by a reader written here from the PNG
/// specification over `flate2`'s inflate, which shares nothing with the `png`
/// crate's, each WebP by `image-webp`'s decoder, pure Rust that shares
/// nothing with the libwebp linked, and each region by reading back the
/// frames of a coder standing in for the daemon's H.264 one, whose own tests
/// decode it with libavcodec.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rfb::client::{self, RectBody, ServerMsg, TileBody};
    use std::io::Read as _;

    /// A PNG as packed RGB, by the specification: the chunks, the IDAT
    /// inflated, each row's filter undone, a palette looked up.
    fn read_png(png: &[u8]) -> (u32, u32, Vec<u8>) {
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "the signature");
        let (mut at, mut ihdr, mut palette, mut idat) = (8, None, Vec::new(), Vec::new());
        loop {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let (kind, data) = (&png[at + 4..at + 8], &png[at + 8..at + 8 + len]);
            match kind {
                b"IHDR" => ihdr = Some((u32::from_be_bytes(data[0..4].try_into().unwrap()), u32::from_be_bytes(data[4..8].try_into().unwrap()), data[8], data[9], data[12])),
                b"PLTE" => palette = data.to_vec(),
                b"IDAT" => idat.extend_from_slice(data),
                b"IEND" => break,
                _ => {}
            }
            at += 12 + len;
        }
        let (w, h, depth, colour, interlace) = ihdr.expect("an IHDR");
        assert_eq!((depth, interlace), (8, 0), "8-bit, not interlaced");
        let bpp = match colour {
            2 => 3,
            3 => 1,
            other => panic!("colour type {other}"),
        };
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&idat[..]).read_to_end(&mut raw).unwrap();
        let line = w as usize * bpp;
        assert_eq!(raw.len(), (line + 1) * h as usize, "every row and its filter");
        let mut rows: Vec<u8> = Vec::with_capacity(line * h as usize);
        for y in 0..h as usize {
            let filter = raw[y * (line + 1)];
            for x in 0..line {
                let byte = raw[y * (line + 1) + 1 + x];
                let a = if x >= bpp { rows[y * line + x - bpp] } else { 0 };
                let b = if y > 0 { rows[(y - 1) * line + x] } else { 0 };
                let c = if x >= bpp && y > 0 { rows[(y - 1) * line + x - bpp] } else { 0 };
                let predicted = match filter {
                    0 => 0,
                    1 => a,
                    2 => b,
                    3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                    4 => {
                        let p = i16::from(a) + i16::from(b) - i16::from(c);
                        let (pa, pb, pc) = ((p - i16::from(a)).abs(), (p - i16::from(b)).abs(), (p - i16::from(c)).abs());
                        if pa <= pb && pa <= pc {
                            a
                        } else if pb <= pc {
                            b
                        } else {
                            c
                        }
                    }
                    other => panic!("filter {other}"),
                };
                rows.push(byte.wrapping_add(predicted));
            }
        }
        let rgb = if colour == 3 { rows.iter().flat_map(|&i| palette[usize::from(i) * 3..][..3].to_vec()).collect() } else { rows };
        (w, h, rgb)
    }

    /// A rectangle as the parser reads one: x, y, width, height.
    type Place = (u16, u16, u16, u16);

    /// What a client draws of the rectangles, and what it was sent.
    struct Page {
        w: usize,
        canvas: Vec<u8>,
        /// The live streams' rectangles.
        streams: HashMap<u8, Place>,
        /// The cache of tiles, each slot a tile's `B, G, R, X` rows.
        slots: HashMap<u16, Vec<u8>>,
        pngs: usize,
        webps: usize,
        starts: usize,
        frames: usize,
        ends: usize,
        copies: usize,
        cached: usize,
        stored: usize,
        bytes: usize,
    }

    impl Page {
        fn new(w: u16, h: u16) -> Self {
            Self { w: usize::from(w), canvas: vec![0; usize::from(w) * usize::from(h) * 4], streams: HashMap::new(), slots: HashMap::new(), pngs: 0, webps: 0, starts: 0, frames: 0, ends: 0, copies: 0, cached: 0, stored: 0, bytes: 0 }
        }

        /// Packed RGB drawn at the rectangle.
        fn paste(&mut self, (x, y, w, h): Place, rgb: &[u8]) {
            for row in 0..usize::from(h) {
                for col in 0..usize::from(w) {
                    let at = ((usize::from(y) + row) * self.w + usize::from(x) + col) * 4;
                    let from = (row * usize::from(w) + col) * 3;
                    self.canvas[at..at + 3].copy_from_slice(&[rgb[from + 2], rgb[from + 1], rgb[from]]);
                }
            }
        }

        /// One update of `count` rectangles, as the encoder appended them.
        fn draw(&mut self, count: u16, rects: &[u8]) {
            let mut wire = crate::rfb::msg::update_header(count).to_vec();
            wire.extend_from_slice(rects);
            self.bytes += rects.len();
            let (ServerMsg::Update(rects), used) = client::parse(&wire).unwrap().expect("a whole update") else { panic!("an update") };
            assert_eq!(used, wire.len(), "the update is its rectangles");
            for rect in rects {
                let RectBody::Tiles(body) = rect.body else { panic!("a tiles rectangle") };
                let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
                match body {
                    TileBody::Png(png) => {
                        self.pngs += 1;
                        let (pw, ph, rgb) = read_png(&png);
                        assert_eq!((pw, ph), (u32::from(w), u32::from(h)), "a PNG of the rectangle");
                        self.paste((x, y, w, h), &rgb);
                    }
                    TileBody::Webp(webp) => {
                        self.webps += 1;
                        let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(&webp)).expect("a WebP");
                        assert_eq!(decoder.dimensions(), (u32::from(w), u32::from(h)), "a WebP of the rectangle");
                        assert!(!decoder.has_alpha() && decoder.is_lossy(), "lossy, 4:2:0, no alpha");
                        let mut rgb = vec![0; decoder.output_buffer_size().unwrap()];
                        decoder.read_image(&mut rgb).unwrap();
                        self.paste((x, y, w, h), &rgb);
                    }
                    TileBody::Start { stream, frame } => {
                        self.starts += 1;
                        assert!(!self.streams.contains_key(&stream), "stream {stream} started while live");
                        assert_eq!(frame[0], Fake::KEYFRAME, "a stream starts at its keyframe");
                        self.streams.insert(stream, (x, y, w, h));
                        self.region(stream, &frame);
                    }
                    TileBody::Frame { stream, frame } => {
                        self.frames += 1;
                        assert_eq!(self.streams.get(&stream), Some(&(x, y, w, h)), "a frame of a stream live over its rectangle");
                        assert_eq!(frame[0], Fake::FRAME, "a keyframe after the start");
                        self.region(stream, &frame);
                    }
                    TileBody::End { stream } => {
                        self.ends += 1;
                        assert_eq!(self.streams.remove(&stream), Some((x, y, w, h)), "the end of a stream live over its rectangle");
                    }
                    TileBody::Copy { x: from_x, y: from_y } => {
                        self.copies += 1;
                        let line = usize::from(w) * 4;
                        assert!(usize::from(from_x) * 4 + line <= self.w * 4 && (usize::from(from_y) + usize::from(h)) * self.w * 4 <= self.canvas.len(), "a copy from inside the desktop");
                        // Copied out first, so a source the rectangle overlaps is read whole.
                        let source: Vec<u8> = (0..usize::from(h)).flat_map(|row| {
                            let at = ((usize::from(from_y) + row) * self.w + usize::from(from_x)) * 4;
                            self.canvas[at..at + line].to_vec()
                        }).collect();
                        for (row, pixels) in source.chunks_exact(line).enumerate() {
                            let at = ((usize::from(y) + row) * self.w + usize::from(x)) * 4;
                            self.canvas[at..at + line].copy_from_slice(pixels);
                        }
                    }
                    TileBody::Store { slot } => {
                        self.stored += 1;
                        assert!((w, h) == (TILE, TILE) && x % TILE == 0 && y % TILE == 0, "a store of one tile on the grid, not {w}x{h}+{x}+{y}");
                        let line = usize::from(TILE) * 4;
                        let tile: Vec<u8> = (0..usize::from(TILE)).flat_map(|row| self.canvas[((usize::from(y) + row) * self.w + usize::from(x)) * 4..][..line].to_vec()).collect();
                        self.slots.insert(slot, tile);
                    }
                    TileBody::Cached { slot } => {
                        self.cached += 1;
                        assert_eq!((w, h), (TILE, TILE), "a cached draw of one tile");
                        let tile = self.slots.get(&slot).unwrap_or_else(|| panic!("slot {slot} drawn from before it was stored"));
                        let line = usize::from(TILE) * 4;
                        for (row, pixels) in tile.chunks_exact(line).enumerate() {
                            let at = ((usize::from(y) + row) * self.w + usize::from(x)) * 4;
                            self.canvas[at..at + line].copy_from_slice(pixels);
                        }
                    }
                }
            }
        }

        fn region(&mut self, stream: u8, frame: &[u8]) {
            let (x, y, w, h) = self.streams[&stream];
            let line = usize::from(w) * 4;
            assert_eq!(frame.len(), 1 + line * usize::from(h), "a fake frame is the region's pixels");
            for (row, pixels) in frame[1..].chunks_exact(line).enumerate() {
                let at = ((usize::from(y) + row) * self.w + usize::from(x)) * 4;
                self.canvas[at..at + line].copy_from_slice(pixels);
            }
        }

        /// How many pixels differ from `pixels` in B, G or R.
        fn off(&self, pixels: &[u8]) -> usize {
            self.canvas.as_chunks::<4>().0.iter().zip(pixels.as_chunks::<4>().0).filter(|(a, b)| a[..3] != b[..3]).count()
        }

        /// How far the page is from `pixels`: the sum over every sample of B,
        /// G and R of how much it differs.
        fn distance(&self, pixels: &[u8]) -> u64 {
            self.canvas.as_chunks::<4>().0.iter().zip(pixels.as_chunks::<4>().0).flat_map(|(a, b)| a[..3].iter().zip(&b[..3])).map(|(a, b)| u64::from(a.abs_diff(*b))).sum()
        }
    }

    /// A busy picture no palette holds: every pixel its own.
    fn noise(w: u16, h: u16, seed: u32) -> Vec<u8> {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..usize::from(w) * usize::from(h))
            .flat_map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let [b, g, r, _] = state.to_le_bytes();
                [b, g, r, 0]
            })
            .collect()
    }

    /// Paint `rect` of a `w`-wide picture with frame `n` of something moving.
    fn paint(pixels: &mut [u8], w: u16, rect: Rect, n: u32) {
        let fill = noise(rect.width, rect.height, n);
        for row in 0..usize::from(rect.height) {
            let at = ((usize::from(rect.y) + row) * usize::from(w) + usize::from(rect.x)) * 4;
            pixels[at..at + usize::from(rect.width) * 4].copy_from_slice(&fill[row * usize::from(rect.width) * 4..][..usize::from(rect.width) * 4]);
        }
    }

    /// Paint `rect` of a `w`-wide picture as a page of black glyphs on white,
    /// lines of dots and gaps: text, two colours with sharp edges.
    fn glyphs(pixels: &mut [u8], w: u16, rect: Rect) {
        for row in 0..usize::from(rect.height) {
            for col in 0..usize::from(rect.width) {
                let at = ((usize::from(rect.y) + row) * usize::from(w) + usize::from(rect.x) + col) * 4;
                let v = if (col * 7 + row * 13) % 11 < 2 && row % 12 < 9 { 0 } else { 255 };
                pixels[at..at + 4].copy_from_slice(&[v, v, v, 0]);
            }
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A list that holds the dial at `quality`.
    fn held(quality: u8) -> TilesPicture {
        TilesPicture { quality, adaptive: false }
    }

    /// A list that lets the dial walk under `quality`.
    fn walked(quality: u8) -> TilesPicture {
        TilesPicture { quality, adaptive: true }
    }

    /// A region coder standing in for the daemon's H.264 one: its frames are
    /// the region's pixels, each sample's lowest bit dropped, so a stream's
    /// picture is not exact, behind a byte that says keyframe or not. It can
    /// be told to refuse every stream, or to fail one after so many frames, at
    /// its read or at its encode.
    struct Fake {
        refuse: bool,
        fail_after: Option<usize>,
        fail_read_after: Option<usize>,
    }

    impl Fake {
        const KEYFRAME: u8 = 0xC0;
        const FRAME: u8 = 0xDE;

        fn coder() -> Box<dyn RegionCoder> {
            Box::new(Self { refuse: false, fail_after: None, fail_read_after: None })
        }
    }

    struct FakeStream {
        w: usize,
        h: usize,
        picture: Vec<u8>,
        filled: bool,
        frames: usize,
        fail_after: Option<usize>,
        fail_read_after: Option<usize>,
    }

    impl RegionCoder for Fake {
        fn open(&mut self, width: u16, height: u16, _quality: u8) -> Result<Box<dyn RegionStream>, RegionError> {
            if self.refuse {
                return Err("the device is gone".into());
            }
            let (w, h) = (usize::from(width), usize::from(height));
            Ok(Box::new(FakeStream { w, h, picture: vec![0; w * h * 4], filled: false, frames: 0, fail_after: self.fail_after, fail_read_after: self.fail_read_after }))
        }
    }

    impl RegionStream for FakeStream {
        fn set_quality(&mut self, _quality: u8) {}

        fn read_bgrx(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> Result<(), RegionError> {
            if self.fail_read_after.is_some_and(|after| self.frames >= after) {
                return Err("the upload failed".into());
            }
            let whole = [Rect { x: 0, y: 0, width: self.w as u16, height: self.h as u16 }];
            let rects = match changed {
                Some(rects) if self.filled => rects,
                _ => &whole,
            };
            for r in rects {
                for row in 0..usize::from(r.height) {
                    let (to, from) = (((usize::from(r.y) + row) * self.w + usize::from(r.x)) * 4, (usize::from(r.y) + row) * stride + usize::from(r.x) * 4);
                    self.picture[to..to + usize::from(r.width) * 4].copy_from_slice(&pixels[from..from + usize::from(r.width) * 4]);
                }
            }
            self.filled = true;
            Ok(())
        }

        fn encode(&mut self, keyframe: bool, out: &mut Vec<u8>) -> Result<bool, RegionError> {
            if self.fail_after.is_some_and(|after| self.frames >= after) {
                return Err("the encoder hung".into());
            }
            out.push(if keyframe || self.frames == 0 { Fake::KEYFRAME } else { Fake::FRAME });
            out.extend(self.picture.iter().map(|sample| sample & !1));
            self.frames += 1;
            Ok(true)
        }
    }

    /// Read and encode, and draw what came out.
    fn step(encoder: &mut TilesEncoder, page: &mut Page, pixels: &[u8], changed: Option<&[Rect]>, now: Duration) -> u16 {
        update(encoder, page, pixels, changed, now).rects
    }

    /// Read and encode, and draw what came out, and say what the update was.
    fn update(encoder: &mut TilesEncoder, page: &mut Page, pixels: &[u8], changed: Option<&[Rect]>, now: Duration) -> Update {
        let stride = usize::from(encoder.size().0) * 4;
        encoder.read(pixels, stride, changed, now).unwrap();
        let mut out = Vec::new();
        let update = encoder.encode(now, &mut out).unwrap();
        page.draw(update.rects, &out);
        update
    }

    /// Run the tick from `now` until nothing is owed, drawing what it sends;
    /// returns when it stopped.
    fn settle(encoder: &mut TilesEncoder, page: &mut Page, mut now: Duration) -> Duration {
        let mut out = Vec::new();
        while let Some(at) = encoder.tick_at() {
            now = now.max(at);
            out.clear();
            let update = encoder.encode(now, &mut out).unwrap();
            page.draw(update.rects, &out);
            assert!(now < ms(60_000), "never settled");
        }
        now
    }

    /// A first picture goes whole as PNG, a band at a time, exact; a palette
    /// where a band has few colours and RGB where it has many, both read back.
    #[test]
    fn a_first_picture_is_sent_exact_in_bands() {
        let (w, h) = (200u16, 150u16);
        let mut pixels = noise(w, h, 1);
        // The last band, rows 128–149, one flat colour, which a palette holds;
        // the two above it noise, which none does.
        for row in 128..usize::from(h) {
            pixels[row * usize::from(w) * 4..][..usize::from(w) * 4].copy_from_slice(&[10, 20, 30, 0].repeat(usize::from(w)));
        }
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let first = update(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 5, y: 5, width: 1, height: 1 }]), ms(0));
        assert_eq!((first.rects, page.pngs, page.stored), (9, 3, 6), "the whole picture in three bands, whatever the read was told, and its six whole tiles stored");
        assert!(first.keyframe, "a picture sent whole crosses the link as a keyframe");
        assert_eq!(page.off(&pixels), 0);
        assert!(encoder.tick_at().is_none(), "a picture sent exact owes nothing");
        paint(&mut pixels, w, Rect { x: 70, y: 70, width: 10, height: 10 }, 9);
        let next = update(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 70, y: 70, width: 10, height: 10 }]), ms(100));
        assert!(next.rects == 1 && !next.keyframe, "a tile that changed is no keyframe: {next:?}");
    }

    /// On a walked list every still is a WebP, at the ceiling too, text and
    /// flat panels included, and one at the ceiling owes nothing.
    #[test]
    fn a_walked_still_is_a_webp() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 11);
        let mut encoder = TilesEncoder::new(w, h, walked(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let first = update(&mut encoder, &mut page, &pixels, None, ms(0));
        assert!(first.keyframe);
        assert_eq!((page.pngs, page.webps), (0, 2), "a busy picture goes lossy, a row's run at a time, at the ceiling");
        assert!(page.distance(&pixels) > 0, "and the page holds it lossy");
        assert!(!encoder.owed(), "a still at the ceiling is owed nothing");
        glyphs(&mut pixels, w, Rect { x: 64, y: 0, width: 64, height: 64 });
        for line in 64..128 {
            pixels[line * usize::from(w) * 4..][..64 * 4].copy_from_slice(&[7, 8, 9, 0].repeat(64));
        }
        let text = Rect { x: 64, y: 0, width: 64, height: 64 };
        let flat = Rect { x: 0, y: 64, width: 64, height: 64 };
        step(&mut encoder, &mut page, &pixels, Some(&[text, flat]), ms(100));
        assert_eq!((page.pngs, page.webps), (0, 4), "text and a flat tile go lossy too");
        assert!(!encoder.owed());
    }

    /// A walked run wider than a WebP is sent as pieces of whole tiles, each
    /// a WebP libwebp codes, and owes nothing at the ceiling.
    #[test]
    fn a_walked_run_wider_than_a_webp_goes_in_pieces() {
        let (w, h) = (16_448u16, 64u16);
        let pixels = noise(w, h, 13);
        let mut encoder = TilesEncoder::new(w, h, walked(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        assert_eq!((page.pngs, page.webps), (0, 2), "16,320 pixels, 255 tiles, and the 128 after");
        assert!(page.distance(&pixels) > 0);
        assert!(!encoder.owed());
    }

    /// Below the ceiling a walked still is owed again at the ceiling once
    /// still; coarse, the debt waits until the desktop has been quiet. At the
    /// ceiling again, a still owes nothing.
    #[test]
    fn below_the_ceiling_a_still_is_owed_again_at_the_ceiling() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 12);
        let mut encoder = TilesEncoder::new(w, h, walked(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        assert_eq!(page.webps, 2);

        encoder.set_pace(30, false);
        let busy = Rect { x: 64, y: 0, width: 64, height: 64 };
        paint(&mut pixels, w, busy, 13);
        let lossy = update(&mut encoder, &mut page, &pixels, Some(&[busy]), ms(100));
        assert_eq!((lossy.rects, lossy.keyframe, page.webps, page.stored), (2, false, 3, 9), "one tile that changed, at the walk's dial, no keyframe, and stored");
        assert!(encoder.owed() && encoder.tick_at().is_some(), "a still below the ceiling is owed");
        let coarse = page.distance(&pixels);
        settle(&mut encoder, &mut page, ms(100));
        assert_eq!((page.pngs, page.webps), (0, 4), "sent again, at the ceiling, and nothing else");
        assert!(page.distance(&pixels) < coarse, "finer at the ceiling: {} from {coarse}", page.distance(&pixels));
        assert!(!encoder.owed());

        // Coarse: the debt waits.
        encoder.set_pace(20, true);
        paint(&mut pixels, w, busy, 14);
        step(&mut encoder, &mut page, &pixels, Some(&[busy]), ms(2000));
        assert_eq!(page.webps, 5);
        let mut out = Vec::new();
        for at in [3000, 3750] {
            assert_eq!(encoder.encode(ms(at), &mut out).unwrap().rects, 0, "nothing is paid while coarse and lately changed");
        }
        assert!(encoder.owed(), "the debt stands");
        // Quiet long enough, it is paid though the walk is still coarse: a
        // still desktop tells the walk nothing that would lift it.
        let paid_at = settle(&mut encoder, &mut page, ms(3750));
        assert!(paid_at >= ms(2000) + COARSE_QUIET, "paid at {paid_at:?}");
        assert_eq!(page.webps, 6, "paid at the ceiling while coarse, once quiet");
        encoder.set_pace(80, false);

        paint(&mut pixels, w, busy, 15);
        step(&mut encoder, &mut page, &pixels, Some(&[busy]), ms(5000));
        assert_eq!(page.webps, 7);
        assert!(!encoder.owed(), "at the ceiling a still owes nothing");
    }

    /// A tick sends the oldest runs owed: below the ceiling
    /// [`SETTLE_TILES_BELOW`] tiles, at it until [`SETTLE_BYTES`] are sent.
    #[test]
    fn a_tick_sends_the_oldest_until_its_bytes_are_spent() {
        let (w, h) = (512u16, 1024u16);
        let mut pixels = noise(w, h, 16);
        let mut encoder = TilesEncoder::new(w, h, walked(90), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        encoder.set_pace(20, false);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let row = Rect { x: 0, y: 0, width: w, height: 64 };
        paint(&mut pixels, w, row, 17);
        step(&mut encoder, &mut page, &pixels, Some(&[row]), ms(300));
        assert_eq!(encoder.debts.len(), 128, "every tile owed, the top row from later");
        let top = |encoder: &TilesEncoder| (0..8).all(|col| encoder.debts.contains_key(&(col, 0)));

        let mut out = Vec::new();
        let at = encoder.tick_at().unwrap();
        assert!(at >= CLEANUP_IDLE, "the first owed once still: {at:?}");
        encoder.encode(at, &mut out).unwrap();
        assert_eq!(encoder.debts.len(), 128 - SETTLE_TILES_BELOW, "below the ceiling, {SETTLE_TILES_BELOW} tiles");
        assert!(top(&encoder), "the oldest first");

        encoder.set_pace(90, false);
        let before = encoder.debts.len();
        out.clear();
        let at = encoder.tick_at().unwrap();
        let tick = encoder.encode(at, &mut out).unwrap();
        page.draw(tick.rects, &out);
        let paid = before - encoder.debts.len();
        assert!(paid > SETTLE_TILES_BELOW && encoder.owed(), "at the ceiling more, and not all: {paid} of {before}");
        // Each run is a row of 8 tiles, and the last one is sent only while
        // what came before it is under the bytes.
        let run = out.len() / (paid / 8);
        assert!(out.len() >= SETTLE_BYTES && out.len() < SETTLE_BYTES + 2 * run, "{} bytes, runs of {run}", out.len());
        assert!(top(&encoder), "the oldest first");
        settle(&mut encoder, &mut page, at);
        assert!(!encoder.owed());
    }

    /// The damage is compared with what the client holds: a redraw of the same
    /// pixels sends nothing, and one tile that changed is one PNG of it.
    #[test]
    fn only_tiles_that_changed_are_sent() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 2);
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let whole = [Rect { x: 0, y: 0, width: w, height: h }];
        assert_eq!(step(&mut encoder, &mut page, &pixels, Some(&whole), ms(100)), 0, "a redraw of what the client holds");
        paint(&mut pixels, w, Rect { x: 70, y: 70, width: 10, height: 10 }, 9);
        let bytes = page.bytes;
        assert_eq!(step(&mut encoder, &mut page, &pixels, Some(&whole), ms(200)), 2, "one tile changed: its PNG and its store");
        assert!(page.bytes - bytes < 64 * 64 * 3 + 200, "more than one tile was sent");
        assert_eq!(page.off(&pixels), 0);
        // The framebuffer's rows may be farther apart than the picture's.
        let stride = usize::from(w) * 4 + 64;
        let mut wide = vec![0xEE; stride * usize::from(h)];
        for row in 0..usize::from(h) {
            wide[row * stride..][..usize::from(w) * 4].copy_from_slice(&pixels[row * usize::from(w) * 4..][..usize::from(w) * 4]);
        }
        encoder.read(&wide, stride, Some(&whole), ms(300)).unwrap();
        assert_eq!(encoder.encode(ms(300), &mut Vec::new()).unwrap().rects, 0, "the same picture at another stride");
        assert!(matches!(encoder.read(&wide[..100], stride, None, ms(400)), Err(TilesError::Buffer { .. })));
    }

    /// What moves becomes a stream of its own, told only its damage, which
    /// ends once the motion stops, and every tile it carried is sent exact
    /// after its end: the picture ends exact, and owes nothing.
    #[test]
    fn what_moves_streams_and_settles_exact() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = noise(w, h, 3);
        let mut encoder = TilesEncoder::new(w, h, held(60), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let pngs = page.pngs;
        // A 192×128 video, every 50 ms for two seconds.
        let video = Rect { x: 128, y: 64, width: 192, height: 128 };
        for n in 1..=40u32 {
            paint(&mut pixels, w, video, 100 + n);
            step(&mut encoder, &mut page, &pixels, Some(&[video]), ms(u64::from(n) * 50));
        }
        assert_eq!((page.starts, page.ends), (1, 0), "one stream, live");
        assert!(page.frames > 20, "{} frames of it", page.frames);
        assert!(page.pngs - pngs <= 16, "{} PNGs while it moved", page.pngs - pngs);
        assert!(encoder.owed() && encoder.tick_at().is_some(), "a live stream owes its end");
        assert!(page.off(&pixels) > 0, "a stream's picture is not exact");
        settle(&mut encoder, &mut page, ms(2000));
        assert_eq!(page.ends, 1, "the stream ended");
        assert_eq!(page.off(&pixels), 0, "the picture settled exact");
        assert!(page.streams.is_empty());
    }

    /// A stream that ends with damage no frame of it carried leaves that
    /// damage as stills in the same update, not the stream's last frame from
    /// before it.
    #[test]
    fn a_stream_that_ends_sends_what_it_did_not_carry() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = noise(w, h, 5);
        let mut encoder = TilesEncoder::new(w, h, held(60), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let video = Rect { x: 128, y: 64, width: 192, height: 128 };
        for n in 1..=40u32 {
            paint(&mut pixels, w, video, 300 + n);
            step(&mut encoder, &mut page, &pixels, Some(&[video]), ms(u64::from(n) * 50));
        }
        assert_eq!((page.starts, page.ends), (1, 0), "one stream, live");
        // Its motion last seen at 1.8 s. A tile of it redrawn twice once that
        // stopped, which is no motion but is carried, its tiles owed the
        // ceiling from then; then one change under it in the round that ends
        // it, idle, before any frame of it carries the change, and before its
        // tiles are owed their stills.
        let now_and_then = Rect { x: 128, y: 64, width: 64, height: 64 };
        for (n, at) in [(1, 2600), (2, 2750)] {
            paint(&mut pixels, w, now_and_then, 900 + n);
            step(&mut encoder, &mut page, &pixels, Some(&[now_and_then]), ms(at));
        }
        assert_eq!(page.ends, 0, "the stream still live");
        let change = Rect { x: 256, y: 128, width: 64, height: 64 };
        paint(&mut pixels, w, change, 999);
        step(&mut encoder, &mut page, &pixels, Some(&[change]), ms(3100));
        assert_eq!(page.ends, 1, "the stream ended");
        let row = |y: usize| (y * usize::from(w) + usize::from(change.x)) * 4..(y * usize::from(w) + usize::from(change.x + change.width)) * 4;
        let held = (usize::from(change.y)..usize::from(change.y + change.height)).all(|y| page.canvas[row(y)].as_chunks::<4>().0.iter().zip(pixels[row(y)].as_chunks::<4>().0).all(|(a, b)| a[..3] == b[..3]));
        assert!(held, "the change is held exact, not the stream's last frame");
    }

    /// A stream's START is no keyframe, nor its restart at a moved dial: a
    /// region's IDR is the load a desktop that moves keeps sending, which the
    /// walk is there to judge, where the picture whole again is a burst once.
    #[test]
    fn a_stream_that_starts_or_restarts_is_no_keyframe() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = noise(w, h, 7);
        let mut encoder = TilesEncoder::new(w, h, walked(60), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let video = Rect { x: 0, y: 0, width: 192, height: 128 };
        for n in 1..=12u32 {
            paint(&mut pixels, w, video, 500 + n);
            let made = page.starts;
            let one = update(&mut encoder, &mut page, &pixels, Some(&[video]), ms(u64::from(n) * 50));
            assert!(!one.keyframe, "frame {n}, a START among it or not: {one:?}");
            if page.starts > made {
                assert!(one.rects > 0 && !one.tick, "the START is what the read brought: {one:?}");
            }
        }
        assert_eq!((page.starts, page.ends), (1, 0), "a stream, live");
        encoder.set_pace(40, false);
        paint(&mut pixels, w, video, 600);
        let restarted = update(&mut encoder, &mut page, &pixels, Some(&[video]), ms(700));
        assert!(restarted.rects > 0 && !restarted.keyframe, "the stream's restart: {restarted:?}");
    }

    /// An update is the tick's when all it sends is what the desktop
    /// stopping owes — a still sent again at the ceiling, a stream's end —
    /// with a read before it that changed nothing or none, and not once a
    /// read brought anything; the session hands the walk no verdict of it.
    #[test]
    fn an_update_of_the_tick_alone_says_so() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 21);
        let mut encoder = TilesEncoder::new(w, h, walked(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let first = update(&mut encoder, &mut page, &pixels, None, ms(0));
        assert!(first.keyframe && !first.tick, "{first:?}");
        encoder.set_pace(30, false);
        let busy = Rect { x: 64, y: 0, width: 64, height: 64 };
        paint(&mut pixels, w, busy, 22);
        let lossy = update(&mut encoder, &mut page, &pixels, Some(&[busy]), ms(100));
        assert!(lossy.rects == 2 && !lossy.tick, "what the read brought, and its store: {lossy:?}");
        // The read the session makes with nothing changed, as it does at a
        // tick, until the debt is old enough to be paid.
        let paid = loop {
            let at = encoder.tick_at().expect("a still below the ceiling is owed");
            let paid = update(&mut encoder, &mut page, &pixels, Some(&[]), at);
            if paid.rects > 0 {
                break paid;
            }
            assert!(paid.tick);
        };
        assert!(paid.rects > 0 && paid.tick, "the debt, at the ceiling: {paid:?}");
        assert!(!encoder.owed());

        // A change read at the tick's time is the read's, debts and all.
        encoder.set_pace(30, false);
        paint(&mut pixels, w, busy, 23);
        update(&mut encoder, &mut page, &pixels, Some(&[busy]), ms(1000));
        let at = encoder.tick_at().expect("owed again");
        let other = Rect { x: 192, y: 64, width: 64, height: 64 };
        paint(&mut pixels, w, other, 24);
        let both = update(&mut encoder, &mut page, &pixels, Some(&[other]), at);
        assert!(both.rects > 0 && !both.tick, "a change and the tick: {both:?}");
    }

    /// On a walked list what a stream carried settles at the ceiling, a
    /// picture as WebP, and then nothing is owed.
    #[test]
    fn a_walked_stream_settles_at_the_ceiling() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = noise(w, h, 4);
        let mut encoder = TilesEncoder::new(w, h, walked(60), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let video = Rect { x: 128, y: 64, width: 192, height: 128 };
        for n in 1..=40u32 {
            paint(&mut pixels, w, video, 200 + n);
            step(&mut encoder, &mut page, &pixels, Some(&[video]), ms(u64::from(n) * 50));
        }
        assert_eq!((page.starts, page.ends), (1, 0), "one stream, live");
        let webps = page.webps;
        settle(&mut encoder, &mut page, ms(2000));
        assert_eq!(page.ends, 1, "the stream ended");
        assert!(page.webps > webps, "its tiles sent again as stills");
        assert!(page.streams.is_empty() && !encoder.owed());
    }

    /// A new ceiling ends every live stream before the picture starts over.
    #[test]
    fn a_new_ceiling_ends_the_live_streams() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 31);
        let mut encoder = TilesEncoder::new(w, h, held(60), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let video = Rect { x: 0, y: 0, width: 256, height: 192 };
        for n in 1..=12u32 {
            paint(&mut pixels, w, video, 700 + n);
            step(&mut encoder, &mut page, &pixels, Some(&[video]), ms(u64::from(n) * 50));
        }
        assert_eq!((page.starts, page.ends), (1, 0), "a stream, live");
        encoder.set_picture(held(80));
        let whole = update(&mut encoder, &mut page, &pixels, Some(&[video]), ms(700));
        assert!(whole.keyframe, "the picture starts over: {whole:?}");
        assert_eq!(page.ends, 1, "the live stream ended");
        assert!(page.streams.is_empty());
        assert_eq!(page.off(&pixels), 0, "sent whole, exact at the new ceiling");
    }

    /// A redraw of what a live stream carries, the pixels the client holds,
    /// codes no frame of it.
    #[test]
    fn a_redraw_inside_a_stream_codes_nothing() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 5);
        let mut encoder = TilesEncoder::new(w, h, held(60), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let video = Rect { x: 0, y: 0, width: 256, height: 192 };
        for n in 1..=12u32 {
            paint(&mut pixels, w, video, 300 + n);
            step(&mut encoder, &mut page, &pixels, Some(&[video]), ms(u64::from(n) * 50));
        }
        assert_eq!((page.starts, page.ends), (1, 0), "a stream, live");
        let frames = page.frames;
        assert_eq!(step(&mut encoder, &mut page, &pixels, Some(&[video]), ms(650)), 0, "a redraw of the stream's pixels was sent");
        assert_eq!(page.frames, frames);
    }

    /// A client that asks for the picture whole is sent every stream's end and
    /// the picture as PNG.
    #[test]
    fn a_picture_forgotten_ends_its_streams_and_starts_over() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 4);
        let mut encoder = TilesEncoder::new(w, h, held(60), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let video = Rect { x: 0, y: 0, width: 256, height: 192 };
        for n in 1..=20u32 {
            paint(&mut pixels, w, video, 200 + n);
            step(&mut encoder, &mut page, &pixels, Some(&[video]), ms(u64::from(n) * 50));
        }
        assert_eq!((page.starts, page.ends), (1, 0));
        encoder.forget();
        assert_eq!(encoder.tick_at(), Some(Duration::ZERO), "the ends are owed at once, read or no read");
        let pngs = page.pngs;
        step(&mut encoder, &mut page, &pixels, None, ms(1100));
        assert_eq!(page.ends, 1, "the stream ended");
        assert_eq!(page.pngs - pngs, 4, "the picture again in its bands");
        assert_eq!(page.off(&pixels), 0);
        assert!(!encoder.owed(), "a picture sent whole owes nothing");
    }

    /// Copy the tile at `from` of a `w`-wide picture to the tile at `to`.
    fn same_tile(pixels: &mut [u8], w: u16, from: (u16, u16), to: (u16, u16)) {
        let line = usize::from(TILE) * 4;
        for row in 0..usize::from(TILE) {
            let src = ((usize::from(from.1) + row) * usize::from(w) + usize::from(from.0)) * 4;
            let dst = ((usize::from(to.1) + row) * usize::from(w) + usize::from(to.0)) * 4;
            pixels.copy_within(src..src + line, dst);
        }
    }

    /// A whole tile the client was shown, shown again elsewhere, is a draw
    /// of the slot it was stored in and no still: text moved to another
    /// window, a page scrolled back by a whole tile. Shown a third time it
    /// is the same slot again, and the page holds it exactly.
    #[test]
    fn a_tile_shown_again_is_drawn_from_the_cache() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 31);
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        assert_eq!(page.stored, 8, "every whole tile of the first picture stored");

        let text = Rect { x: 64, y: 0, width: 64, height: 64 };
        glyphs(&mut pixels, w, text);
        let (pngs, stored) = (page.pngs, page.stored);
        step(&mut encoder, &mut page, &pixels, Some(&[text]), ms(100));
        assert_eq!((page.pngs - pngs, page.stored - stored, page.cached), (1, 1, 0), "a tile of text: a PNG, stored");

        same_tile(&mut pixels, w, (64, 0), (192, 64));
        let (pngs, stored) = (page.pngs, page.stored);
        let shown = update(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 192, y: 64, width: 64, height: 64 }]), ms(200));
        assert_eq!((shown.rects, page.pngs - pngs, page.stored - stored, page.cached), (1, 0, 0, 1), "the same text elsewhere: drawn from the cache, no still, nothing stored");
        assert_eq!(page.off(&pixels), 0);
        assert!(!encoder.owed(), "a tile stored exact is owed nothing");

        // Painted over and shown again: the slot still has it.
        paint(&mut pixels, w, text, 32);
        step(&mut encoder, &mut page, &pixels, Some(&[text]), ms(300));
        glyphs(&mut pixels, w, text);
        let pngs = page.pngs;
        step(&mut encoder, &mut page, &pixels, Some(&[text]), ms(400));
        assert_eq!((page.pngs - pngs, page.cached), (0, 2), "shown a third time, from the cache again");
        assert_eq!(page.off(&pixels), 0);

        // A tile the damage covers in part is a still of that part, which
        // the cache neither keeps nor draws: the client's tile is not whole
        // in it.
        let part = Rect { x: 200, y: 70, width: 10, height: 10 };
        paint(&mut pixels, w, part, 33);
        let (pngs, stored) = (page.pngs, page.stored);
        step(&mut encoder, &mut page, &pixels, Some(&[part]), ms(500));
        assert_eq!((page.pngs - pngs, page.stored - stored, page.cached), (1, 0, 2), "a part of a tile: a PNG of it, not stored");
        assert_eq!(page.off(&pixels), 0);
    }

    /// A tile of one colour is never stored: a PNG of it is a few bytes,
    /// and a slot is worth more than that.
    #[test]
    fn a_flat_tile_is_not_stored() {
        let (w, h) = (128u16, 64u16);
        let mut pixels = noise(w, h, 41);
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        assert_eq!(page.stored, 2);
        let flat = Rect { x: 0, y: 0, width: 64, height: 64 };
        for row in 0..64 {
            pixels[row * usize::from(w) * 4..][..64 * 4].copy_from_slice(&[10, 20, 30, 0].repeat(64));
        }
        step(&mut encoder, &mut page, &pixels, Some(&[flat]), ms(100));
        assert_eq!((page.pngs, page.stored), (2, 2), "the flat tile's PNG, and no store");
        same_tile(&mut pixels, w, (0, 0), (64, 0));
        step(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 64, y: 0, width: 64, height: 64 }]), ms(200));
        assert_eq!((page.pngs, page.stored, page.cached), (3, 2, 0), "the same flat tile elsewhere: a PNG again, and no cache");
        assert_eq!(page.off(&pixels), 0);
    }

    /// On a walked list a tile stored below the ceiling is drawn from the
    /// cache only below it, and the tile it draws is owed the ceiling, as a
    /// copy of a coarse still is; the settle sends the stored tile at the
    /// ceiling and stores it again in its slot, settled, so the other is
    /// then a draw of that slot, at the ceiling, and nothing is owed.
    #[test]
    fn a_tile_cached_below_the_ceiling_is_owed_it() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 51);
        let mut encoder = TilesEncoder::new(w, h, walked(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        settle(&mut encoder, &mut page, ms(0));
        assert_eq!(page.stored, 8);

        encoder.set_pace(30, false);
        let text = Rect { x: 64, y: 0, width: 64, height: 64 };
        glyphs(&mut pixels, w, text);
        step(&mut encoder, &mut page, &pixels, Some(&[text]), ms(100));
        assert_eq!((page.webps, page.stored), (3, 9), "a WebP at 30, stored");
        same_tile(&mut pixels, w, (64, 0), (192, 64));
        step(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 192, y: 64, width: 64, height: 64 }]), ms(200));
        assert_eq!((page.webps, page.stored, page.cached), (3, 9, 1), "the same text elsewhere, below the ceiling: drawn from the cache");
        assert!(encoder.owed(), "both tiles are owed the ceiling");
        let coarse = page.distance(&pixels);

        encoder.set_pace(80, false);
        settle(&mut encoder, &mut page, ms(200));
        assert_eq!((page.webps, page.stored, page.cached), (4, 10, 2), "the first at the ceiling, stored again; the second a draw of that slot");
        assert!(page.distance(&pixels) < coarse, "finer at the ceiling");
        assert!(!encoder.owed());

        // Below the ceiling again, a tile stored at it is drawn from the cache
        // and owed nothing: it is the ceiling's picture.
        encoder.set_pace(30, false);
        same_tile(&mut pixels, w, (64, 0), (0, 64));
        step(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 0, y: 64, width: 64, height: 64 }]), ms(1000));
        assert_eq!((page.webps, page.cached), (4, 3));
        assert!(!encoder.owed(), "a tile drawn from a settled slot is owed nothing");
    }

    /// A picture that starts over forgets the cache: what the client was
    /// shown before is sent as a still again, and stored again.
    #[test]
    fn a_picture_forgotten_forgets_the_cache() {
        let (w, h) = (128u16, 64u16);
        let mut pixels = noise(w, h, 61);
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let text = Rect { x: 0, y: 0, width: 64, height: 64 };
        glyphs(&mut pixels, w, text);
        step(&mut encoder, &mut page, &pixels, Some(&[text]), ms(100));
        encoder.forget();
        step(&mut encoder, &mut page, &pixels, None, ms(200));
        assert_eq!((page.pngs, page.stored, page.cached), (3, 5, 0), "the picture again, whole, every whole tile stored anew");
        same_tile(&mut pixels, w, (0, 0), (64, 0));
        step(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 64, y: 0, width: 64, height: 64 }]), ms(300));
        assert_eq!((page.pngs, page.cached), (3, 1), "and from then on the cache serves again");
        assert_eq!(page.off(&pixels), 0);
    }

    /// A picture of a dot in each tile, every tile its own, `cols`×`rows`
    /// tiles: cheap to code as a palette, and more tiles than the cache has
    /// slots where it is big enough.
    fn dots(cols: u16, rows: u16) -> Vec<u8> {
        let w = usize::from(cols) * usize::from(TILE);
        let mut pixels = vec![0xE0; w * usize::from(rows) * usize::from(TILE) * 4];
        for i in 0..usize::from(cols) * usize::from(rows) {
            let (col, row) = (i % usize::from(cols), i / usize::from(cols));
            let (x, y) = (col * usize::from(TILE) + i % 64, row * usize::from(TILE) + (i / 64) % 64);
            pixels[(y * w + x) * 4..][..4].copy_from_slice(&[(i / 4096) as u8 + 1, 7, 9, 0]);
        }
        pixels
    }

    /// A tile drawn from a slot that the stores of the run ahead of it would
    /// take as the least recently used: the draw names the slot first, so
    /// the stores take others, and the page holds the picture exactly. The
    /// picture has 64 tiles more than the cache has slots, so its first 64
    /// tiles' slots were taken again by its last 64, and the slot of the
    /// first tile of the second row is the least recently used.
    #[test]
    fn a_draw_keeps_its_slot_from_the_stores_ahead_of_it() {
        let (cols, rows) = (64u16, 65u16);
        let (w, h) = (cols * TILE, rows * TILE);
        let mut pixels = dots(cols, rows);
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        assert_eq!(page.stored, usize::from(cols) * usize::from(rows));
        // The second row painted over: 63 tiles new, which take 63 slots, and
        // the last the first tile's old content, in the slot they would take first.
        let second = Rect { x: 0, y: TILE, width: w, height: TILE };
        let old: Vec<u8> = (0..usize::from(TILE)).flat_map(|row| pixels[((usize::from(TILE) + row) * usize::from(w)) * 4..][..usize::from(TILE) * 4].to_vec()).collect();
        paint(&mut pixels, w, Rect { width: w - TILE, ..second }, 71);
        for (row, line) in old.chunks_exact(usize::from(TILE) * 4).enumerate() {
            let at = ((usize::from(TILE) + row) * usize::from(w) + usize::from(w - TILE)) * 4;
            pixels[at..at + line.len()].copy_from_slice(line);
        }
        let (pngs, stored) = (page.pngs, page.stored);
        step(&mut encoder, &mut page, &pixels, Some(&[second]), ms(100));
        assert_eq!((page.pngs - pngs, page.stored - stored, page.cached), (1, 63, 1), "63 tiles stored and the last drawn from its slot");
        assert_eq!(page.off(&pixels), 0, "the drawn tile is what the slot held when it was named");
    }

    /// The record of the cache fills its slots in order, keeps a tile stored
    /// again in its slot, and once full takes the least recently used slot —
    /// used by a store or a draw — for a new tile, forgetting the one it kept.
    #[test]
    fn the_cache_fills_then_evicts_the_least_recently_used() {
        let mut cache = Cache::default();
        for hash in 0..u64::from(CACHE_SLOTS) {
            assert_eq!(cache.store(hash, true), hash as u16);
        }
        assert_eq!(cache.find(5), Some((5, true)));
        assert_eq!(cache.store(5, false), 5, "stored again, in its slot");
        assert_eq!(cache.find(5), Some((5, false)), "as it was last stored");
        cache.touch(0);
        cache.touch(1);
        assert_eq!(cache.store(1_000_000, true), 2, "slot 2 is the least recently used");
        assert_eq!(cache.find(2), None, "the tile it kept is forgotten");
        assert_eq!(cache.find(1_000_000), Some((2, true)));
        assert_eq!(cache.store(1_000_001, true), 3);
        assert_eq!(cache.store(1_000_002, true), 4);
        cache.clear();
        assert_eq!(cache.find(0), None);
        assert_eq!(cache.store(7, true), 0, "filled from the first slot again");
    }

    /// Play a video in the corner for `frames` frames of 50 ms from `from`,
    /// until an encode fails, and return its error.
    fn play_to_failure(encoder: &mut TilesEncoder, page: &mut Page, pixels: &mut [u8], w: u16, frames: u32) -> Option<TilesError> {
        let video = Rect { x: 0, y: 0, width: 256, height: 192 };
        let stride = usize::from(w) * 4;
        for n in 1..=frames {
            paint(pixels, w, video, 500 + n);
            let now = ms(u64::from(n) * 50);
            encoder.read(pixels, stride, Some(&[video]), now).unwrap();
            let mut out = Vec::new();
            match encoder.encode(now, &mut out) {
                Ok(update) => page.draw(update.rects, &out),

                Err(e) => return Some(e),
            }
        }
        None
    }

    /// A coder that refuses a stream fails the encode that would start it:
    /// a region has no other codec.
    #[test]
    fn a_region_coder_that_refuses_fails_the_encode() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 7);
        let mut encoder = TilesEncoder::new(w, h, held(60), Box::new(Fake { refuse: true, fail_after: None, fail_read_after: None })).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let failed = play_to_failure(&mut encoder, &mut page, &mut pixels, w, 20);
        assert!(matches!(failed, Some(TilesError::Region(_))), "{failed:?}");
        assert_eq!(page.starts, 0, "no stream started");
    }

    /// A stream that fails midway, at its encode or at its read, fails the
    /// encode it was coded in.
    #[test]
    fn a_region_stream_that_fails_fails_the_encode() {
        let (w, h) = (512u16, 256u16);
        for (seed, fake) in [(8, Fake { refuse: false, fail_after: Some(4), fail_read_after: None }), (10, Fake { refuse: false, fail_after: None, fail_read_after: Some(4) })] {
            let mut pixels = noise(w, h, seed);
            let mut encoder = TilesEncoder::new(w, h, held(60), Box::new(fake)).unwrap();
            let mut page = Page::new(w, h);
            step(&mut encoder, &mut page, &pixels, None, ms(0));
            let failed = play_to_failure(&mut encoder, &mut page, &mut pixels, w, 40);
            assert!(matches!(failed, Some(TilesError::Region(_))), "{failed:?}");
            assert_eq!((page.starts, page.frames, page.ends), (1, 3, 0), "the stream's four frames, then the failure");
        }
    }

    /// Rows `top` on of a `w`-wide document, `h` of them: a page scrolled to `top`.
    fn scrolled(document: &[u8], w: u16, top: usize, h: u16) -> Vec<u8> {
        let line = usize::from(w) * 4;
        document[top * line..(top + usize::from(h)) * line].to_vec()
    }

    /// A page scrolled is a copy of what the client holds, and only the rows
    /// it uncovered go as stills, a band of tiles: the page exact after it,
    /// for a fraction of the picture's bytes.
    #[test]
    fn a_scroll_is_a_copy() {
        let (w, h) = (256u16, 256u16);
        let document = noise(w, 3 * h, 21);
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &scrolled(&document, w, 0, h), None, ms(0));
        let whole = page.bytes;
        let pixels = scrolled(&document, w, 40, h);
        let scroll = update(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 0, y: 0, width: w, height: h }]), ms(100));
        assert_eq!((page.copies, page.pngs), (1, 4 + 1), "a copy, and the band the rows uncovered are in, a still");
        assert!(!scroll.keyframe);
        assert_eq!(page.off(&pixels), 0, "the page exact");
        assert!(page.bytes - whole < whole / 3, "{} bytes for a scroll, {whole} for the picture", page.bytes - whole);
        assert!(!encoder.owed(), "a copy of exact pixels owes nothing");
    }

    /// A scroll that goes on stays copies, the tiles it moves no churn, so it
    /// never becomes a region's stream, and up as well as down.
    #[test]
    fn a_scroll_that_goes_on_is_no_stream() {
        let (w, h) = (320u16, 192u16);
        let document = noise(w, 4 * h, 22);
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &scrolled(&document, w, 0, h), None, ms(0));
        let tops = (1..=12).map(|n| n * 24).chain((0..12).rev().map(|n| n * 24));
        for (n, top) in tops.enumerate() {
            let pixels = scrolled(&document, w, top, h);
            let copies = page.copies;
            step(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 0, y: 0, width: w, height: h }]), ms(50 * (n as u64 + 1)));
            assert_eq!(page.copies, copies + 1, "a copy at row {top}");
            let bad: Vec<usize> = (0..usize::from(h)).filter(|y| page.canvas[y * usize::from(w) * 4..(y + 1) * usize::from(w) * 4].as_chunks::<4>().0.iter().zip(pixels[y * usize::from(w) * 4..(y + 1) * usize::from(w) * 4].as_chunks::<4>().0).any(|(a, b)| a[..3] != b[..3])).collect();
            assert_eq!(page.off(&pixels), 0, "the page exact at row {top}: rows {bad:?} pngs {}", page.pngs);
        }
        assert_eq!(page.starts, 0, "no stream");
    }

    /// A window dragged over text is a copy of where it was: what it
    /// uncovered and the tiles its edges cut go as stills.
    #[test]
    fn a_window_dragged_is_a_copy() {
        let (w, h) = (384u16, 256u16);
        let mut desktop = vec![0; usize::from(w) * usize::from(h) * 4];
        glyphs(&mut desktop, w, Rect { x: 0, y: 0, width: w, height: h });
        let window = noise(160, 112, 23);
        let place = |at: (u16, u16)| {
            let mut pixels = desktop.clone();
            for row in 0..112 {
                let to = ((usize::from(at.1) + row) * usize::from(w) + usize::from(at.0)) * 4;
                pixels[to..to + 160 * 4].copy_from_slice(&window[row * 160 * 4..(row + 1) * 160 * 4]);
            }
            pixels
        };
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &place((40, 30)), None, ms(0));
        let pixels = place((110, 90));
        let moved = Rect { x: 40, y: 30, width: 160 + 70, height: 112 + 60 };
        step(&mut encoder, &mut page, &pixels, Some(&[moved]), ms(100));
        assert_eq!(page.copies, 1, "the window");
        assert_eq!(page.off(&pixels), 0);
        assert!(!encoder.owed());
    }

    /// Two reads before an encode, a scroll each: the second's copy reads
    /// none of the rows the first uncovered, whose still is not sent yet, so
    /// the page is exact after the one update that carries both.
    #[test]
    fn a_copy_reads_only_what_the_client_holds() {
        let (w, h) = (256u16, 256u16);
        let document = noise(w, 3 * h, 24);
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &scrolled(&document, w, 0, h), None, ms(0));
        let stride = usize::from(w) * 4;
        let all = [Rect { x: 0, y: 0, width: w, height: h }];
        encoder.read(&scrolled(&document, w, 40, h), stride, Some(&all), ms(100)).unwrap();
        let pixels = scrolled(&document, w, 100, h);
        encoder.read(&pixels, stride, Some(&all), ms(110)).unwrap();
        let mut out = Vec::new();
        let update = encoder.encode(ms(110), &mut out).unwrap();
        page.draw(update.rects, &out);
        assert_eq!(page.copies, 2, "both scrolls");
        assert_eq!(page.off(&pixels), 0, "the page exact");
    }

    /// A picture with no edge that repeats, which a WebP at 20 blurs far
    /// more than one at 80: three smooth ramps, one of them a product.
    fn ramps(w: u16, h: u16) -> Vec<u8> {
        (0..usize::from(h)).flat_map(|y| (0..usize::from(w)).flat_map(move |x| [((x + 2 * y) / 3 % 256) as u8, (x * y / 40 % 256) as u8, ((3 * x + y) / 5 % 256) as u8, 0])).collect()
    }

    /// A copy of a still the walk sent below the ceiling is owed the ceiling
    /// as its source is, and the settle pays it.
    #[test]
    fn a_copy_of_a_coarse_still_is_owed_the_ceiling() {
        let (w, h) = (256u16, 256u16);
        let document = ramps(w, 3 * h);
        let mut encoder = TilesEncoder::new(w, h, walked(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        encoder.set_pace(20, false);
        step(&mut encoder, &mut page, &scrolled(&document, w, 0, h), None, ms(0));
        let pixels = scrolled(&document, w, 40, h);
        step(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 0, y: 0, width: w, height: h }]), ms(100));
        assert_eq!(page.copies, 1);
        let coarse = page.distance(&pixels);
        encoder.set_pace(80, false);
        assert!(encoder.owed(), "the copied tiles are owed the ceiling");
        let webps = page.webps;
        settle(&mut encoder, &mut page, ms(100));
        assert!(page.webps - webps >= 4, "the copied rows of tiles sent again: {}", page.webps - webps);
        let mut fresh = Page::new(w, h);
        step(&mut TilesEncoder::new(w, h, walked(80), Fake::coder()).unwrap(), &mut fresh, &pixels, None, ms(0));
        let (settled, ceiling) = (page.distance(&pixels), fresh.distance(&pixels));
        assert!(settled < coarse && settled <= ceiling + ceiling / 20, "settled {settled}, against {coarse} at 20 and {ceiling} for the picture sent at 80");
    }

    /// On a walked list a copy is owed the ceiling even from a still sent
    /// at it, and the settle sends its tiles again where they now are.
    #[test]
    fn a_walked_copy_is_owed_the_ceiling() {
        let (w, h) = (256u16, 256u16);
        let document = ramps(w, 3 * h);
        let mut encoder = TilesEncoder::new(w, h, walked(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &scrolled(&document, w, 0, h), None, ms(0));
        settle(&mut encoder, &mut page, ms(0));
        assert!(!encoder.owed(), "the first picture, at the ceiling, owes nothing");
        let pixels = scrolled(&document, w, 40, h);
        step(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 0, y: 0, width: w, height: h }]), ms(100));
        assert_eq!(page.copies, 1);
        assert!(encoder.owed(), "the copied tiles are owed the ceiling");
        let webps = page.webps;
        settle(&mut encoder, &mut page, ms(100));
        assert!(page.webps > webps, "the copied tiles sent again");
        assert!(!encoder.owed());
    }

    /// Beside a region's stream, a scroll's copy neither draws on the tiles
    /// the stream carries nor reads them, and the page is exact outside it.
    #[test]
    fn a_copy_keeps_off_a_stream() {
        let (w, h) = (512u16, 256u16);
        let document = noise(256, 3 * h, 26);
        let mut pixels = noise(w, h, 27);
        let page_at = |pixels: &mut [u8], top: usize| {
            for row in 0..usize::from(h) {
                let to = (row * usize::from(w) + 256) * 4;
                pixels[to..to + 256 * 4].copy_from_slice(&document[(top + row) * 256 * 4..(top + row + 1) * 256 * 4]);
            }
        };
        page_at(&mut pixels, 0);
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let video = Rect { x: 0, y: 0, width: 256, height: 192 };
        let outside = |page: &Page, pixels: &[u8]| {
            (0..usize::from(h)).flat_map(|y| (0..usize::from(w)).map(move |x| (x, y))).filter(|(x, y)| *x >= 256 || *y >= 192).filter(|(x, y)| page.canvas[(y * usize::from(w) + x) * 4..][..3] != pixels[(y * usize::from(w) + x) * 4..][..3]).count()
        };
        for n in 1..=20u32 {
            paint(&mut pixels, w, video, 600 + n);
            let mut damage = vec![video];
            if n > 12 {
                page_at(&mut pixels, (n as usize - 12) * 24);
                damage.push(Rect { x: 256, y: 0, width: 256, height: h });
            }
            step(&mut encoder, &mut page, &pixels, Some(&damage), ms(u64::from(n) * 50));
            assert_eq!(outside(&page, &pixels), 0, "the page exact outside the stream at frame {n}");
        }
        assert_eq!(page.starts, 1, "the video streams");
        assert!(page.copies >= 8, "the page beside it scrolls by copies: {}", page.copies);
    }

    /// A copy draws what the client holds of its source, so where that is lossy
    /// — the tiles a stream carried, owed their still — every tile the copy
    /// draws on is owed it too, one that did not change included, and the
    /// settle leaves the page exact.
    #[test]
    fn a_copy_from_a_lossy_source_owes_every_tile_it_draws() {
        let (w, h) = (256u16, 320u16);
        // Noise, but for 192 rows of grey from row 64 of the document.
        let mut document = noise(w, 3 * h, 30);
        let line = usize::from(w) * 4;
        for row in 64..256 {
            document[row * line..(row + 1) * line].copy_from_slice(&[0x81, 0x81, 0x81, 0].repeat(usize::from(w)));
        }
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &scrolled(&document, w, 0, h), None, ms(0));
        // The third row of tiles as a stream leaves it: lossy, and owed.
        for row in 128..192 {
            for sample in &mut page.canvas[row * line..(row + 1) * line] {
                *sample &= !1;
            }
        }
        for col in 0..4 {
            encoder.debts.insert((col, 2), ms(0));
        }
        // Scrolled 64 rows: the grey moves up a row of tiles, and the second row
        // of tiles, grey before and after, is drawn from the lossy third.
        let pixels = scrolled(&document, w, 64, h);
        step(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 0, y: 0, width: w, height: h }]), ms(100));
        assert_eq!(page.copies, 1);
        assert!((0..4).all(|col| encoder.debts.contains_key(&(col, 1))), "the tiles the copy drew that did not change are owed");
        settle(&mut encoder, &mut page, ms(100));
        assert_eq!(page.off(&pixels), 0, "the page exact once settled");
    }

    /// A picture that changed without anything in it moving finds no copy.
    #[test]
    fn a_new_picture_is_no_copy() {
        let (w, h) = (256u16, 256u16);
        let mut encoder = TilesEncoder::new(w, h, held(80), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &noise(w, h, 28), None, ms(0));
        let pixels = noise(w, h, 29);
        step(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 0, y: 0, width: w, height: h }]), ms(100));
        assert_eq!(page.copies, 0);
        assert_eq!(page.off(&pixels), 0);
    }
}
