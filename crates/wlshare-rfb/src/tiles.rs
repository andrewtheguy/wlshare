//! The tiles encoding: the desktop as exact PNG tiles where it is still, and a
//! video stream of its own for each region that moves, for a client on a link
//! with room to spare.
//!
//! A private encoding, [`crate::ENCODING_TILES`], beside [`crate::vp9`]'s
//! rather than in place of it: the VP9 encoding, one stream over the whole
//! desktop walked on the link, is the picture for a link that is short of
//! bytes, and this one spends bytes the link has for less of the encoder's
//! time and a picture that ends exact. Its rules are the remotex gateway's
//! Motion's, brought here because only here are the compositor's pixels and
//! damage coded once.
//!
//! **The rectangles.** A pixel update in this encoding is any number of
//! rectangles of it, drawn in order, each a rectangle of the desktop whose
//! body begins with a kind:
//!
//! ```text
//! u8 kind
//! 0 PNG     u32 length | u8[length]  a PNG of exactly the rectangle's pixels
//! 1 START   u8 stream | u8 codec | u32 length | u8[length]  a region's first frame
//! 2 FRAME   u8 stream | u32 length | u8[length]  a region's next frame
//! 3 END     u8 stream                            the region's stream is over
//! ```
//!
//! - A **PNG** is the desktop's pixels in the rectangle exactly, 8-bit RGB or
//!   a palette of it, and the picture there until a later rectangle draws
//!   over it. It depends on nothing before it.
//! - A **START** begins a stream on `stream`, over its rectangle, and says
//!   which codec it is ([`RegionCodec`]): a keyframe of exactly the
//!   rectangle's size, 8-bit 4:2:0, BT.601 at studio swing — VP9 profile 0,
//!   or an H.264 access unit in Annex B with its parameter sets, from a host
//!   with a hardware encoder: the server's choice, which a client takes
//!   either way. Each **FRAME** on
//!   that id is the next frame of that stream, over the same rectangle, in
//!   its codec, coded against the frames before it. A decoder draws each
//!   frame's picture at the rectangle.
//! - An **END** says the stream on `stream` is over: its last picture stays
//!   until something draws over it, and no frame follows on the id until a
//!   START begins another. The rectangle is the stream's.
//!
//! A PNG over a stream's rectangle only ever follows the stream's END, so a
//! client draws every rectangle where it says, in order, and the picture is
//! right. A change of the desktop's size, which is sent first as an update of
//! its own, ends every stream with no END, and the next update sends the
//! picture again whole.
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
//! **What a stream codes.** A region is an encoder over its rectangle alone,
//! told the damage inside it as the VP9 encoding's whole-desktop stream is, so
//! the encoder's time follows the area that moves. It is 4:2:0 at the dial the
//! client's list names, held there: what a stream loses of a picture lasts
//! only while the region moves, since it settles exact. The encoder is libvpx
//! at speed 9, the least search it has, or, where the daemon hands this
//! encoder a [`RegionCoder`] — its host's hardware H.264 encoder — and the
//! client takes H.264, that: a region then costs the processor its conversion
//! alone. A hardware coder that refuses a stream, or fails one midway, is
//! given up for the session, the failed stream ended, and VP9 codes every
//! region from there: the picture goes on either way.
//!
//! **It settles exact.** Every tile a stream carried is owed a PNG once it has
//! been still for half a second, looked for every quarter of a second
//! ([`TilesEncoder::tick_at`]), at most 40 tiles at a time, and sent after the
//! stream's END, so a desktop that stops is exact. A picture that starts over —
//! an encoder's first, or one a client asks for whole ([`TilesEncoder::forget`])
//! — goes as PNG at once, in bands of 64 rows; from then on a band is sent as
//! the runs of its changed tiles along each row.
//!
//! The measurements these rules were chosen by are `examples/motion-bench.rs`,
//! which plays the kept captures through this encoder and through the VP9
//! encoding's.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::time::Duration;

use log::warn;
use thiserror::Error;

use crate::vp9::{Chroma, QUALITY_MAX, QUALITY_MIN, Rect, Speed};

/// A PNG of the rectangle's pixels.
pub const KIND_PNG: u8 = 0;
/// A region's first frame, which begins its stream.
pub const KIND_START: u8 = 1;
/// A region's next frame.
pub const KIND_FRAME: u8 = 2;
/// A region's stream is over.
pub const KIND_END: u8 = 3;

/// What a region's stream is coded as, the byte its START carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RegionCodec {
    /// VP9 profile 0, by libvpx: every region's codec unless the host has a
    /// hardware H.264 encoder and the client takes H.264.
    Vp9 = 0,
    /// H.264, each frame one Annex B access unit, the parameter sets ahead of
    /// each keyframe's slices.
    H264 = 1,
}

impl RegionCodec {
    /// The codec a START's byte names, or `None` for a byte that is neither.
    pub fn of(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Vp9),
            1 => Some(Self::H264),
            _ => None,
        }
    }
}

/// Why a region coder could not open or code a stream: a hardware encoder's
/// reason, in its own words.
pub type RegionError = Box<dyn std::error::Error + Send + Sync>;

/// A coder of region streams the daemon hands a [`TilesEncoder`] in place of
/// VP9: its host's hardware H.264 encoder, wherever the host has one, since
/// every client decodes both. The encoder opens a stream of it for each
/// region, and on the first failure, to open or to code, gives it up for
/// VP9 for the rest of the session.
pub trait RegionCoder: Send {
    /// The codec its streams are coded as.
    fn codec(&self) -> RegionCodec;
    /// A stream over a `width`×`height` region at `quality`, the 1–100 dial,
    /// which codes nothing until read into.
    fn open(&mut self, width: u16, height: u16, quality: u8) -> Result<Box<dyn RegionStream>, RegionError>;
}

/// One region's stream as a [`RegionCoder`] codes it, in the shape of
/// [`wlshare_vp9::Stream`]: read, then encode.
pub trait RegionStream: Send {
    /// Move the dial without a keyframe.
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

/// A region's chroma: profile 0, which every browser's decoder takes, its
/// hardware one included, at half the samples of 4:4:4 to code.
pub const REGION_CHROMA: Chroma = Chroma::Yuv420;
/// A region's speed: libvpx's least search, since what it leaves is settled exact.
pub const REGION_SPEED: Speed = Speed::Fastest;

/// The dial the client's list asks the tiles picture's streams held at — the
/// VP9 encoding's `WLQ` and the value
/// ([`crate::vp9::ENCODING_VP9_QUALITY_BASE`]) beside [`crate::ENCODING_TILES`] —
/// or `None` where the list does not name the encoding. The server has no
/// quality of its own, so a list that names the encoding without one is
/// fatal, and so is one that names the VP9 encoding beside it: they are two
/// pictures, of which a client wants one.
pub fn listed(encodings: &[i32]) -> Result<Option<u8>, TilesListError> {
    if !encodings.contains(&crate::ENCODING_TILES) {
        return Ok(None);
    }
    if encodings.contains(&crate::ENCODING_VP9) {
        return Err(TilesListError::Both);
    }
    let quality = encodings
        .iter()
        .filter_map(|&e| u8::try_from(e.checked_sub(crate::vp9::ENCODING_VP9_QUALITY_BASE)?).ok())
        .find(|q| (QUALITY_MIN..=QUALITY_MAX).contains(q))
        .ok_or(TilesListError::NoQuality)?;
    Ok(Some(quality))
}

/// Why a list that names the tiles encoding asks for no picture this server
/// sends. Fatal to the connection, as a malformed message is.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TilesListError {
    #[error("the list names the tiles encoding without a quality 1–100 beside it")]
    NoQuality,
    #[error("the list names both the tiles and the VP9 encoding")]
    Both,
}

/// Why a picture could not be read or encoded.
#[derive(Debug, Error)]
pub enum TilesError {
    #[error(transparent)]
    Codec(#[from] wlshare_vp9::Error),
    /// A VP9 region stream refused its pixels or its frame.
    #[error("coding a region: {0}")]
    Region(RegionError),
    #[error(transparent)]
    Png(#[from] png::EncodingError),
    #[error("a {width}x{height} picture at a stride of {stride} does not fit in {bytes} bytes")]
    Buffer { width: u16, height: u16, stride: usize, bytes: usize },
    #[error("a rectangle of {0} bytes is over the {max}-byte ceiling", max = crate::msg::MAX_RECT_BODY)]
    TooLong(usize),
    #[error("an update of {0} rectangles is more than one can count")]
    TooMany(usize),
}

// ── Geometry ─────────────────────────────────────────────────────────────────

/// A tile's side, ZRLE's, from the desktop's top-left.
const TILE: u16 = 64;

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
/// How long a tile a stream carried is still before it is owed as PNG.
const CLEANUP_IDLE: Duration = Duration::from_millis(500);
/// How often what is owed is looked for.
pub const CLEANUP_TICK: Duration = Duration::from_millis(250);
/// The most tiles a tick sends.
const MAX_CLEANUPS_PER_TICK: usize = 40;
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
/// have at most 256 colours, RGB with each row's filter chosen otherwise, at
/// `png`'s fast compression. What the remotex gateway codes its own tiles as.
fn encode_png(w: u16, h: u16, rgb: &[u8], out: &mut Vec<u8>) -> Result<(), png::EncodingError> {
    let mut encoder = png::Encoder::new(out, u32::from(w), u32::from(h));
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
    writer.finish()
}

// ── The encoder ──────────────────────────────────────────────────────────────

/// One live region stream.
struct Live {
    /// The id its rectangles carry, which no other live stream has.
    id: u8,
    area: Area,
    cells: Vec<(u16, u16)>,
    codec: RegionCodec,
    stream: Box<dyn RegionStream>,
    /// The damage inside the region since its last frame, from its top-left.
    pending: Vec<Rect>,
    dirty: bool,
    /// Its START went out, and so its END is owed when it ends.
    started: bool,
    carried: bool,
    moving_at: Duration,
}

/// The rectangles of one update as they are appended, and how many.
struct Out<'a> {
    out: &'a mut Vec<u8>,
    count: usize,
}

impl Out<'_> {
    fn header(&mut self, area: Area, kind: u8) {
        self.out.extend_from_slice(&crate::msg::rect_header(area.left, area.top, area.w(), area.h(), crate::ENCODING_TILES));
        self.out.push(kind);
        self.count += 1;
    }

    /// Fill the length word at `at` with what follows it, or take back the
    /// rectangle begun at `from` if it is past the ceiling.
    fn length(&mut self, from: usize, at: usize) -> Result<(), TilesError> {
        let len = self.out.len() - at - 4;
        if len > crate::msg::MAX_RECT_BODY {
            self.out.truncate(from);
            self.count -= 1;
            return Err(TilesError::TooLong(len));
        }
        self.out[at..at + 4].copy_from_slice(&(len as u32).to_be_bytes());
        Ok(())
    }

    fn end(&mut self, id: u8, area: Area) {
        self.header(area, KIND_END);
        self.out.push(id);
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
    live: Vec<Live>,
    /// The tiles a live stream carries.
    covered: HashSet<(u16, u16)>,
    /// Tile → when a stream last carried it, which is owed a PNG.
    debts: HashMap<(u16, u16), Duration>,
    retuned_at: Option<Duration>,
    /// The PNGs the reads since the last encode owe.
    pngs: Vec<Area>,
    /// Streams that ended outside an encode, owed their END.
    ended: Vec<(u8, Area)>,
    /// A read came since the last encode, which owes a round of the streams.
    read: bool,
    next_tick: Duration,
    quality: u8,
    threads: usize,
    /// The coder every new stream is opened on ahead of VP9, while one is
    /// held and has not failed.
    hardware: Option<Box<dyn RegionCoder>>,
    scratch: Vec<u8>,
}

impl TilesEncoder {
    /// An encoder for a `width`×`height` desktop whose streams are at
    /// `quality` (1–100), each a VP9 stream on as many threads as this machine
    /// gives one, or a stream of `hardware` where one is given, until it
    /// fails. The client holds nothing of it yet.
    pub fn new(width: u16, height: u16, quality: u8, hardware: Option<Box<dyn RegionCoder>>) -> Result<Self, TilesError> {
        Self::with_threads(width, height, quality, crate::vp9::encoder_threads(), hardware)
    }

    /// [`Self::new`] with each VP9 stream on `threads` threads.
    pub fn with_threads(width: u16, height: u16, quality: u8, threads: usize, hardware: Option<Box<dyn RegionCoder>>) -> Result<Self, TilesError> {
        if width == 0 || height == 0 {
            return Err(wlshare_vp9::Error::Empty(width, height).into());
        }
        let stride = usize::from(width) * 4;
        let mut encoder = Self {
            size: (width, height),
            stride,
            mirror: vec![0; stride * usize::from(height)],
            churn: Churn::default(),
            unknown: HashSet::new(),
            live: Vec::new(),
            covered: HashSet::new(),
            debts: HashMap::new(),
            retuned_at: None,
            pngs: Vec::new(),
            ended: Vec::new(),
            read: false,
            next_tick: Duration::ZERO,
            quality,
            threads,
            hardware,
            scratch: Vec::new(),
        };
        encoder.forget();
        Ok(encoder)
    }

    /// The codec new streams are opened in: the hardware coder's while one is
    /// held, VP9 otherwise.
    pub fn codec(&self) -> RegionCodec {
        self.hardware.as_ref().map_or(RegionCodec::Vp9, |coder| coder.codec())
    }

    /// The desktop size this encoder codes.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// The dial the streams are coded at.
    pub fn quality(&self) -> u8 {
        self.quality
    }

    /// Move every stream's dial, and the dial streams are made at, without a
    /// keyframe.
    pub fn set_quality(&mut self, quality: u8) {
        if quality != self.quality {
            self.quality = quality;
            for live in &mut self.live {
                live.stream.set_quality(quality);
            }
        }
    }

    /// The client holds nothing: every stream ends, with its END at the next
    /// encode, nothing is owed, and the next read is sent whole as PNG.
    pub fn forget(&mut self) {
        for live in self.live.drain(..) {
            if live.started {
                self.ended.push((live.id, live.area));
            }
        }
        self.covered.clear();
        self.debts.clear();
        self.retuned_at = None;
        self.churn.clear();
        self.pngs.clear();
        self.unknown = tiles_of(Area::whole(self.size)).map(|(cell, _)| cell).collect();
    }

    /// The picture as it is now — [`Self::size`] of `B, G, R, X` pixels whose
    /// rows are `stride` bytes apart — where `changed` says it changed since
    /// the read before, or anywhere for `None`: the caller's word, as the VP9
    /// encoding takes it. The rectangles are copied into the encoder's own
    /// picture and compared with what the client holds, so `pixels` is the
    /// framebuffer itself, for as long as the copy takes. Nothing is coded: the
    /// next [`Self::encode`] codes what this owes. The first read of an encoder
    /// made or forgotten is the whole picture whatever `changed` says.
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
        for area in areas {
            self.damage(pixels, stride, area, now);
        }
        self.read = true;
        Ok(())
    }

    /// One rectangle of damage: into the picture, the streams told where it
    /// changed what they carry, the churn of the tiles it changed, and the runs
    /// of those no stream carries owed as PNG, band by band. A tile it names
    /// whose pixels are what the client holds is no change to anything.
    fn damage(&mut self, pixels: &[u8], stride: usize, area: Area, now: Duration) {
        let mut changed: HashSet<(u16, u16)> = tiles_of(area).filter(|(_, part)| differs(&self.mirror, self.stride, pixels, stride, *part)).map(|(cell, _)| cell).collect();
        for (cell, part) in tiles_of(area) {
            if self.unknown.contains(&cell) {
                changed.insert(cell);
                if part == self.cell_area(cell) {
                    self.unknown.remove(&cell);
                }
            }
        }
        for top in (area.top..=area.bottom).step_by(usize::from(BAND_ROWS)) {
            let band = Area { top, bottom: top.saturating_add(BAND_ROWS - 1).min(area.bottom), ..area };
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
                self.churn.observe(cell, now);
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
            self.pngs.extend(runs);
        }
    }

    /// Append to `out` the rectangles of what is owed at `now`, and return how
    /// many: the ENDs of streams [`Self::forget`] ended, the PNGs the reads
    /// owe, a round of the streams after a read — the regions laid out again
    /// and every stream the damage fell on coded — and, at its time
    /// ([`Self::tick_at`]), the streams whose motion stopped ended and the
    /// tiles still long enough sent exact. None when nothing is owed, which is
    /// no update to send.
    ///
    /// Tens of milliseconds for a desktop that changed all over. An error
    /// takes back the rectangle it was in, and leaves the ones before it.
    pub fn encode(&mut self, now: Duration, out: &mut Vec<u8>) -> Result<u16, TilesError> {
        let mut out = Out { out, count: 0 };
        for (id, area) in std::mem::take(&mut self.ended) {
            out.end(id, area);
        }
        for area in std::mem::take(&mut self.pngs) {
            self.png(area, &mut out)?;
        }
        if std::mem::take(&mut self.read) {
            self.round(now, &mut out)?;
        }
        if now >= self.next_tick {
            self.next_tick = now + CLEANUP_TICK;
            self.tick(now, &mut out)?;
        }
        u16::try_from(out.count).map_err(|_| TilesError::TooMany(out.count))
    }

    /// Whether anything is owed that no read will bring: a stream is live, or
    /// its end is yet to be sent, or a tile one carried is yet to be sent exact.
    pub fn owed(&self) -> bool {
        !self.live.is_empty() || !self.ended.is_empty() || !self.debts.is_empty()
    }

    /// When an [`Self::encode`] with no read is next owed something — a
    /// stream to end, a tile to send exact — or `None` while the client holds
    /// the picture exactly and nothing streams. Streams [`Self::forget`] ended
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

    /// Send `area` of the picture exactly, which pays the debt of every tile
    /// it covers whole.
    fn png(&mut self, area: Area, out: &mut Out) -> Result<(), TilesError> {
        crop_rgb(&self.mirror, self.stride, area, &mut self.scratch);
        let from = out.out.len();
        out.header(area, KIND_PNG);
        let at = out.out.len();
        out.out.extend_from_slice(&[0; 4]);
        if let Err(e) = encode_png(area.w(), area.h(), &self.scratch, out.out) {
            out.out.truncate(from);
            out.count -= 1;
            return Err(e.into());
        }
        out.length(from, at)?;
        for (cell, _) in tiles_of(area) {
            if area.contains(self.cell_area(cell)) {
                self.debts.remove(&cell);
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
        let mut failed: Option<(usize, RegionError)> = None;
        for (index, live) in self.live.iter_mut().enumerate() {
            live.carried = false;
            if !live.dirty {
                continue;
            }
            let at = usize::from(live.area.top) * stride + usize::from(live.area.left) * 4;
            let from = out.out.len();
            // Whether the rectangle was begun, which a failed read never does.
            let mut headed = false;
            let coded = live.stream.read_bgrx(&self.mirror[at..], stride, Some(&live.pending)).and_then(|()| {
                live.pending.clear();
                headed = true;
                out.header(live.area, if live.started { KIND_FRAME } else { KIND_START });
                out.out.push(live.id);
                if !live.started {
                    out.out.push(live.codec as u8);
                }
                let length_at = out.out.len();
                out.out.extend_from_slice(&[0; 4]);
                Ok((live.stream.encode(!live.started, out.out)?, length_at))
            });
            match coded {
                Ok((true, length_at)) => {
                    out.length(from, length_at)?;
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
                Err(e) if live.codec == RegionCodec::Vp9 => {
                    if headed {
                        out.out.truncate(from);
                        out.count -= 1;
                    }
                    return Err(TilesError::Region(e));
                }
                // A hardware coder that fails a stream is given up: the stream
                // ends, what it carried is owed exact, and the regions are laid
                // out again at the next round, on VP9.
                Err(e) => {
                    if headed {
                        out.out.truncate(from);
                        out.count -= 1;
                    }
                    failed = Some((index, e));
                    break;
                }
            }
        }
        if let Some((index, e)) = failed {
            let live = self.live.remove(index);
            warn!("the {:?} region coder failed a {}x{} stream; its regions are VP9 from here: {e}", live.codec, live.area.w(), live.area.h());
            if live.started {
                out.end(live.id, live.area);
            }
            self.hardware = None;
            self.retuned_at = None;
            self.cover();
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
        for area in wanted {
            if next.iter().any(|live| live.area.contains(area)) {
                continue;
            }
            if next.len() < MAX_STREAMS {
                for live in old.extract_if(.., |live| live.area.intersect(area).is_some()) {
                    if live.started {
                        out.end(live.id, live.area);
                    }
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
            } else if live.started {
                out.end(live.id, live.area);
            }
        }
        self.live = next;
        self.cover();
        Ok(())
    }

    /// A stream over `area`, every tile of it owed from now: on the hardware
    /// coder while one is held, which is given up for VP9 the first time it
    /// refuses a stream.
    fn build(&mut self, id: u8, area: Area, now: Duration) -> Result<Live, TilesError> {
        let (w, h) = (area.w(), area.h());
        let hardware = self.hardware.as_mut().and_then(|coder| match coder.open(w, h, self.quality) {
            Ok(stream) => Some((coder.codec(), stream)),
            Err(e) => {
                warn!("the {:?} region coder refused a {w}x{h} stream; its regions are VP9 from here: {e}", coder.codec());
                None
            }
        });
        if hardware.is_none() {
            self.hardware = None;
        }
        let (codec, stream): (RegionCodec, Box<dyn RegionStream>) = match hardware {
            Some(opened) => opened,
            None => {
                let mut stream = wlshare_vp9::Stream::new(w, h, self.quality, REGION_CHROMA, self.threads)?;
                stream.set_speed(REGION_SPEED);
                (RegionCodec::Vp9, Box::new(Vp9Region(stream)))
            }
        };
        let cells: Vec<(u16, u16)> = tiles_of(area).map(|(cell, _)| cell).collect();
        for cell in &cells {
            self.debts.insert(*cell, now);
        }
        Ok(Live { id, area, cells, codec, stream, pending: Vec::new(), dirty: true, started: false, carried: false, moving_at: now })
    }

    /// The streams whose motion stopped ended, and up to
    /// [`MAX_CLEANUPS_PER_TICK`] tiles still for [`CLEANUP_IDLE`] sent
    /// exactly, oldest first, as runs along a row.
    fn tick(&mut self, now: Duration, out: &mut Out) -> Result<(), TilesError> {
        let before = self.live.len();
        for live in self.live.extract_if(.., |live| now.saturating_sub(live.moving_at) >= STREAM_IDLE) {
            if live.started {
                out.end(live.id, live.area);
            }
        }
        if self.live.len() != before {
            self.cover();
        }
        let mut ready: Vec<((u16, u16), Duration)> = self.debts.iter().filter(|(cell, at)| !self.covered.contains(*cell) && now.saturating_sub(**at) >= CLEANUP_IDLE).map(|(cell, at)| (*cell, *at)).collect();
        ready.sort_unstable_by_key(|((col, row), at)| (*at, *row, *col));
        ready.truncate(MAX_CLEANUPS_PER_TICK);
        let mut cells: Vec<(u16, u16)> = ready.into_iter().map(|(cell, _)| cell).collect();
        cells.sort_unstable_by_key(|(col, row)| (*row, *col));
        let mut runs: Vec<CellBox> = Vec::new();
        for (col, row) in cells {
            match runs.last_mut() {
                Some(run) if run.r0 == row && run.c1.checked_add(1) == Some(col) => run.c1 = col,
                _ => runs.push(CellBox::of((col, row))),
            }
        }
        for run in runs {
            if let Some(area) = run.to_area(self.size) {
                self.png(area, out)?;
            }
        }
        Ok(())
    }
}

/// A region's VP9 stream as a [`RegionStream`]: libvpx's, at 4:2:0 and speed 9.
struct Vp9Region(wlshare_vp9::Stream);

impl RegionStream for Vp9Region {
    fn set_quality(&mut self, quality: u8) {
        self.0.set_quality(quality);
    }

    fn read_bgrx(&mut self, pixels: &[u8], stride: usize, changed: Option<&[Rect]>) -> Result<(), RegionError> {
        Ok(self.0.read_bgrx(pixels, stride, changed)?)
    }

    fn encode(&mut self, keyframe: bool, out: &mut Vec<u8>) -> Result<bool, RegionError> {
        match self.0.encode(keyframe, out) {
            Ok(_) => Ok(true),
            Err(wlshare_vp9::Error::NoFrame) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

/// The encoder read back as a client would: every rectangle parsed by the
/// client's parser, each PNG by a reader written here from the PNG
/// specification over `flate2`'s inflate, which shares nothing with the `png`
/// crate's, and each region by libvpx's decoder.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{self, RectBody, ServerMsg, TileBody};
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

    /// A stream's decoder: libvpx's for VP9, and for the fake H.264 coder of
    /// these tests ([`Fake`]) nothing, since its frames are the pixels.
    enum Region {
        Vp9(wlshare_vp9::Decoder),
        Fake,
    }

    /// What a client draws of the rectangles, and what it was sent.
    struct Page {
        w: usize,
        canvas: Vec<u8>,
        decoders: HashMap<u8, (Region, Place)>,
        pngs: usize,
        starts: usize,
        /// The codec of each START, in order.
        codecs: Vec<RegionCodec>,
        frames: usize,
        ends: usize,
        bytes: usize,
    }

    impl Page {
        fn new(w: u16, h: u16) -> Self {
            Self { w: usize::from(w), canvas: vec![0; usize::from(w) * usize::from(h) * 4], decoders: HashMap::new(), pngs: 0, starts: 0, codecs: Vec::new(), frames: 0, ends: 0, bytes: 0 }
        }

        /// One update of `count` rectangles, as the encoder appended them.
        fn draw(&mut self, count: u16, rects: &[u8]) {
            let mut wire = crate::msg::update_header(count).to_vec();
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
                        for row in 0..usize::from(h) {
                            for col in 0..usize::from(w) {
                                let at = ((usize::from(y) + row) * self.w + usize::from(x) + col) * 4;
                                let from = (row * usize::from(w) + col) * 3;
                                self.canvas[at..at + 3].copy_from_slice(&[rgb[from + 2], rgb[from + 1], rgb[from]]);
                            }
                        }
                    }
                    TileBody::Start { stream, codec, frame } => {
                        self.starts += 1;
                        let codec = RegionCodec::of(codec).expect("a codec of the tiles");
                        self.codecs.push(codec);
                        assert!(!self.decoders.contains_key(&stream), "stream {stream} started while live");
                        let decoder = match codec {
                            RegionCodec::Vp9 => {
                                let header = wlshare_vp9::frame_header(&frame).expect("a VP9 frame");
                                assert!(header.keyframe && header.profile == 0, "a stream starts at a 4:2:0 keyframe");
                                Region::Vp9(wlshare_vp9::Decoder::new(1).unwrap())
                            }
                            RegionCodec::H264 => {
                                assert_eq!(frame[0], Fake::KEYFRAME, "a fake stream starts at its keyframe");
                                Region::Fake
                            }
                        };
                        self.decoders.insert(stream, (decoder, (x, y, w, h)));
                        self.region(stream, &frame);
                    }
                    TileBody::Frame { stream, frame } => {
                        self.frames += 1;
                        assert_eq!(self.decoders.get(&stream).map(|d| d.1), Some((x, y, w, h)), "a frame of a stream live over its rectangle");
                        match self.decoders[&stream].0 {
                            Region::Vp9(_) => assert!(!wlshare_vp9::frame_header(&frame).expect("a VP9 frame").keyframe, "a keyframe after the start"),
                            Region::Fake => assert_eq!(frame[0], Fake::FRAME, "a fake keyframe after the start"),
                        }
                        self.region(stream, &frame);
                    }
                    TileBody::End { stream } => {
                        self.ends += 1;
                        assert_eq!(self.decoders.remove(&stream).map(|d| d.1), Some((x, y, w, h)), "the end of a stream live over its rectangle");
                    }
                }
            }
        }

        fn region(&mut self, stream: u8, frame: &[u8]) {
            let (decoder, (x, y, w, h)) = self.decoders.get_mut(&stream).unwrap();
            let mut bgrx = vec![0; usize::from(*w) * usize::from(*h) * 4];
            match decoder {
                Region::Vp9(decoder) => {
                    let decoded = decoder.decode(frame).unwrap();
                    assert_eq!(decoded.size(), (u32::from(*w), u32::from(*h)));
                    decoded.write_bgrx(&mut bgrx, usize::from(*w) * 4).unwrap();
                }
                Region::Fake => {
                    assert_eq!(frame.len(), 1 + bgrx.len(), "a fake frame is the region's pixels");
                    bgrx.copy_from_slice(&frame[1..]);
                }
            }
            for row in 0..usize::from(*h) {
                let at = ((usize::from(*y) + row) * self.w + usize::from(*x)) * 4;
                self.canvas[at..at + usize::from(*w) * 4].copy_from_slice(&bgrx[row * usize::from(*w) * 4..][..usize::from(*w) * 4]);
            }
        }

        /// How many pixels differ from `pixels` in B, G or R.
        fn off(&self, pixels: &[u8]) -> usize {
            self.canvas.as_chunks::<4>().0.iter().zip(pixels.as_chunks::<4>().0).filter(|(a, b)| a[..3] != b[..3]).count()
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

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A region coder standing in for a hardware H.264 one: its frames are the
    /// region's pixels behind a byte that says keyframe or not, and it can be
    /// told to refuse every stream, or to fail one after so many frames, at
    /// its read or at its encode.
    struct Fake {
        refuse: bool,
        fail_after: Option<usize>,
        fail_read_after: Option<usize>,
        opened: usize,
    }

    impl Fake {
        const KEYFRAME: u8 = 0xC0;
        const FRAME: u8 = 0xDE;
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
        fn codec(&self) -> RegionCodec {
            RegionCodec::H264
        }

        fn open(&mut self, width: u16, height: u16, _quality: u8) -> Result<Box<dyn RegionStream>, RegionError> {
            if self.refuse {
                return Err("the device is gone".into());
            }
            self.opened += 1;
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
            out.extend_from_slice(&self.picture);
            self.frames += 1;
            Ok(true)
        }
    }

    /// Read and encode, and draw what came out.
    fn step(encoder: &mut TilesEncoder, page: &mut Page, pixels: &[u8], changed: Option<&[Rect]>, now: Duration) -> u16 {
        let stride = usize::from(encoder.size().0) * 4;
        encoder.read(pixels, stride, changed, now).unwrap();
        let mut out = Vec::new();
        let count = encoder.encode(now, &mut out).unwrap();
        page.draw(count, &out);
        count
    }

    /// Run the tick from `now` until nothing is owed, drawing what it sends;
    /// returns when it stopped.
    fn settle(encoder: &mut TilesEncoder, page: &mut Page, mut now: Duration) -> Duration {
        let mut out = Vec::new();
        while let Some(at) = encoder.tick_at() {
            now = now.max(at);
            out.clear();
            let count = encoder.encode(now, &mut out).unwrap();
            page.draw(count, &out);
            assert!(now < ms(60_000), "never settled");
        }
        now
    }

    #[test]
    fn the_quality_is_read_from_the_list_beside_the_encoding() {
        let quality = crate::vp9::ENCODING_VP9_QUALITY_BASE + 70;
        assert_eq!(crate::ENCODING_TILES, i32::from_be_bytes(*b"WLST"));
        assert_eq!(listed(&[crate::ENCODING_ZRLE, quality]), Ok(None));
        assert_eq!(listed(&[crate::ENCODING_TILES, quality, crate::ENCODING_ZRLE]), Ok(Some(70)));
        assert_eq!((RegionCodec::of(0), RegionCodec::of(1), RegionCodec::of(2)), (Some(RegionCodec::Vp9), Some(RegionCodec::H264), None));
        assert_eq!(listed(&[crate::ENCODING_TILES, crate::ENCODING_ZRLE]), Err(TilesListError::NoQuality));
        assert_eq!(listed(&[crate::ENCODING_TILES, crate::vp9::ENCODING_VP9_QUALITY_BASE + 101]), Err(TilesListError::NoQuality));
        assert_eq!(listed(&[crate::ENCODING_TILES, crate::ENCODING_VP9, quality]), Err(TilesListError::Both));
    }

    /// A first picture goes whole as PNG, a band at a time, exact; a palette
    /// where it has few colours and RGB where it has many, both read back.
    #[test]
    fn a_first_picture_is_sent_exact_in_bands() {
        let (w, h) = (200u16, 150u16);
        let mut pixels = noise(w, h, 1);
        // A flat left half, which a palette holds.
        for row in 0..usize::from(h) {
            pixels[row * usize::from(w) * 4..][..400].copy_from_slice(&[10, 20, 30, 0].repeat(100));
        }
        let mut encoder = TilesEncoder::with_threads(w, h, 80, 1, None).unwrap();
        let mut page = Page::new(w, h);
        let count = step(&mut encoder, &mut page, &pixels, Some(&[Rect { x: 5, y: 5, width: 1, height: 1 }]), ms(0));
        assert_eq!((count, page.pngs), (3, 3), "the whole picture in three bands, whatever the read was told");
        assert_eq!(page.off(&pixels), 0);
        assert!(encoder.tick_at().is_none(), "a picture sent exact owes nothing");
    }

    /// The damage is compared with what the client holds: a redraw of the same
    /// pixels sends nothing, and one tile that changed is one PNG of it.
    #[test]
    fn only_tiles_that_changed_are_sent() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 2);
        let mut encoder = TilesEncoder::with_threads(w, h, 80, 1, None).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let whole = [Rect { x: 0, y: 0, width: w, height: h }];
        assert_eq!(step(&mut encoder, &mut page, &pixels, Some(&whole), ms(100)), 0, "a redraw of what the client holds");
        paint(&mut pixels, w, Rect { x: 70, y: 70, width: 10, height: 10 }, 9);
        let bytes = page.bytes;
        assert_eq!(step(&mut encoder, &mut page, &pixels, Some(&whole), ms(200)), 1, "one tile changed");
        assert!(page.bytes - bytes < 64 * 64 * 3 + 200, "more than one tile was sent");
        assert_eq!(page.off(&pixels), 0);
        // The framebuffer's rows may be farther apart than the picture's.
        let stride = usize::from(w) * 4 + 64;
        let mut wide = vec![0xEE; stride * usize::from(h)];
        for row in 0..usize::from(h) {
            wide[row * stride..][..usize::from(w) * 4].copy_from_slice(&pixels[row * usize::from(w) * 4..][..usize::from(w) * 4]);
        }
        encoder.read(&wide, stride, Some(&whole), ms(300)).unwrap();
        assert_eq!(encoder.encode(ms(300), &mut Vec::new()).unwrap(), 0, "the same picture at another stride");
        assert!(matches!(encoder.read(&wide[..100], stride, None, ms(400)), Err(TilesError::Buffer { .. })));
    }

    /// What moves becomes a stream of its own, told only its damage, which
    /// ends once the motion stops, and every tile it carried is sent exact
    /// after its end: the picture ends exact, and owes nothing.
    #[test]
    fn what_moves_streams_and_settles_exact() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = noise(w, h, 3);
        let mut encoder = TilesEncoder::with_threads(w, h, 60, 2, None).unwrap();
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
        assert!(page.off(&pixels) > 0, "a 4:2:0 picture of noise is not exact");
        settle(&mut encoder, &mut page, ms(2000));
        assert_eq!(page.ends, 1, "the stream ended");
        assert_eq!(page.off(&pixels), 0, "the picture settled exact");
        assert!(page.decoders.is_empty());
    }

    /// A redraw of what a live stream carries, the pixels the client holds,
    /// codes no frame of it.
    #[test]
    fn a_redraw_inside_a_stream_codes_nothing() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 5);
        let mut encoder = TilesEncoder::with_threads(w, h, 60, 2, None).unwrap();
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
        let mut encoder = TilesEncoder::with_threads(w, h, 60, 2, None).unwrap();
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

    /// Play a video in the corner for `frames` frames of 50 ms from `from`.
    fn play(encoder: &mut TilesEncoder, page: &mut Page, pixels: &mut [u8], w: u16, from: u32, frames: u32) {
        let video = Rect { x: 0, y: 0, width: 256, height: 192 };
        for n in from..from + frames {
            paint(pixels, w, video, 500 + n);
            step(encoder, page, pixels, Some(&[video]), ms(u64::from(n) * 50));
        }
    }

    /// With a region coder held, a region streams in its codec, said in its
    /// START, and the stream is told its damage as a VP9 one is; the picture
    /// settles exact after it as any other.
    #[test]
    fn a_region_coder_codes_the_streams_in_its_codec() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 6);
        let mut encoder = TilesEncoder::with_threads(w, h, 60, 1, Some(Box::new(Fake { refuse: false, fail_after: None, fail_read_after: None, opened: 0 }))).unwrap();
        assert_eq!(encoder.codec(), RegionCodec::H264);
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        play(&mut encoder, &mut page, &mut pixels, w, 1, 20);
        assert_eq!((page.starts, page.ends, page.codecs.as_slice()), (1, 0, &[RegionCodec::H264][..]), "one H.264 stream, live");
        assert!(page.frames > 10, "{} frames of it", page.frames);
        assert_eq!(page.off(&pixels), 0, "the fake's frames are the pixels, so the page is exact while it moves");
        settle(&mut encoder, &mut page, ms(2000));
        assert_eq!(page.ends, 1);
        assert_eq!(page.off(&pixels), 0);
    }

    /// A coder that refuses a stream is given up: the region streams as VP9
    /// instead, from a START that says so, and the coder is not asked again.
    #[test]
    fn a_region_coder_that_refuses_is_given_up_for_vp9() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 7);
        let mut encoder = TilesEncoder::with_threads(w, h, 60, 1, Some(Box::new(Fake { refuse: true, fail_after: None, fail_read_after: None, opened: 0 }))).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        play(&mut encoder, &mut page, &mut pixels, w, 1, 20);
        assert_eq!((page.starts, page.codecs.as_slice()), (1, &[RegionCodec::Vp9][..]), "the stream is VP9");
        assert_eq!(encoder.codec(), RegionCodec::Vp9, "the coder was given up");
        settle(&mut encoder, &mut page, ms(2000));
        assert_eq!(page.off(&pixels), 0);
    }

    /// A coder that fails a stream midway ends that stream, with its END, and
    /// the region it carried comes back as a VP9 stream; the page is exact in
    /// the end.
    #[test]
    fn a_region_coder_that_fails_midway_ends_its_stream_and_vp9_takes_over() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 8);
        let mut encoder = TilesEncoder::with_threads(w, h, 60, 1, Some(Box::new(Fake { refuse: false, fail_after: Some(4), fail_read_after: None, opened: 0 }))).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        play(&mut encoder, &mut page, &mut pixels, w, 1, 40);
        assert_eq!(page.codecs, [RegionCodec::H264, RegionCodec::Vp9], "the H.264 stream, then VP9 over the region");
        assert_eq!((page.starts, page.ends), (2, 1), "the failed stream ended, the VP9 one is live");
        assert_eq!(encoder.codec(), RegionCodec::Vp9);
        settle(&mut encoder, &mut page, ms(3000));
        assert_eq!(page.ends, 2);
        assert_eq!(page.off(&pixels), 0);
        assert!(page.decoders.is_empty());
    }

    /// A coder that fails a stream at its read, before anything of the frame
    /// is written, is given up the same way, and the update's rectangle count
    /// stays what was written.
    #[test]
    fn a_region_coder_that_fails_a_read_ends_its_stream_and_vp9_takes_over() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 10);
        let mut encoder = TilesEncoder::with_threads(w, h, 60, 1, Some(Box::new(Fake { refuse: false, fail_after: None, fail_read_after: Some(4), opened: 0 }))).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        play(&mut encoder, &mut page, &mut pixels, w, 1, 40);
        assert_eq!(page.codecs, [RegionCodec::H264, RegionCodec::Vp9], "the H.264 stream, then VP9 over the region");
        assert_eq!((page.starts, page.ends), (2, 1), "the failed stream ended, the VP9 one is live");
        assert_eq!(encoder.codec(), RegionCodec::Vp9);
        settle(&mut encoder, &mut page, ms(3000));
        assert_eq!(page.ends, 2);
        assert_eq!(page.off(&pixels), 0);
        assert!(page.decoders.is_empty());
    }

}
