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
//! encoder takes a new constant quantizer on a running stream, and the update
//! that carries one counts as a keyframe for the walk. Every still is a lossy
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
//! ([`TilesEncoder::tick_at`]), at most 40 tiles at a time, and sent after the
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
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::time::Duration;

use thiserror::Error;

use wlshare_proto::tiles::{TileHeader, TilesPicture};

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

/// What one [`TilesEncoder::encode`] appended: how many rectangles, and
/// whether any of them is a stream's START or a still of what the client
/// held nothing of — the whole picture over again, which crosses a link as a
/// keyframe does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Update {
    pub rects: u16,
    pub keyframe: bool,
}

// ── The encoder ──────────────────────────────────────────────────────────────

/// One live region stream.
struct Live {
    /// The id its rectangles carry, which no other live stream has.
    id: u8,
    area: Area,
    cells: Vec<(u16, u16)>,
    stream: Box<dyn RegionStream>,
    /// The damage inside the region since its last frame, from its top-left.
    pending: Vec<Rect>,
    dirty: bool,
    /// Its START went out, and so its END is owed when it ends.
    started: bool,
    /// The dial moved under it: its next frame starts it over at a keyframe.
    restarting: bool,
    carried: bool,
    moving_at: Duration,
}

/// The rectangles of one update as they are appended, and how many.
struct Out<'a> {
    out: &'a mut Vec<u8>,
    count: usize,
    /// A START, or a still of what the client did not hold, was appended.
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
    retuned_at: Option<Duration>,
    /// The stills the reads since the last encode owe.
    stills: Vec<Area>,
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
            retuned_at: None,
            stills: Vec::new(),
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
        };
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
                live.restarting = true;
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
    /// encode, nothing is owed, and the next read is sent whole.
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
        self.stills.clear();
        self.restarting = true;
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
    /// of those no stream carries owed as stills, band by band. A tile it
    /// names whose pixels are what the client holds is no change to anything.
    fn damage(&mut self, pixels: &[u8], stride: usize, area: Area, now: Duration) {
        let mut changed: HashSet<(u16, u16)> = tiles_of(area).filter(|(_, part)| differs(&self.mirror, self.stride, pixels, stride, *part)).map(|(cell, _)| cell).collect();
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
            self.stills.extend(runs);
        }
    }

    /// Append to `out` the rectangles of what is owed at `now`, and return how
    /// many: the ENDs of streams [`Self::forget`] ended, the stills the reads
    /// owe, a round of the streams after a read — the regions laid out again
    /// and every stream the damage fell on coded — and, at its time
    /// ([`Self::tick_at`]), the streams whose motion stopped ended and the
    /// tiles still long enough sent at the ceiling. None when nothing is owed, which is
    /// no update to send.
    ///
    /// Tens of milliseconds for a desktop that changed all over. An error
    /// takes back the rectangle it was in, and leaves the ones before it.
    pub fn encode(&mut self, now: Duration, out: &mut Vec<u8>) -> Result<Update, TilesError> {
        let mut out = Out { out, count: 0, keyframe: false };
        for (id, area) in std::mem::take(&mut self.ended) {
            out.end(id, area);
        }
        let stills = std::mem::take(&mut self.stills);
        if !stills.is_empty() && std::mem::take(&mut self.restarting) {
            out.keyframe = true;
        }
        for area in stills {
            self.still(area, self.quality, now, &mut out)?;
        }
        if std::mem::take(&mut self.read) {
            self.round(now, &mut out)?;
        }
        if now >= self.next_tick {
            self.next_tick = now + CLEANUP_TICK;
            self.tick(now, &mut out)?;
        }
        let rects = u16::try_from(out.count).map_err(|_| TilesError::TooMany(out.count))?;
        Ok(Update { rects, keyframe: out.keyframe })
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

    /// Send `area` of the picture as a still coded at `quality`: on a held list
    /// a PNG, exact; on a walked one a lossy WebP at `quality`. A PNG, or a
    /// WebP at the ceiling, pays the debt of every tile it covers whole; a
    /// WebP below the ceiling owes every tile it touches a still at the
    /// ceiling from now.
    fn still(&mut self, area: Area, quality: u8, now: Duration, out: &mut Out) -> Result<(), TilesError> {
        // A run wider than a WebP goes as pieces cut where tiles meet, so that
        // each tile is in one of them whole as it was in the run.
        if self.adaptive && area.w() > WEBP_MAX_WIDTH {
            let mut left = area.left;
            loop {
                let right = (left / TILE + WEBP_MAX_WIDTH / TILE).saturating_mul(TILE).saturating_sub(1).min(area.right);
                self.still(Area { left, right, ..area }, quality, now, out)?;
                if right == area.right {
                    return Ok(());
                }
                left = right + 1;
            }
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
        let settled = !self.adaptive || quality >= self.ceiling;
        for (cell, _) in tiles_of(area) {
            if !settled {
                self.debts.insert(cell, now);
            } else if area.contains(self.cell_area(cell)) {
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
                live.pending.clear();
                headed = true;
                let header_at = out.header(live.area, header);
                Ok((live.stream.encode(!live.started, out.out)?, header_at))
            });
            match coded {
                Ok((true, header_at)) => {
                    out.length(from, header_at, header)?;
                    out.keyframe |= !live.started || live.restarting;
                    live.restarting = false;
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

    /// A stream of the region coder over `area`, every tile of it owed from now.
    fn build(&mut self, id: u8, area: Area, now: Duration) -> Result<Live, TilesError> {
        let stream = self.coder.open(area.w(), area.h(), self.quality).map_err(TilesError::Region)?;
        let cells: Vec<(u16, u16)> = tiles_of(area).map(|(cell, _)| cell).collect();
        for cell in &cells {
            self.debts.insert(*cell, now);
        }
        Ok(Live { id, area, cells, stream, pending: Vec::new(), dirty: true, started: false, restarting: false, carried: false, moving_at: now })
    }

    /// The streams whose motion stopped ended, and up to
    /// [`MAX_CLEANUPS_PER_TICK`] tiles still for [`CLEANUP_IDLE`] sent at
    /// the ceiling, oldest first, as runs along a row — none while the walk is
    /// coarse, since the link is behind on what it has, until the desktop has
    /// been still for [`COARSE_QUIET`].
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
        if self.coarse && now.saturating_sub(self.changed_at) < COARSE_QUIET {
            return Ok(());
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
                self.still(area, self.ceiling, now, out)?;
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
        pngs: usize,
        webps: usize,
        starts: usize,
        frames: usize,
        ends: usize,
        bytes: usize,
    }

    impl Page {
        fn new(w: u16, h: u16) -> Self {
            Self { w: usize::from(w), canvas: vec![0; usize::from(w) * usize::from(h) * 4], streams: HashMap::new(), pngs: 0, webps: 0, starts: 0, frames: 0, ends: 0, bytes: 0 }
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
        assert_eq!((first.rects, page.pngs), (3, 3), "the whole picture in three bands, whatever the read was told");
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
        assert_eq!((lossy.rects, lossy.keyframe, page.webps), (1, false, 3), "one tile that changed, at the walk's dial, no keyframe");
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

    /// A dial move restarts each live stream at its next frame, which is
    /// when the update counts as a keyframe: not an update before it that
    /// codes none of them.
    #[test]
    fn a_dial_move_is_a_keyframe_when_its_stream_is_coded() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = noise(w, h, 7);
        let mut encoder = TilesEncoder::new(w, h, walked(60), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let video = Rect { x: 0, y: 0, width: 192, height: 128 };
        for n in 1..=12u32 {
            paint(&mut pixels, w, video, 500 + n);
            step(&mut encoder, &mut page, &pixels, Some(&[video]), ms(u64::from(n) * 50));
        }
        assert_eq!((page.starts, page.ends), (1, 0), "a stream, live");
        encoder.set_pace(40, false);
        let elsewhere = Rect { x: 512, y: 320, width: 64, height: 64 };
        paint(&mut pixels, w, elsewhere, 900);
        let other = update(&mut encoder, &mut page, &pixels, Some(&[elsewhere]), ms(650));
        assert!(other.rects > 0 && !other.keyframe, "an update that codes no stream: {other:?}");
        paint(&mut pixels, w, video, 600);
        let restarted = update(&mut encoder, &mut page, &pixels, Some(&[video]), ms(700));
        assert!(restarted.keyframe, "the stream's restart: {restarted:?}");
        paint(&mut pixels, w, video, 601);
        let next = update(&mut encoder, &mut page, &pixels, Some(&[video]), ms(750));
        assert!(!next.keyframe, "the frame after it: {next:?}");
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
}
