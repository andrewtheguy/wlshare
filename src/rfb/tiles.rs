//! The tiles encoding's server side: the desktop as lossless WebP tiles where it
//! is still, and a video stream of its own for each region that moves, for a
//! client on a link with room to spare — or, on a list whose dial walks, every
//! tile a RemoteFX Progressive tile, still or moving, and no stream, for a
//! client that wants the walk more than a picture that ends lossless — or, on
//! a list that asks for it lossless, every tile that changed exact as it is
//! sent, planar where it is still and a median predictor's residuals where
//! it moves ([`planar`], [`median`]).
//!
//! The encoding — the rectangles and their kinds — is
//! [`wlshare_proto::tiles`]'s. It sits beside [`crate::rfb::video`]'s rather than in
//! place of it: the video encoding, one H.264 stream over the whole desktop
//! paced to the link, is the picture for a desktop that moves, and this one
//! spends bytes the link has for less of the encoder's time and a picture that
//! ends lossless. Its rules are the zDesk gateway's Motion's, brought here because
//! only here are the compositor's pixels and damage coded once. A region's
//! stream is H.264, by the host's hardware encoder or by libx264.
//!
//! **The dial walks, where the list lets it.** wlshare's dial
//! ([`crate::rfb::QUALITY`]) is a ceiling, and the session's walk of the link ([`crate::walk`]) moves
//! the encoder below it ([`TilesEncoder::set_pace`]). A list whose dial walks is progressive: every tile, still or
//! moving, a RemoteFX Progressive tile ([`progressive`],
//! [`wlshare_proto::tiles::KIND_PROGRESSIVE`]), and no stream. A tile's first
//! pass is at 75 while the walk stands at the ceiling and is not coarse, at
//! 50 below it, and at a stage coarser still once the dial is under
//! [`PROGRESSIVE_COARSE_BELOW`]; or whole, where that costs little more.
//! Nothing is owed a lossless picture: a first pass below the full stage is
//! owed its upgrades, and the debts are paid while the walk is not slowed: a link that
//! is behind is not handed finer pictures of what it has already seen. An
//! update of the tick alone, what the desktop stopping owes, is no verdict
//! for the walk ([`Update`]). A list that holds the dial never walks: every
//! still is a lossless WebP, and every region that moves a stream at the dial.
//!
//! **Which tiles move.** The desktop is in 64×64 tiles from its top-left. The
//! damage a read is told of is compared, tile by tile, with what the client
//! was sent, so a redraw of what it holds sends nothing. A tile is moving when
//! it changed in 4 of the last 8 tenths of a second. Moving tiles join into
//! 4-connected components; one under 5 tiles is left to WebP, and so is one
//! whose stills came to under 256 KiB over those 8 tenths, a spinner, a
//! progress bar or a line of text that slides, which cost little lossless and
//! are no video unless they join one that is; boxes merge
//! unless the merged box would be more than twice the moving tiles, and at
//! most 4 regions stream at once. The regions are laid out again at most every
//! 500 ms: one that grows starts another, one with no motion for a second
//! ends, and one whose motion has kept for two seconds to a box of at most
//! half of it is judged again, some of what moves coded to see: where that
//! would be dear as stills it starts another over that box, and otherwise it
//! ends as one with no motion does, what moves in it stills from then; less
//! of a difference keeps its stream. A region one large change began, the
//! box around all of it, is kept past its second only for what moves over
//! half of it and would be dear as stills, and ends otherwise, for regions of
//! what moves in it where that is dear.
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
//! then a few bytes a tile. A progressive tile stored below the full stage
//! is owed it when drawn, as a copy of one is; settled by its upgrades, it is
//! stored again in its slot, settled. A picture that starts over
//! ([`TilesEncoder::forget`]) keeps the cache, as the client does, so a
//! whole picture of tiles it was shown is a draw of a slot for each: an
//! output switched back to, a new size, another dial. A tile a walked list
//! stored is not lossless, so a held list draws none of them until it is stored
//! again ([`TilesEncoder::set_picture`]). The client says when its cache is
//! gone ([`TilesEncoder::forget_cache`]), and a new size hands the record to
//! the encoder made for it ([`TilesEncoder::keep_cache`]).
//!
//! **What a stream codes.** A region is an encoder over its rectangle alone,
//! told the damage inside it as the video encoding's whole-desktop stream is, so
//! the encoder's time follows the area that moves. It is 4:2:0 at wlshare's
//! dial, held there: what a stream loses of a picture lasts
//! only while the region moves, since it settles. The encoder is the
//! [`H264Coder`] the daemon hands this one, its host's H.264 encoder, on a
//! held list alone: the
//! hardware's, where a region costs the processor its conversion alone, or
//! else libx264 (`src/h264/x264.rs`). A coder that refuses a stream, or
//! fails one midway, fails the encode: a region has no other codec.
//!
//! **A change past an update's stills goes as video.** On a held list an
//! update carries at most [`HELD_BYTES`] of stills where the read moved
//! nothing: a change past that is a region over the box around the whole of
//! it, its START in the same update after the stills, which ends as any
//! region once nothing in it has moved for a second, every tile of it owed
//! its still ([`TilesEncoder::held_stills`]). A picture that
//! starts over — an encoder's first, or one a client asks for whole
//! ([`TilesEncoder::forget`]) — goes whole at once, in bands of 64 rows, or
//! as that region where it is past the budget, as the first frames of a
//! video are, their region from the first frame rather than once the churn
//! says moving; from
//! then on a band is sent as the runs of its changed tiles along each row.
//! What a scroll or a drag uncovers is stills whatever its size, and so is
//! the rest where the coder refuses the stream or every stream is taken.
//!
//! **It settles.** Every tile a stream carried is owed a WebP, and every
//! progressive tile below the full stage its next upgrade, once it has been
//! still for half a second, looked for every quarter of a second
//! ([`TilesEncoder::tick_at`]), the coarsest and longest owed first, at most
//! 320 tiles at a time. On a held list a look keeps to the link's pace, the
//! most the desktop's own changes were found sending over a second since
//! something was last owed, and eases in to it: its rate rises to the pace
//! over the second after the settle began paying, its share of the look's
//! interval at most 128 KiB and at least 32 KiB a quarter of a second, the
//! looks sooner than that where the pace has 128 KiB going in less, never
//! one on the heels of the last ([`TilesEncoder::paced_budget`]), and a run
//! cut to what is left of the look's bytes by what its tiles are taken to
//! cost ([`TilesEncoder::tick`]). So a desktop that stops settles at no
//! more than the rate its motion had of the link, with no step up to it. On a walked list the
//! look follows the walk: 256 KiB at the ceiling; below it, where the link
//! is short of bytes, 8 KiB while the desktop moves and what the link was
//! found to take once it is still, a look cut short owed again at the next
//! update's turn ([`TilesEncoder::delivered`]) — and sent after the stream's
//! END, so a
//! desktop that stops is lossless on a held list and at the full stage on a
//! walked one.
//!
//! The measurements these rules were chosen by are the daemon crate's
//! `examples/motion-bench.rs`, which plays the kept captures through this
//! encoder and through the video encoding's, both on the daemon's H.264, and
//! `examples/tiles-walk-sim.rs`, which walks the progressive tiles over a
//! slow link.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};
use std::time::Duration;

use thiserror::Error;
use xxhash_rust::xxh3::xxh3_64;

use wlshare_proto::tiles::{CACHE_SLOTS, TileHeader, TilesPicture};

use crate::rfb::{H264Coder, H264Error, H264Stream, Rect, webp};

/// Why a picture could not be read or encoded.
#[derive(Debug, Error)]
pub enum TilesError {
    #[error("a {0}x{1} desktop has no pixels")]
    Empty(u16, u16),
    /// The region coder refused a stream, or a stream its pixels or its frame.
    #[error("coding a region: {0}")]
    Region(H264Error),
    #[error("coding a still: {0}")]
    Still(#[from] webp::WebpError),
    #[error("coding a lossless run: {0}")]
    Lossless(std::io::Error),
    #[error("a {width}x{height} picture at a stride of {stride} does not fit in {bytes} bytes")]
    Buffer {
        width: u16,
        height: u16,
        stride: usize,
        bytes: usize,
    },
    #[error("a rectangle of {0} bytes is over the {max}-byte ceiling", max = wlshare_proto::MAX_BODY)]
    TooLong(usize),
    #[error("an update of {0} rectangles is more than one can count")]
    TooMany(usize),
}

// ── Geometry ─────────────────────────────────────────────────────────────────

/// A tile's side, ZRLE's, from the desktop's top-left: the encoding's.
const TILE: u16 = wlshare_proto::tiles::TILE;

use rayon::prelude::*;

use super::{median, planar, progressive};

/// A progressive first pass whole costs at most this many times the one at
/// its stage.
const PROGRESSIVE_SLACK: f32 = 1.25;

/// The dial below which a progressive first pass is coded at the stage
/// coarser than 50 (measured: over the mixed session at 5 Mbit/s, 70 and 60
/// alike sent 7% fewer bytes than 50 alone and showed a change 11% sooner,
/// 0.8 dB coarser on the mean; 70 the window's sooner too).
const PROGRESSIVE_COARSE_BELOW: u8 = 70;

/// The fewest first passes an update codes ahead on the pool
/// ([`TilesEncoder::precode`]); fewer are coded as they are sent.
const PRECODE_LEAST: usize = 4;

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
        Self {
            left: 0,
            top: 0,
            right: w - 1,
            bottom: h - 1,
        }
    }

    /// `r` on a `w`×`h` desktop, clipped to it; `None` for one with nothing inside.
    fn clipped(r: Rect, (w, h): (u16, u16)) -> Option<Self> {
        let right = (u32::from(r.x) + u32::from(r.width)).min(u32::from(w));
        let bottom = (u32::from(r.y) + u32::from(r.height)).min(u32::from(h));
        (u32::from(r.x) < right && u32::from(r.y) < bottom).then(|| Self {
            left: r.x,
            top: r.y,
            right: (right - 1) as u16,
            bottom: (bottom - 1) as u16,
        })
    }

    fn w(self) -> u16 {
        self.right - self.left + 1
    }

    fn h(self) -> u16 {
        self.bottom - self.top + 1
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let r = Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        };
        (r.left <= r.right && r.top <= r.bottom).then_some(r)
    }

    fn contains(self, other: Self) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && other.right <= self.right
            && other.bottom <= self.bottom
    }

    /// This rectangle as a stream takes one, from `origin`'s top-left.
    fn within(self, origin: Self) -> Rect {
        Rect {
            x: self.left - origin.left,
            y: self.top - origin.top,
            width: self.w(),
            height: self.h(),
        }
    }
}

/// The tiles `area` touches, each with the part of it inside that tile, row by row.
fn tiles_of(area: Area) -> impl Iterator<Item = ((u16, u16), Area)> {
    (area.top / TILE..=area.bottom / TILE).flat_map(move |row| {
        (area.left / TILE..=area.right / TILE).map(move |col| {
            let tile = Area {
                left: col * TILE,
                top: row * TILE,
                right: col.saturating_mul(TILE).saturating_add(TILE - 1),
                bottom: row.saturating_mul(TILE).saturating_add(TILE - 1),
            };
            (
                (col, row),
                tile.intersect(area).expect("a tile the rectangle touches"),
            )
        })
    })
}

/// The tiles of `areas` that are not among `carried`, as runs along a row,
/// none of them across two of the areas.
fn runs_outside(areas: &[Area], carried: &HashSet<(u16, u16)>) -> Vec<Area> {
    let mut all = Vec::new();
    for area in areas {
        let mut runs: Vec<Area> = Vec::new();
        for (_, part) in tiles_of(*area).filter(|(cell, _)| !carried.contains(cell)) {
            match runs.last_mut() {
                Some(run) if run.top == part.top && run.right.checked_add(1) == Some(part.left) => {
                    run.right = part.right
                }
                _ => runs.push(part),
            }
        }
        all.extend(runs);
    }
    all
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
        Self {
            c0: cell.0,
            r0: cell.1,
            c1: cell.0,
            r1: cell.1,
        }
    }

    fn area(&self) -> u32 {
        u32::from(self.c1 - self.c0 + 1) * u32::from(self.r1 - self.r0 + 1)
    }

    fn union(&self, other: &Self) -> Self {
        Self {
            c0: self.c0.min(other.c0),
            r0: self.r0.min(other.r0),
            c1: self.c1.max(other.c1),
            r1: self.r1.max(other.r1),
        }
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
        Some(Area {
            left,
            top,
            right: self
                .c1
                .saturating_mul(TILE)
                .saturating_add(TILE - 1)
                .min(w - 1),
            bottom: self
                .r1
                .saturating_mul(TILE)
                .saturating_add(TILE - 1)
                .min(h - 1),
        })
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
/// How long a stream has to start: one whose coder has made no frame of it
/// by then ends, whatever moves in it, and its rectangle goes as stills.
const START_WITHIN: Duration = Duration::from_millis(1000);
/// How long no region is laid out after a stream ran out of
/// [`START_WITHIN`]: the coder that made no frame of it would make none of
/// the next, so what changes goes as stills, whatever they come to, until
/// it is tried again.
const START_RETRY: Duration = Duration::from_millis(5000);
/// The most tiles a merge may cover, as a multiple of the moving tiles it joins.
const MERGE_WASTE: u32 = 2;
/// How long what moves in a region has kept to a smaller box before the
/// region is laid out again over that box: a page scrolled with a video on
/// it is one region while it scrolls, and the video's alone this long after.
/// A smaller box is one of at most half the region's tiles, the waste a
/// merge's box may have ([`MERGE_WASTE`]): less of a difference is not worth
/// the keyframe, and a video whose motion narrows and widens, as most do,
/// would be started again each time.
const SHRINK_AFTER: Duration = Duration::from_millis(2000);
/// The fewest moving tiles a component needs to be worth a stream at scale 1:
/// at another, the same area of the layout, 64×64 of its pixels a tile here
/// ([`stream_cells`]).
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
/// The most tiles a tick codes.
const SETTLE_TILES: usize = 320;
/// The bytes after which a tick sends no more while a walked list's walk
/// stands at the ceiling, and at most below it, where what it may send on a
/// quiet desktop walks between this and [`SETTLE_BYTES_LEAST`] on how long
/// the updates that carried a settle took to cross
/// ([`TilesEncoder::delivered`]).
const SETTLE_BYTES: usize = 256 * 1024;
/// A held list's stills: the bytes of them an update carries at once where
/// nothing moved, a change past it going as video
/// ([`TilesEncoder::held_stills`]). The least of the three measured
/// — 128, 256 and 512 KiB — that flattens what the link sees: a change
/// growing under it is this much of stills a frame until the churn calls it
/// moving, at 25 frames a second 26 Mbit/s where 256 KiB was 52. Measured
/// while a still was a PNG, when its one cost was a page of text at 1080p,
/// 230 KiB, which started as a region rather than lossless at once; as a
/// WebP that page is 96 kB and within it (`docs/tiles-performance.md`).
const HELD_BYTES: usize = 128 * 1024;
/// The most a held list's tick sends at any pace, and what the pace has
/// going between two ticks where that is sooner than [`CLEANUP_TICK`]
/// ([`TilesEncoder::tick_interval`]): its own figure, apart from the stills
/// an update carries, which decide how a picture starts where this decides
/// how fast one settles (measured in `docs/tiles-performance.md`).
const SETTLE_MOST: usize = 128 * 1024;
/// The fewest bytes a tick may send before it stops, and what it starts at:
/// 13 ms of a 5 Mbit/s link.
const SETTLE_BYTES_LEAST: usize = 8 * 1024;
/// An update that crossed within this had the link to itself: the next tick
/// may send twice as much. The walk's own word for a link that is clear.
const SETTLE_QUICK: Duration = Duration::from_millis(30);
/// An update that took this long or more to cross queued on the link: the
/// next tick sends half as much.
const SETTLE_LATE: Duration = Duration::from_millis(60);
/// How far back the updates go whose bytes are what the link has been
/// carrying, which a held list's settle keeps to ([`TilesEncoder::pace`]).
const PACE_WINDOW: Duration = Duration::from_millis(1000);
/// The fewest bytes a held list's tick a [`CLEANUP_TICK`] after the last
/// sends, whatever the pace, and a sooner one its share of that: 1 Mbit/s,
/// so what little motion owes is paid within a tick or two.
const SETTLE_FLOOR: usize = 32 * 1024;
/// How long a held list's settle takes to reach its pace from when it began
/// paying: its rate rises with the time since, so what the link is handed
/// eases in ([`TilesEncoder::paced_budget`]).
const SETTLE_EASE: Duration = Duration::from_millis(1000);
/// What a held list's settle takes a pixel that differs from the one to its
/// left to cost, in bytes, before a piece has been coded: a photograph's,
/// about. A tick cuts its runs by what the last piece cost one
/// ([`TilesEncoder::tick`]).
const SETTLE_RATE_GUESS: f64 = 2.0;
/// What a tile costs whatever is in it, in bytes: its share of a WebP's
/// header, its store, or its draw from the cache.
const SETTLE_TILE_LEAST: f64 = 32.0;
/// The least time between two of a held list's ticks, where the pace would
/// have one of [`SETTLE_MOST`] sooner than [`CLEANUP_TICK`].
const SETTLE_TICK_LEAST: Duration = Duration::from_millis(50);
/// The most tiles along a row one WebP holds, its longest side
/// ([`webp::MAX_SIDE`]) in whole tiles.
const WEBP_TILES: usize = (webp::MAX_SIDE / TILE) as usize;
/// What a moving component's stills must have come to over the churn's
/// window, [`CHURN_WINDOW`] slots, to be worth a stream: about 2.6 Mbit/s of
/// them. Twice what the widgets of a desk came to at their most, a line of
/// text sliding sideways at scale 2, and half of the least of what a video
/// did, 320 by 180 points at scale 1; and half of it keeps a stream on what
/// moves under a region one large change began, where it is guessed
/// ([`TilesEncoder::probe`]).
const MOVING_BYTES: u64 = 2 * HELD_BYTES as u64;

/// The most tiles of a component [`TilesEncoder::probe`] codes.
const PROBE_TILES: usize = 16;

/// The rows a report of damage is cut into.
const BAND_ROWS: u16 = 64;

/// One tile's history: bit 0 the slot it last changed in, shifted a place a
/// slot, and the bytes its stills came to in each of those slots, the last
/// first.
#[derive(Clone, Copy)]
struct ChurnCell {
    history: u8,
    spent: [u32; CHURN_WINDOW as usize],
    last_slot: u64,
}

impl ChurnCell {
    /// The history as of `slot`, what is older than the window dropped.
    fn advance(&mut self, slot: u64) {
        let elapsed = slot.saturating_sub(self.last_slot);
        if elapsed >= CHURN_WINDOW {
            self.history = 0;
            self.spent = [0; CHURN_WINDOW as usize];
        } else if elapsed > 0 {
            self.history <<= elapsed;
            self.spent.rotate_right(elapsed as usize);
            self.spent[..elapsed as usize].fill(0);
        }
        self.last_slot = slot;
    }
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
        let cell = self.churn.entry(key).or_insert(ChurnCell {
            history: 0,
            spent: [0; CHURN_WINDOW as usize],
            last_slot: slot,
        });
        cell.advance(slot);
        cell.history |= 1;
    }

    /// `bytes` of a still that carried `key` at `now`, where it has changed
    /// within the window: what its changes cost as stills.
    fn spend(&mut self, key: (u16, u16), now: Duration, bytes: u32) {
        let (Some(origin), Some(cell)) = (self.origin, self.churn.get_mut(&key)) else {
            return;
        };
        cell.advance(Self::slot(origin, now));
        cell.spent[0] = cell.spent[0].saturating_add(bytes);
    }

    /// The bytes `key`'s stills came to over the window, as of the last
    /// [`Self::moving`].
    fn spent(&self, key: (u16, u16)) -> u32 {
        self.churn
            .get(&key)
            .map_or(0, |cell| cell.spent.iter().sum())
    }

    fn moving(&mut self, now: Duration) -> Vec<(u16, u16)> {
        let Some(origin) = self.origin else {
            return Vec::new();
        };
        let slot = Self::slot(origin, now);
        let mut moving = Vec::new();
        self.churn.retain(|key, cell| {
            cell.advance(slot);
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
        let (Some(origin), Some(cell)) = (self.origin, self.churn.get(&key)) else {
            return 0;
        };
        let elapsed = Self::slot(origin, now).saturating_sub(cell.last_slot);
        if elapsed >= CHURN_WINDOW {
            0
        } else {
            (cell.history << elapsed).count_ones()
        }
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

/// The fewest moving tiles a component needs to be worth a stream on an
/// output drawn at `scale`: [`MIN_STREAM_CELLS`]'s area of the layout, so a
/// spinner or a progress bar drawn at 2 covers four times the tiles it does at
/// 1 and is no more a video for it; and never fewer than at 1, where fewer
/// tiles than that are not worth a stream's keyframe and encoder.
fn stream_cells(scale: f64) -> u32 {
    ((f64::from(MIN_STREAM_CELLS) * scale * scale).ceil() as u32).max(MIN_STREAM_CELLS)
}

/// The 4-connected components of `cells` of at least `least` tiles that are
/// `worth` a stream, asked of each one's tiles.
fn components(
    cells: &[(u16, u16)],
    least: u32,
    mut worth: impl FnMut(&[(u16, u16)]) -> bool,
) -> Vec<Component> {
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
        let mut members = Vec::new();
        let mut stack = vec![start];
        while let Some((c, r)) = stack.pop() {
            members.push((c, r));
            bbox = bbox.union(&CellBox::of((c, r)));
            for next in [
                (c.wrapping_sub(1), r),
                (c + 1, r),
                (c, r.wrapping_sub(1)),
                (c, r + 1),
            ] {
                if remaining.contains(&next) && seen.insert(next) {
                    stack.push(next);
                }
            }
        }
        let moving = members.len() as u32;
        if moving >= least && worth(&members) {
            components.push(Component { bbox, moving });
        }
    }
    components
}

type OverlapQueue = BinaryHeap<Reverse<(usize, usize, u32, u32)>>;
type MergeQueue = BinaryHeap<Reverse<(u32, usize, usize, u32, u32)>>;

/// At most `max` pairwise disjoint boxes from the components: overlaps first,
/// merged unless the union is past the waste, then the cheapest merges while
/// there are too many, then the smallest dropped.
fn merge(components: Vec<Component>, max: usize) -> Vec<Component> {
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
            let Some(Reverse((i, j, gi, gj))) = overlaps.pop() else {
                break None;
            };
            if current(&components, &generations, i, j, gi, gj) {
                break Some((i, j));
            }
        };
        if let Some((i, j)) = overlapping {
            let (left, right) = (
                components[i].expect("current"),
                components[j].expect("current"),
            );
            let merged = left.bbox.union(&right.bbox);
            let moving = left.moving + right.moving;
            let nested = merged == left.bbox || merged == right.bbox;
            if nested || merged.area() <= MERGE_WASTE * moving {
                components[j] = None;
                components[i] = Some(Component {
                    bbox: merged,
                    moving,
                });
                generations[i] += 1;
                active -= 1;
                queue_component(i, &components, &generations, &mut overlaps, &mut merges);
            } else {
                let loser = if (right.moving, right.bbox.area()) < (left.moving, left.bbox.area()) {
                    j
                } else {
                    i
                };
                components[loser] = None;
                active -= 1;
            }
            continue;
        }
        if active <= max {
            break;
        }
        let best = loop {
            let Some(Reverse((_, i, j, gi, gj))) = merges.pop() else {
                break None;
            };
            if current(&components, &generations, i, j, gi, gj) {
                break Some((i, j));
            }
        };
        match best {
            Some((i, j)) => {
                let left = components[i].expect("current");
                let right = components[j].take().expect("current");
                components[i] = Some(Component {
                    bbox: left.bbox.union(&right.bbox),
                    moving: left.moving + right.moving,
                });
                generations[i] += 1;
                active -= 1;
                queue_component(i, &components, &generations, &mut overlaps, &mut merges);
            }
            None => {
                let smallest = components
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| c.map(|c| (i, c)))
                    .min_by_key(|(_, c)| (c.moving, c.bbox.area()))
                    .map(|(i, _)| i)
                    .expect("more than max");
                components[smallest] = None;
                active -= 1;
            }
        }
    }
    components.into_iter().flatten().collect()
}

fn queue_component(
    at: usize,
    components: &[Option<Component>],
    generations: &[u32],
    overlaps: &mut OverlapQueue,
    merges: &mut MergeQueue,
) {
    for other in 0..components.len() {
        if other != at && components[other].is_some() {
            queue_pair(
                at.min(other),
                at.max(other),
                components,
                generations,
                overlaps,
                merges,
            );
        }
    }
}

fn queue_pair(
    i: usize,
    j: usize,
    components: &[Option<Component>],
    generations: &[u32],
    overlaps: &mut OverlapQueue,
    merges: &mut MergeQueue,
) {
    let (left, right) = (components[i].expect("live"), components[j].expect("live"));
    if left.bbox.overlaps(&right.bbox) {
        overlaps.push(Reverse((i, j, generations[i], generations[j])));
        return;
    }
    let merged = left.bbox.union(&right.bbox);
    if merged.area() <= MERGE_WASTE * (left.moving + right.moving) {
        merges.push(Reverse((
            merged.area() - left.bbox.area() - right.bbox.area(),
            i,
            j,
            generations[i],
            generations[j],
        )));
    }
}

fn current(
    components: &[Option<Component>],
    generations: &[u32],
    i: usize,
    j: usize,
    gi: u32,
    gj: u32,
) -> bool {
    components[i].is_some()
        && components[j].is_some()
        && generations[i] == gi
        && generations[j] == gj
}

// ── The pixels ───────────────────────────────────────────────────────────────

/// Whether `area` of `a` and `b`, two `B, G, R, X` pictures, differ.
fn differs(a: &[u8], a_stride: usize, b: &[u8], b_stride: usize, area: Area) -> bool {
    let (from, to) = (
        usize::from(area.left) * 4,
        (usize::from(area.right) + 1) * 4,
    );
    (area.top..=area.bottom).any(|y| {
        let (ra, rb) = (usize::from(y) * a_stride, usize::from(y) * b_stride);
        a[ra + from..ra + to] != b[rb + from..rb + to]
    })
}

/// Copy `area` of `from` into `to`, two `B, G, R, X` pictures.
fn blit(to: &mut [u8], to_stride: usize, from: &[u8], from_stride: usize, area: Area) {
    let (left, right) = (
        usize::from(area.left) * 4,
        (usize::from(area.right) + 1) * 4,
    );
    for y in area.top..=area.bottom {
        let (rt, rf) = (usize::from(y) * to_stride, usize::from(y) * from_stride);
        to[rt + left..rt + right].copy_from_slice(&from[rf + left..rf + right]);
    }
}

/// A whole tile's pixels as a cache keys them: a hash of `area` of a
/// `B, G, R, X` picture, and whether it is one colour, which is never stored.
/// Its rows are put together and hashed at once: xxh3 fed a row at a time
/// spent as long again on its buffering as on the hash.
fn tile_key(pixels: &[u8], stride: usize, area: Area) -> (u64, bool) {
    let (from, to) = (
        usize::from(area.left) * 4,
        (usize::from(area.right) + 1) * 4,
    );
    let mut tile = [0u8; TILE as usize * TILE as usize * 4];
    let first = &pixels[usize::from(area.top) * stride + from..usize::from(area.top) * stride + to];
    let mut flat = first
        .as_chunks::<4>()
        .0
        .iter()
        .all(|px| px == first.first_chunk::<4>().expect("a pixel"));
    for (y, line) in (area.top..=area.bottom).zip(tile.chunks_exact_mut(to - from)) {
        let row = &pixels[usize::from(y) * stride + from..usize::from(y) * stride + to];
        line.copy_from_slice(row);
        flat &= row == first;
    }
    (xxh3_64(&tile[..(to - from) * usize::from(area.h())]), flat)
}

/// What one [`TilesEncoder::encode`] appended: how many rectangles; whether
/// any of them is a still of what the client held nothing of — the whole
/// picture over again, which crosses a link as a keyframe does; whether all
/// of them are the tick's ([`TilesEncoder::tick_at`]), what was owed of
/// tiles already sent and none of what a read brought; and whether the
/// desktop had stopped then, still for half a second ([`CLEANUP_IDLE`]):
/// what its stopping owes, where a tick between a typist's keys is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Update {
    pub rects: u16,
    pub keyframe: bool,
    pub tick: bool,
    pub stopped: bool,
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
/// The most offsets the scan found that are checked against every probe.
const MAX_CHECKED: usize = 64;
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
/// at `src`, and whether it is owed what settles it: where its source is. On
/// a walked list a copy from a settled source is settled too, as a cached
/// tile stored at the full stage is: what it draws is the full stage's
/// pixels, though the client holds no progressive coefficients of them where
/// they now are, so the next change to one of its tiles is a first pass that
/// is no difference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CopyRect {
    dst: Area,
    src: (u16, u16),
    owed: bool,
}

impl CopyRect {
    /// The source as a rectangle of the desktop.
    fn source(self) -> Area {
        Area {
            left: self.src.0,
            top: self.src.1,
            right: self.src.0 + (self.dst.right - self.dst.left),
            bottom: self.src.1 + (self.dst.bottom - self.dst.top),
        }
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
    (x..x + PROBE).fold(0, |hash, x| {
        hash.wrapping_mul(HASH_BASE)
            .wrapping_add(u64::from(pixel(pixels, stride, x, y)))
    })
}

// ── What the client keeps ────────────────────────────────────────────────────

/// One slot of the client's cache of tiles, as the encoder records it.
#[derive(Clone, Copy)]
struct Kept {
    hash: u64,
    /// Stored lossless, or at the full stage: a tile drawn from it is owed nothing.
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

    /// No slot is settled: what each keeps is owed a picture finer than it
    /// was stored as, and is stored again once settled.
    fn unsettle(&mut self) {
        for kept in self.slots.values_mut() {
            kept.settled = false;
        }
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
                let &(used, slot) = self
                    .by_use
                    .iter()
                    .next()
                    .expect("a full cache has a least recently used slot");
                self.by_use.remove(&(used, slot));
                let evicted = self.slots.remove(&slot).expect("the slot it keeps");
                self.by_hash.remove(&evicted.hash);
                slot
            }
        };
        if let Some(kept) = self.slots.get(&slot) {
            self.by_use.remove(&(kept.used, slot));
        }
        self.slots.insert(
            slot,
            Kept {
                hash,
                settled,
                used: self.stamp,
            },
        );
        self.by_hash.insert(hash, slot);
        self.by_use.insert((self.stamp, slot));
        slot
    }
}

// ── The encoder ──────────────────────────────────────────────────────────────

/// One tile of a still's run: the tile, the part of it the still covers, and
/// where that is the whole tile, its key for the cache ([`tile_key`]).
type StillTile = ((u16, u16), Area, Option<(u64, bool)>);

/// Why a region was laid out, which [`TilesEncoder::set_debug`]'s outline
/// of it says by its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cause {
    /// Its tiles kept changing ([`TilesEncoder::retune`]): magenta.
    Moving,
    /// One update's change was more than its stills
    /// ([`TilesEncoder::held_stills`]): orange.
    Burst,
}

impl Cause {
    /// The outline's colour, `B, G, R, X`.
    fn colour(self) -> [u8; 4] {
        match self {
            Self::Moving => [255, 0, 255, 255],
            Self::Burst => [0, 165, 255, 255],
        }
    }
}

/// The four strips, `thick` pixels wide, along the inside of `area`'s edges:
/// the outline [`TilesEncoder::set_debug`] draws on a region.
fn outline(area: Area, thick: u16) -> [Area; 4] {
    let thick = thick.clamp(1, area.w().min(area.h())) - 1;
    [
        Area {
            bottom: area.top + thick,
            ..area
        },
        Area {
            top: area.bottom - thick,
            ..area
        },
        Area {
            right: area.left + thick,
            ..area
        },
        Area {
            left: area.right - thick,
            ..area
        },
    ]
}

/// One live region stream.
struct Live {
    /// The id its rectangles carry, which no other live stream has.
    id: u8,
    area: Area,
    cells: Vec<(u16, u16)>,
    stream: Box<dyn H264Stream>,
    /// The damage inside the region no frame has carried, from its top-left.
    pending: Vec<Rect>,
    dirty: bool,
    /// Its START went out, and so its END is owed when it ends.
    started: bool,
    carried: bool,
    moving_at: Duration,
    cause: Cause,
    /// When it was laid out.
    since: Duration,
    /// What moves under it is dear as stills: by the stills counted before
    /// it was laid out, or by its tiles coded since
    /// ([`TilesEncoder::probe`]), which is not asked of it again. Not yet
    /// of a region one change began, nor of one whose motion has kept to a
    /// smaller box since ([`SHRINK_AFTER`]), each judged at the layouts
    /// until it is.
    dear: bool,
    /// Since when what moves in it has kept to a smaller box
    /// ([`SHRINK_AFTER`]), and the box around all that moved since.
    smaller: Option<(Duration, CellBox)>,
}

impl Live {
    /// The stream is over, and what it leaves owed: its END, where it
    /// started, and into `unsent` the damage no frame carried, on the
    /// desktop, of which the client holds the stream's last frame from
    /// before it ([`TilesEncoder::owe`]); or, where it never started, into
    /// `recovery` its rectangle whole, the client holding whatever was
    /// there before it, the change that began it never sent
    /// ([`TilesEncoder::recover`]).
    fn end(self, out: &mut Out, unsent: &mut Vec<Area>, recovery: &mut Vec<Area>) {
        if !self.started {
            recovery.push(self.area);
            return;
        }
        out.end(self.id, self.area);
        let Area { left, top, .. } = self.area;
        unsent.extend(self.pending.iter().map(|r| Area {
            left: left + r.x,
            top: top + r.y,
            right: left + r.x + r.width - 1,
            bottom: top + r.y + r.height - 1,
        }));
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
        self.out.extend_from_slice(&crate::rfb::msg::rect_header(
            area.left,
            area.top,
            area.w(),
            area.h(),
            wlshare_proto::ENCODING_TILES,
        ));
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
/// Its two halves are two calls, as the video encoding's are: [`Self::read`]
/// takes the damage out of the framebuffer, under its lock, into a picture of
/// its own, and [`Self::encode`] codes what is owed from there once the lock
/// is let go. Time is a [`Duration`] from any origin the caller keeps to.
pub struct TilesEncoder {
    size: (u16, u16),
    stride: usize,
    /// The picture as the encoder last read it, which every WebP and stream is
    /// coded from and every read is compared with.
    mirror: Vec<u8>,
    churn: Churn,
    /// When a read last found a copy: within [`CHURN_WINDOW`] of it, what
    /// churns may be a scroll the search missed rather than a video, and is
    /// looked among all the same.
    copied_at: Option<Duration>,
    /// The offsets of the copies the last read that found any found, from
    /// where each draws to where it reads, which the next read's search
    /// checks first.
    copied_by: Vec<(i32, i32)>,
    /// Tiles the client cannot be counted on to hold.
    unknown: HashSet<(u16, u16)>,
    /// The stills the next encode sends are of a picture the client held
    /// nothing of: it starts over, and the update crosses the link whole.
    restarting: bool,
    live: Vec<Live>,
    /// The tiles a live stream carries.
    covered: HashSet<(u16, u16)>,
    /// Tile → when a stream last carried it, owed a WebP, or a progressive
    /// pass below the full stage went out, owed its upgrade.
    debts: HashMap<(u16, u16), Duration>,
    /// What the client keeps of the tiles it was shown.
    cache: Cache,
    retuned_at: Option<Duration>,
    /// The stills the reads since the last encode owe.
    stills: Vec<Area>,
    /// The rectangles of streams that ended without starting, which the
    /// encode they ended in sends as stills whatever they come to.
    recovery: Vec<Area>,
    /// Until when no region is laid out, a stream having run out of
    /// [`START_WITHIN`] without a frame ([`START_RETRY`]).
    no_streams_until: Duration,
    /// The copies the reads since the last encode found, which go ahead of
    /// the stills, in the order they were found.
    copies: Vec<CopyRect>,
    /// Streams that ended outside an encode, owed their END.
    ended: Vec<(u8, Area)>,
    /// A read came since the last encode, which owes a round of the streams.
    read: bool,
    next_tick: Duration,
    /// The bytes the next tick may send ([`SETTLE_BYTES_LEAST`] to
    /// [`SETTLE_BYTES`]), and whether the update in flight carried a settle,
    /// whose crossing moves it.
    settle_bytes: usize,
    settling: bool,
    /// The updates of the last [`PACE_WINDOW`]: when each went, and the bytes
    /// the reads brought it, the tick's left out.
    sent: VecDeque<(Duration, usize)>,
    /// A held list's pace, in bytes a second: the most the desktop's own
    /// changes were found sending since something was last owed, which the
    /// settle keeps to; zero while nothing is.
    pace: usize,
    /// When a held list's settle began paying, which its rate eases in
    /// from; none while no tile is ready to be paid.
    easing_from: Option<Duration>,
    /// What the last piece a held list's settle coded cost a pixel that
    /// differs from the one to its left, in bytes.
    settle_rate: f64,
    /// wlshare's dial: what the streams are coded at, and the finest the walk
    /// asks for.
    ceiling: u8,
    /// The walk's dial, which a progressive first pass's stage follows.
    quality: u8,
    /// The walk has slowed the frames: no debt is paid until it has not.
    coarse: bool,
    /// The fewest moving tiles a region is laid out over, for the scale the
    /// output is drawn at ([`stream_cells`]).
    stream_cells: u32,
    /// When a read last found a tile changed.
    changed_at: Duration,
    /// The coder every stream is opened on.
    coder: Box<dyn H264Coder>,
    /// The cache's key of each whole tile [`Self::precode`] looked up, for
    /// the still it goes in to take rather than hash it again.
    keys: HashMap<(u16, u16), (u64, bool)>,
    /// The picture the client's list asks for.
    picture: TilesPicture,
    /// The progressive encoder, where the list walks and every tile is a
    /// progressive tile; where it holds every still is a WebP, and where it
    /// is lossless every still tile a planar bitmap and every moving one a
    /// median predictor's residuals.
    progressive: Option<progressive::Encoder>,
    /// A lossless list's still stream, from its first STILL; none where it
    /// starts over, with the picture or the client's cache.
    still_stream: Option<planar::StillStream>,
    /// Every region's frames carry its outline ([`Self::set_debug`]), this
    /// many pixels thick, two of the layout's.
    debug: bool,
    outline: u16,
    /// The mirror's pixels under an outline while its frame is read.
    under: Vec<u8>,
}

impl TilesEncoder {
    /// An encoder for a `width`×`height` desktop whose streams are `coder`'s,
    /// of the picture the client's list asks for: whether the dial walks, from
    /// wlshare's own as its ceiling. The client holds nothing
    /// of it yet.
    pub fn new(
        width: u16,
        height: u16,
        picture: TilesPicture,
        coder: Box<dyn H264Coder>,
    ) -> Result<Self, TilesError> {
        if width == 0 || height == 0 {
            return Err(TilesError::Empty(width, height));
        }
        let stride = usize::from(width) * 4;
        let mut encoder = Self {
            size: (width, height),
            stride,
            mirror: vec![0; stride * usize::from(height)],
            churn: Churn::default(),
            copied_at: None,
            copied_by: Vec::new(),
            unknown: HashSet::new(),
            restarting: false,
            live: Vec::new(),
            covered: HashSet::new(),
            debts: HashMap::new(),
            cache: Cache::default(),
            retuned_at: None,
            stills: Vec::new(),
            recovery: Vec::new(),
            no_streams_until: Duration::ZERO,
            copies: Vec::new(),
            ended: Vec::new(),
            read: false,
            next_tick: Duration::ZERO,
            settle_bytes: SETTLE_BYTES_LEAST,
            settling: false,
            sent: VecDeque::new(),
            pace: 0,
            easing_from: None,
            settle_rate: SETTLE_RATE_GUESS,
            ceiling: crate::rfb::QUALITY,
            quality: crate::rfb::QUALITY,
            coarse: false,
            stream_cells: MIN_STREAM_CELLS,
            changed_at: Duration::ZERO,
            coder,
            keys: HashMap::new(),
            picture,
            progressive: (picture == TilesPicture::Progressive)
                .then(|| progressive::Encoder::new(width, progressive::Tables::protocol())),
            still_stream: None,
            debug: false,
            outline: 2,
            under: Vec::new(),
        };
        encoder.forget();
        Ok(encoder)
    }

    /// The desktop size this encoder codes.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// The walk's dial, which `examples/motion-bench.rs` reads.
    #[allow(dead_code)]
    pub fn quality(&self) -> u8 {
        self.quality
    }

    /// Where the walk stands: `quality` is the dial the progressive first
    /// passes' stage follows from here ([`Self::first_stage`]); `coarse` says
    /// the walk has slowed the frames, which holds the upgrades owed until
    /// it has not. A held list's pace is its ceiling, never coarse.
    pub fn set_pace(&mut self, quality: u8, coarse: bool) {
        // A walk that steps down found the link short of what it was: what a
        // settle was found to take before says nothing of it now.
        if quality < self.quality || (coarse && !self.coarse) {
            self.settle_bytes = SETTLE_BYTES_LEAST;
        }
        self.coarse = coarse;
        self.quality = quality;
    }

    /// The scale the output is drawn at, 1 until it is said: the fewest
    /// moving tiles a region is laid out over from the next layout on
    /// ([`stream_cells`]). A live stream is kept, or ends, by that layout.
    pub fn set_scale(&mut self, scale: f64) {
        self.stream_cells = stream_cells(scale);
        self.outline = (2.0 * scale).round().clamp(2.0, 16.0) as u16;
    }

    /// Whether every region's stream carries an outline of its rectangle,
    /// drawn into the frames it is coded from and nowhere else: magenta
    /// around a region laid out over tiles that kept changing, orange around
    /// one a single update's change was more than its stills for. It is two
    /// pixels of the layout thick, inside the rectangle, and gone as the
    /// region settles, since every tile of a region is owed its still, which
    /// is of the desktop's pixels alone. For seeing what the rules make a
    /// video of; the strips are painted and put back around each frame's
    /// read, a few thousand pixels a frame.
    pub fn set_debug(&mut self, debug: bool) {
        self.debug = debug;
    }

    /// A new picture, a walk the list did not name before: what the client
    /// holds was coded against the old one, and nothing of it is owed what the
    /// new one would owe, so the picture starts over as [`Self::forget`]
    /// starts it, every live stream's END owed first. The cache is kept, but a held or a
    /// lossless list is owed lossless pixels, which no tile a walked list stored is.
    pub fn set_picture(&mut self, picture: TilesPicture) {
        if self.picture == TilesPicture::Progressive && picture != TilesPicture::Progressive {
            self.cache.unsettle();
        }
        if (picture == TilesPicture::Progressive) != self.progressive.is_some() {
            self.progressive = (picture == TilesPicture::Progressive)
                .then(|| progressive::Encoder::new(self.size.0, progressive::Tables::protocol()));
        }
        self.picture = picture;
        self.forget();
    }

    /// The client holds nothing of the picture: every stream ends, with its
    /// END at the next encode, nothing is owed, and the next read is sent
    /// whole. Its cache it keeps, so the whole picture is drawn from it
    /// wherever the record has a tile.
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
        self.copied_at = None;
        self.copied_by.clear();
        self.stills.clear();
        self.recovery.clear();
        self.copies.clear();
        self.restarting = true;
        self.pace = 0;
        self.easing_from = None;
        self.still_stream = None;
        self.unknown = tiles_of(Area::whole(self.size))
            .map(|(cell, _)| cell)
            .collect();
        if let Some(enc) = &mut self.progressive {
            enc.forget();
        }
    }

    /// The client's cache holds nothing it can count on: no slot is drawn
    /// from until it is stored again, from the first. Nor its still stream,
    /// which starts over: a client whose cache is gone, or that missed a
    /// rectangle or could not read one, holds none of it either.
    pub fn forget_cache(&mut self) {
        self.cache.clear();
        self.still_stream = None;
    }

    /// The client keeps its cache through a new size: this encoder, made for
    /// it, draws from what `old`, the size before's, stored.
    pub fn keep_cache(&mut self, mut old: TilesEncoder) {
        self.cache = std::mem::take(&mut old.cache);
    }

    /// The stage a progressive first pass is coded at: 75 while the walk is
    /// at the ceiling and not coarse, 50 while its dial is at
    /// [`PROGRESSIVE_COARSE_BELOW`] or above, else the coarser stage.
    fn first_stage(&self) -> usize {
        if !self.coarse && self.quality >= self.ceiling {
            2
        } else if self.quality >= PROGRESSIVE_COARSE_BELOW {
            1
        } else {
            0
        }
    }

    /// Code the first passes of the progressive tiles of `stills` ahead, on
    /// rayon's threads where there are two or more, each tile the client's
    /// cache does not hold now; [`Self::progressive_tile`] takes each as it comes
    /// to it, in order, where nothing has touched the tile since.
    fn precode(&mut self, stills: &[Area]) {
        if self.progressive.is_none() || rayon::current_num_threads() < 2 {
            return;
        }
        let mut seen = HashSet::new();
        let (mirror, stride, size, cache, keys) = (
            &self.mirror,
            self.stride,
            self.size,
            &self.cache,
            &mut self.keys,
        );
        let cell_area = |cell: (u16, u16)| {
            CellBox::of(cell)
                .to_area(size)
                .expect("a tile of the desktop")
        };
        let jobs: Vec<progressive::Job> = stills
            .iter()
            .flat_map(|&area| tiles_of(area))
            .filter(|&(cell, part)| {
                let whole = part.w() == TILE && part.h() == TILE && part == cell_area(cell);
                seen.insert(cell)
                    && !(whole
                        && cache
                            .find(
                                keys.entry(cell)
                                    .or_insert_with(|| tile_key(mirror, stride, part))
                                    .0,
                            )
                            .is_some())
            })
            .map(|(cell, _)| {
                let area = cell_area(cell);
                progressive::Job {
                    cell,
                    at: usize::from(area.top) * stride + usize::from(area.left) * 4,
                    w: usize::from(area.w()),
                    h: usize::from(area.h()),
                }
            })
            .collect();
        // A few tiles are coded as they are sent, on this thread: waking the
        // pool for them costs more than it saves.
        if jobs.len() < PRECODE_LEAST {
            return;
        }
        let stage = self.first_stage();
        self.progressive.as_mut().expect("progressive").precode(
            &jobs,
            &self.mirror,
            self.stride,
            stage,
            PROGRESSIVE_SLACK,
        );
    }

    /// Send tile `cell` as a progressive tile: a first pass at
    /// [`Self::first_stage`] — 75 while the walk is at the ceiling and not
    /// coarse, as the host's passes are at 75 while its link has room — or
    /// whole where that costs little more; or, settling, the upgrade
    /// to the stage after the one it stands at, a whole first pass where the
    /// client holds no coefficients of it. Owed the next stage until full.
    fn progressive_tile(
        &mut self,
        cell: (u16, u16),
        pass: ProgressivePass,
        now: Duration,
        out: &mut Out,
    ) -> Result<bool, TilesError> {
        let area = self.cell_area(cell);
        let at = usize::from(area.top) * self.stride + usize::from(area.left) * 4;
        let stage = self.first_stage();
        let enc = self.progressive.as_mut().expect("progressive");
        let full = enc.tables.stages.len() - 1;
        let (w, h) = (usize::from(area.w()), usize::from(area.h()));
        let block = match (pass, enc.stage_of(cell)) {
            (ProgressivePass::Upgrade, Some(stage)) => enc.upgrade(cell, (stage + 1).min(full)),
            (ProgressivePass::Upgrade, None) => Some(
                enc.first(cell, &self.mirror[at..], self.stride, w, h, full)
                    .block,
            ),
            (ProgressivePass::First, _) => Some(
                enc.first_or_full(
                    cell,
                    &self.mirror[at..],
                    self.stride,
                    w,
                    h,
                    stage,
                    PROGRESSIVE_SLACK,
                )
                .block,
            ),
        };
        let settled = enc.stage_of(cell) == Some(full);
        if let Some(block) = block {
            let from = out.out.len();
            let header = TileHeader::Progressive { len: 0 };
            let header_at = out.header(area, header);
            out.out.extend_from_slice(&block);
            out.length(from, header_at, header)?;
        }
        // An upgrade is no change: the tile stays owed from when it last
        // changed, so its next stage is ready at once.
        if settled {
            self.debts.remove(&cell);
        } else if pass == ProgressivePass::First {
            self.debts.insert(cell, now);
        } else {
            self.debts.entry(cell).or_insert(now);
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
    /// the read before, or anywhere for `None`: the caller's word, as the video
    /// encoding takes it. The rectangles are copied into the encoder's own
    /// picture and compared with what the client holds, so `pixels` is the
    /// framebuffer itself, for as long as the copy takes. Nothing is coded: the
    /// next [`Self::encode`] codes what this owes. The first read of an encoder
    /// made or forgotten is the whole picture whatever `changed` says.
    ///
    /// Where the damage moved pixels the client holds — a scroll, a window
    /// dragged — the rectangle they now fill is a copy of where they were
    /// ([`Self::find_copies`]), and none of its tiles is a still.
    pub fn read(
        &mut self,
        pixels: &[u8],
        stride: usize,
        changed: Option<&[Rect]>,
        now: Duration,
    ) -> Result<(), TilesError> {
        let (w, h) = self.size;
        if stride < usize::from(w) * 4
            || pixels.len() < (usize::from(h) - 1) * stride + usize::from(w) * 4
        {
            return Err(TilesError::Buffer {
                width: w,
                height: h,
                stride,
                bytes: pixels.len(),
            });
        }
        let whole = [Area::whole(self.size)];
        let areas: Vec<Area> = match changed.filter(|_| self.unknown.is_empty()) {
            Some(rects) => rects
                .iter()
                .filter_map(|r| Area::clipped(*r, self.size))
                .collect(),
            None => whole.to_vec(),
        };
        // The tiles of each area that differ from the picture before any of
        // them, compared once: what copies are looked among, and what each
        // area's damage takes as changed.
        let differ: Vec<HashSet<(u16, u16)>> = areas
            .iter()
            .map(|area| {
                tiles_of(*area)
                    .filter(|(_, part)| differs(&self.mirror, self.stride, pixels, stride, *part))
                    .map(|(cell, _)| cell)
                    .collect()
            })
            .collect();
        let copies = self.find_copies(pixels, stride, &areas, &differ, now);
        if !copies.is_empty() {
            self.copied_at = Some(now);
            self.copied_by = copies
                .iter()
                .map(|copy| {
                    (
                        i32::from(copy.src.0) - i32::from(copy.dst.left),
                        i32::from(copy.src.1) - i32::from(copy.dst.top),
                    )
                })
                .collect();
            self.copied_by.dedup();
        }
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
    /// been changing in place ([`COPY_CHURN`]), unless a copy was found within
    /// the churn's window, a scroll the search may have missed: up to
    /// [`MAX_PROBES`] runs of those tiles, none of one colour, are looked for
    /// along the rows of the old picture within the damage and [`COPY_REACH`]
    /// of them, every [`MAX_SCAN_STEP`]th at most, by a rolling hash, and
    /// along the rows it stepped over where nothing that found grew into a
    /// copy; each offset
    /// one of them was found at, of those found at [`MAX_PROBE_HITS`] places
    /// at most, is checked against every such probe, and the offsets the most
    /// match at are grown, from each run, into the rectangle whose rows are
    /// the old picture's at that offset, kept where it draws a changed tile
    /// whole. A
    /// copy goes out ahead of the update's stills, so it reads only what the
    /// client holds by then: never a tile a stream carries, or one a still not yet sent is
    /// owed for, or the rectangle of a copy before it in the same read, since
    /// it is matched against the picture before any of them; nor does it draw
    /// over a tile a stream carries.
    fn find_copies(
        &self,
        pixels: &[u8],
        stride: usize,
        areas: &[Area],
        differ: &[HashSet<(u16, u16)>],
        now: Duration,
    ) -> Vec<CopyRect> {
        if !self.unknown.is_empty()
            || areas
                .iter()
                .map(|area| tiles_of(*area).count())
                .sum::<usize>()
                < COPY_MIN_TILES
        {
            return Vec::new();
        }
        let (old, old_stride) = (&self.mirror[..], self.stride);
        // A copy found within the churn's window: a scroll is on, and a tile
        // it changed that the search missed churns as a video's would. Left
        // out, two misses would leave the rest of the scroll nothing to find.
        let scrolling = self
            .copied_at
            .is_some_and(|at| now.saturating_sub(at) < CHURN_SLOT * CHURN_WINDOW as u32);
        let parts: Vec<(usize, Area)> = areas
            .iter()
            .enumerate()
            .flat_map(|(i, area)| tiles_of(*area).map(move |(cell, part)| (i, cell, part)))
            .filter(|(i, cell, _)| {
                !self.covered.contains(cell)
                    && (scrolling || self.churn.recent(*cell, now) < COPY_CHURN)
                    && differ[*i].contains(cell)
            })
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
            let busy = |y: u16| {
                (x + 1..x + PROBE)
                    .any(|at| pixel(pixels, stride, at, y) != pixel(pixels, stride, x, y))
            };
            let shift = (probes.len() % usize::from(MAX_SCAN_STEP)) as u16;
            if let Some(y) = (0..4)
                .map(|k| (part.top + part.h() * k / 4 + shift).min(part.bottom))
                .find(|y| busy(*y))
            {
                probes.push(Probe {
                    x,
                    y,
                    area,
                    hash: run_hash(pixels, stride, x, y),
                });
            }
        }
        // Within the damage, which what moved left too, and within reach of the
        // tiles looked among.
        let union = |a: Area, b: Area| Area {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        };
        let damage = areas.iter().copied().reduce(union).expect("some damage");
        let looked = parts
            .iter()
            .map(|(_, part)| *part)
            .reduce(union)
            .expect("some parts");
        let reach = Area {
            left: looked.left.saturating_sub(COPY_REACH),
            top: looked.top.saturating_sub(COPY_REACH),
            right: looked.right.saturating_add(COPY_REACH),
            bottom: looked.bottom.saturating_add(COPY_REACH),
        };
        let window = damage
            .intersect(reach)
            .expect("the parts are in the damage");
        let looked_at: u32 = parts
            .iter()
            .map(|(_, part)| u32::from(part.w()) * u32::from(part.h()))
            .sum();
        if probes.is_empty()
            || window.w() < PROBE
            || u32::from(window.w()) * u32::from(window.h()) > COPY_WINDOW_RATIO * looked_at
        {
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
        let mut scan = |y: u16, hits: &mut [Vec<(i32, i32)>]| {
            let at = usize::from(y) * old_stride + usize::from(window.left) * 4;
            row.clear();
            row.extend(
                old[at..at + usize::from(window.w()) * 4]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|px| u64::from(u32::from_le_bytes(*px))),
            );
            let mut hash = row[..usize::from(PROBE)].iter().fold(0u64, |hash, px| {
                hash.wrapping_mul(HASH_BASE).wrapping_add(*px)
            });
            let last = row.len() - usize::from(PROBE);
            for dx in 0..=last {
                if dx > 0 {
                    // The run before, its first pixel dropped and the next taken on.
                    hash = hash
                        .wrapping_sub(row[dx - 1].wrapping_mul(first))
                        .wrapping_mul(HASH_BASE)
                        .wrapping_add(row[dx + usize::from(PROBE) - 1]);
                }
                let bit = filter_bit(hash);
                if filter[bit / 64] & (1 << (bit % 64)) != 0 {
                    let x = window.left + dx as u16;
                    for (probe, hits) in probes
                        .iter()
                        .zip(hits.iter_mut())
                        .filter(|(probe, hits)| probe.hash == hash && hits.len() <= MAX_PROBE_HITS)
                    {
                        let offset = (
                            i32::from(x) - i32::from(probe.x),
                            i32::from(y) - i32::from(probe.y),
                        );
                        if offset != (0, 0) {
                            hits.push(offset);
                        }
                    }
                }
            }
        };
        // The scan looks along every `step`th row only, so a probe is found
        // at an offset only where its row lands on one: a quarter of them at
        // a step of four, fewer than two on a fast scroll whose new rows hold
        // most of the rest. Each offset it found, the most found first, is
        // checked against every probe it did not crowd out, a probe's run
        // against the old picture's there; those two or more match are kept,
        // the ones a quarter as many as the most at least.
        let (w, h) = self.size;
        let matches = |probe: &Probe, (dx, dy): (i32, i32)| {
            let (Ok(x), Ok(y)) = (
                u16::try_from(i32::from(probe.x) + dx),
                u16::try_from(i32::from(probe.y) + dy),
            ) else {
                return false;
            };
            if u32::from(x) + u32::from(PROBE) > u32::from(w) || y >= h {
                return false;
            }
            let new = usize::from(probe.y) * stride + usize::from(probe.x) * 4;
            let was = usize::from(y) * old_stride + usize::from(x) * 4;
            pixels[new..new + usize::from(PROBE) * 4] == old[was..was + usize::from(PROBE) * 4]
        };
        let tally = |hits: &[Vec<(i32, i32)>], tried: &HashSet<(i32, i32)>| {
            let mut found: HashMap<(i32, i32), usize> = HashMap::new();
            for offsets in hits
                .iter()
                .filter(|offsets| offsets.len() <= MAX_PROBE_HITS)
            {
                for offset in offsets.iter().filter(|offset| !tried.contains(offset)) {
                    *found.entry(*offset).or_default() += 1;
                }
            }
            // The offsets the last read's copies were at, checked whatever
            // the scan found of them: a window dragged on moves as it did,
            // and its text, found at too many places to say anything, leaves
            // the scan too few runs that agree (measured: a terminal dragged
            // at 2880x1800 sent 2.2 to 3.9 times the bytes without).
            for offset in self
                .copied_by
                .iter()
                .filter(|offset| !tried.contains(offset))
            {
                *found.entry(*offset).or_default() = usize::MAX;
            }
            let mut found: Vec<((i32, i32), usize)> = found.into_iter().collect();
            found.sort_unstable_by_key(|&(offset, seeds)| (Reverse(seeds), offset));
            found.truncate(MAX_CHECKED);
            let mut offsets: Vec<((i32, i32), Vec<usize>)> = found
                .into_iter()
                .map(|(offset, _)| {
                    // At an offset a copy was at, every probe found there
                    // counts, those found at many places too.
                    let known = self.copied_by.contains(&offset);
                    (
                        offset,
                        (0..probes.len())
                            .filter(|&i| {
                                (known || hits[i].len() <= MAX_PROBE_HITS)
                                    && matches(&probes[i], offset)
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .filter(|(_, seeds)| seeds.len() >= 2)
                .collect();
            offsets.sort_unstable_by_key(|(offset, seeds)| (Reverse(seeds.len()), *offset));
            let most = offsets.first().map_or(0, |(_, seeds)| seeds.len());
            offsets.retain(|(_, seeds)| seeds.len() * 4 >= most);
            offsets.truncate(MAX_OFFSETS);
            offsets
        };
        // Each offset grown from its probes, one not inside a copy already.
        let pending: HashSet<(u16, u16)> = self
            .stills
            .iter()
            .flat_map(|still| tiles_of(*still).map(|(cell, _)| cell))
            .collect();
        let grow_all = |offsets: Vec<((i32, i32), Vec<usize>)>| {
            let mut copies: Vec<CopyRect> = Vec::new();
            for (offset, seeds) in offsets {
                for probe in seeds.into_iter().map(|i| &probes[i]) {
                    if copies.len() == MAX_COPIES {
                        return copies;
                    }
                    let at = Area {
                        left: probe.x,
                        top: probe.y,
                        right: probe.x,
                        bottom: probe.y,
                    };
                    if copies.iter().any(|copy| copy.dst.contains(at)) {
                        continue;
                    }
                    let Some(dst) = self.grow(
                        pixels,
                        stride,
                        areas[probe.area],
                        offset,
                        probe,
                        &pending,
                        &copies,
                    ) else {
                        continue;
                    };
                    if u32::from(dst.w()) * u32::from(dst.h()) < COPY_MIN_PIXELS {
                        continue;
                    }
                    // Where it draws only some of a tile, the tile is a still
                    // anyway: a copy that draws no changed tile whole saves
                    // nothing, and cuts the stills' runs short.
                    if !parts.iter().any(|(_, part)| dst.contains(*part)) {
                        continue;
                    }
                    let src = (
                        (i32::from(dst.left) + offset.0) as u16,
                        (i32::from(dst.top) + offset.1) as u16,
                    );
                    let copy = CopyRect {
                        dst,
                        src,
                        owed: false,
                    };
                    let owed =
                        tiles_of(copy.source()).any(|(cell, _)| self.debts.contains_key(&cell));
                    copies.push(CopyRect { owed, ..copy });
                }
            }
            copies
        };
        for y in (window.top..=window.bottom).filter(|y| y % step == 0) {
            scan(y, &mut hits);
        }
        let offsets = tally(&hits, &HashSet::new());
        let tried: HashSet<(i32, i32)> = offsets.iter().map(|(offset, _)| *offset).collect();
        let copies = grow_all(offsets);
        // Where that found nothing to copy, the rows stepped over, the
        // offsets already grown left out: a miss is the whole page coded
        // again, many times the scan's cost.
        if !copies.is_empty() || step == 1 {
            return copies;
        }
        for y in (window.top..=window.bottom).filter(|y| y % step != 0) {
            scan(y, &mut hits);
        }
        grow_all(tally(&hits, &tried))
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
    fn grow(
        &self,
        pixels: &[u8],
        stride: usize,
        area: Area,
        (dx, dy): (i32, i32),
        probe: &Probe,
        pending: &HashSet<(u16, u16)>,
        copies: &[CopyRect],
    ) -> Option<Area> {
        let (w, h) = self.size;
        let (old, old_stride) = (&self.mirror[..], self.stride);
        let from_x = |x: u16| u16::try_from(i32::from(x) + dx).ok().filter(|x| *x < w);
        let from_y = |y: u16| u16::try_from(i32::from(y) + dy).ok().filter(|y| *y < h);
        // Where a copy may draw, and where it may read from: no stream's tile,
        // no tile owed a still not yet sent, nothing an earlier copy draws.
        let drawable = |cell: (u16, u16)| !self.covered.contains(&cell);
        let readable = |cell: (u16, u16)| !self.covered.contains(&cell) && !pending.contains(&cell);
        let clear = |source: Area| {
            copies
                .iter()
                .all(|copy| copy.dst.intersect(source).is_none())
        };
        let source = |dst: Area| {
            let (Some(left), Some(top), Some(right), Some(bottom)) = (
                from_x(dst.left),
                from_y(dst.top),
                from_x(dst.right),
                from_y(dst.bottom),
            ) else {
                return None;
            };
            Some(Area {
                left,
                top,
                right,
                bottom,
            })
        };
        let cells = |dst: Area, source: Area| {
            tiles_of(dst).all(|(cell, _)| drawable(cell))
                && tiles_of(source).all(|(cell, _)| readable(cell))
        };
        let fits =
            |dst: Area| source(dst).is_some_and(|source| cells(dst, source) && clear(source));
        // A row's tiles, and its source's, are those of every row of the same
        // band and the same band of the source: the last band's answer, for
        // the width it was asked for.
        let fitted = std::cell::Cell::new(None);
        let row = |y: u16, left: u16, right: u16| {
            let dst = Area {
                left,
                top: y,
                right,
                bottom: y,
            };
            let Some(from) = source(dst) else {
                return false;
            };
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
                let (oy, ol, or) = (
                    from_y(y).expect("fits"),
                    from_x(left).expect("fits"),
                    from_x(right).expect("fits"),
                );
                let new = &pixels[usize::from(y) * stride + usize::from(left) * 4
                    ..usize::from(y) * stride + (usize::from(right) + 1) * 4];
                let was = &old[usize::from(oy) * old_stride + usize::from(ol) * 4
                    ..usize::from(oy) * old_stride + (usize::from(or) + 1) * 4];
                new == was
            }
        };
        let column = |x: u16, top: u16, bottom: u16, sample: &[u16]| {
            let dst = Area {
                left: x,
                top,
                right: x,
                bottom,
            };
            fits(dst) && {
                let ox = from_x(x).expect("fits");
                sample.iter().all(|y| {
                    pixel(pixels, stride, x, *y)
                        == pixel(old, old_stride, ox, from_y(*y).expect("fits"))
                })
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
        let sample: Vec<u16> = (top..=bottom)
            .step_by(GROW_SAMPLE)
            .chain([y, bottom])
            .collect();
        while left > area.left && column(left - 1, top, bottom, &sample) {
            left -= 1;
        }
        while right < area.right && column(right + 1, top, bottom, &sample) {
            right += 1;
        }
        let (mut top, mut bottom) = (y, y);
        rows(&mut top, &mut bottom, left, right);
        Some(Area {
            left,
            top,
            right,
            bottom,
        })
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
    fn damage(
        &mut self,
        pixels: &[u8],
        stride: usize,
        area: Area,
        differ: &HashSet<(u16, u16)>,
        before: &[Area],
        copies: &[CopyRect],
        now: Duration,
    ) {
        let mut changed: HashSet<(u16, u16)> = tiles_of(area)
            .filter(|(cell, part)| {
                if before.iter().any(|b| b.intersect(*part).is_some()) {
                    differs(&self.mirror, self.stride, pixels, stride, *part)
                } else {
                    differ.contains(cell)
                }
            })
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
            let band = Area {
                top,
                bottom: (top / BAND_ROWS + 1)
                    .saturating_mul(BAND_ROWS)
                    .saturating_sub(1)
                    .min(area.bottom),
                ..area
            };
            top = band.bottom.saturating_add(1);
            blit(&mut self.mirror, self.stride, pixels, stride, band);
            // The runs of changed tiles, and of those the ones no stream carries.
            let (mut changes, mut runs): (Vec<Area>, Vec<Area>) = (Vec::new(), Vec::new());
            let extend = |runs: &mut Vec<Area>, part: Area| match runs.last_mut() {
                Some(run) if run.top == part.top && run.right.checked_add(1) == Some(part.left) => {
                    run.right = part.right
                }
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
                if !copies.iter().any(|copy| {
                    copy.dst.intersect(part).is_some() || copy.source().intersect(part).is_some()
                }) {
                    self.churn.observe(cell, now);
                }
                extend(&mut changes, part);
                if !self.covered.contains(&cell) {
                    extend(&mut runs, part);
                }
            }
            for live in &mut self.live {
                for part in changes
                    .iter()
                    .filter_map(|change| live.area.intersect(*change))
                {
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
    /// ended and the tiles still long enough settled; last, as
    /// stills, what changed under a stream that ended here that no frame of it
    /// carried. None when nothing is owed, which is no update to send.
    ///
    /// Tens of milliseconds for a desktop that changed all over. An error
    /// takes back the rectangle it was in, and leaves the ones before it.
    pub fn encode(&mut self, now: Duration, out: &mut Vec<u8>) -> Result<Update, TilesError> {
        // Keys of the picture an encode that failed was coding, before
        // anything here can fail.
        self.keys.clear();
        let from = out.len();
        let mut out = Out {
            out,
            count: 0,
            keyframe: false,
        };
        for (id, area) in std::mem::take(&mut self.ended) {
            out.end(id, area);
        }
        let moved = !self.copies.is_empty();
        for copy in std::mem::take(&mut self.copies) {
            out.header(
                copy.dst,
                TileHeader::Copy {
                    x: copy.src.0,
                    y: copy.src.1,
                },
            );
        }
        let stills = std::mem::take(&mut self.stills);
        if !stills.is_empty() && std::mem::take(&mut self.restarting) {
            out.keyframe = true;
        }
        let mut owed_round = false;
        if self.picture == TilesPicture::Held {
            owed_round = self.held_stills(stills, moved, now, &mut out)?;
        } else {
            self.stills(stills, now, &mut out)?;
        }
        if std::mem::take(&mut self.read) || owed_round {
            self.round(now, &mut out)?;
        }
        let brought = out.count;
        // What the reads brought so far, in the pace before the tick measures
        // by it: a tick with nothing ready to pay looks at no budget, and the
        // video then playing is the pace its settle will keep to.
        self.observe_pace(now, out.out.len() - from);
        if now >= self.next_tick {
            let more = self.tick(now, &mut out)?;
            self.next_tick = if self.picture == TilesPicture::Held {
                // A held list's settle keeps to its pace: never a tick on the
                // heels of the last, however much is owed.
                now + self.tick_interval()
            } else if more && self.quiet(now) {
                // On a quiet desktop a tick its budget cut short is owed again at
                // the next update's turn, which waits for this one's fence: the
                // settle goes as fast as the link takes it. While it moves, a
                // tick alone between its updates would hold back the next, and
                // start the walk's verdicts over.
                now
            } else {
                now + CLEANUP_TICK
            };
        }
        self.settling = out.count > brought;
        let after_tick = out.out.len();
        // What the streams that ended here had not carried, after their ENDs;
        // a region it is more than an update's stills for started at once.
        let stills = std::mem::take(&mut self.stills);
        if self.picture == TilesPicture::Held {
            if self.held_stills(stills, moved, now, &mut out)? {
                self.round(now, &mut out)?;
            }
            self.recover(now, &mut out)?;
        } else {
            self.stills(stills, now, &mut out)?;
        }
        if let Some(enc) = &mut self.progressive {
            enc.drop_ready();
            self.keys.clear();
        }
        // And what the streams that ended here brought; a settle's own bytes
        // are none of the pace, or it would raise the rate it keeps to.
        self.observe_pace(now, out.out.len() - after_tick);
        if !self.owed() {
            self.pace = 0;
        }
        let rects = u16::try_from(out.count).map_err(|_| TilesError::TooMany(out.count))?;
        Ok(Update {
            rects,
            keyframe: out.keyframe,
            tick: brought == 0,
            stopped: brought == 0 && self.quiet(now),
        })
    }

    /// How long the last update queued on its way: its fence's round trip
    /// less the link's own, the least of the last minute's as the walk keeps
    /// it ([`crate::walk::QualityWalk::floor`]), or where the client has no
    /// fence how long its write blocked. One that
    /// carried a settle and crossed within [`SETTLE_QUICK`] lets the next
    /// tick send twice the bytes, up to [`SETTLE_BYTES`]; one that took
    /// [`SETTLE_LATE`] or more, half, down to [`SETTLE_BYTES_LEAST`]. So a
    /// desktop that stops settles as fast as the link carries it, whatever
    /// the walk's dial, and a settle never queues on a link with no room.
    pub fn delivered(&mut self, took: Duration) {
        if !std::mem::take(&mut self.settling) {
            return;
        }
        if took <= SETTLE_QUICK {
            self.settle_bytes = (self.settle_bytes * 2).min(SETTLE_BYTES);
        } else if took >= SETTLE_LATE {
            self.settle_bytes = (self.settle_bytes / 2).max(SETTLE_BYTES_LEAST);
        }
    }

    /// No tile has changed for [`CLEANUP_IDLE`].
    fn quiet(&self, now: Duration) -> bool {
        now.saturating_sub(self.changed_at) >= CLEANUP_IDLE
    }

    /// Whether anything is owed that no read will bring: a stream is live, or
    /// its end is yet to be sent, or a tile is yet to be settled.
    pub fn owed(&self) -> bool {
        !self.live.is_empty() || !self.ended.is_empty() || !self.debts.is_empty()
    }

    /// When an [`Self::encode`] with no read is next owed something — a
    /// stream to end, a tile to settle — or `None` while the
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
        CellBox::of(cell)
            .to_area(self.size)
            .expect("a tile of the desktop")
    }

    /// Send `area` of the picture, a run of tiles along one row, as a still:
    /// on a held list a WebP, lossless; on a walked one each tile a progressive
    /// tile ([`Self::progressive_still`]). A whole tile of it the client's
    /// cache keeps lossless goes as a draw of its slot instead, and the run is
    /// coded around it ([`Self::coded`]). A WebP pays the debt of every tile it
    /// covers whole, and so does a draw.
    fn still(
        &mut self,
        area: Area,
        now: Duration,
        change: bool,
        out: &mut Out,
    ) -> Result<(), TilesError> {
        debug_assert!(
            self.picture != TilesPicture::Lossless,
            "a lossless list's stills are packed together"
        );
        if self.progressive.is_some() {
            return self.progressive_still(area, now, out);
        }
        let tiles: Vec<StillTile> = tiles_of(area)
            .map(|(cell, part)| {
                let whole = part.w() == TILE && part.h() == TILE && part == self.cell_area(cell);
                (
                    cell,
                    part,
                    whole.then(|| tile_key(&self.mirror, self.stride, part)),
                )
            })
            .collect();
        // Every slot drawn from is touched before any of the run is coded:
        // the stores of the pieces coded ahead of a draw take the least
        // recently used slots, which the drawn one must not be among, or the
        // client would have written it over before the draw.
        let hits: Vec<Option<u16>> = tiles
            .iter()
            .map(|(_, _, key)| {
                key.and_then(|(hash, _)| self.cache.find(hash))
                    .filter(|(_, kept)| *kept)
                    .map(|(slot, _)| slot)
            })
            .collect();
        for slot in hits.iter().flatten() {
            self.cache.touch(*slot);
        }
        let mut run = 0;
        for (i, hit) in hits.into_iter().enumerate() {
            let Some(slot) = hit else { continue };
            let (cell, part, _) = tiles[i];
            self.coded(&tiles[run..i], change.then_some(now), out)?;
            run = i + 1;
            out.header(part, TileHeader::Cached { slot });
            self.debts.remove(&cell);
        }
        self.coded(&tiles[run..], change.then_some(now), out)
    }

    /// Send `stills`, each a run of tiles along one row: on a lossless list
    /// packed together ([`Self::lossless_stills`]), on any other one at a
    /// time ([`Self::still`]), a walked list's first passes coded ahead.
    fn stills(
        &mut self,
        stills: Vec<Area>,
        now: Duration,
        out: &mut Out,
    ) -> Result<(), TilesError> {
        if self.picture == TilesPicture::Lossless {
            let sent = self.lossless_stills(stills, now, out);
            if sent.is_err() {
                // Its bytes are the stream's, and what a client was sent of it is not.
                self.still_stream = None;
            }
            return sent;
        }
        self.precode(&stills);
        for area in stills {
            self.still(area, now, true, out)?;
        }
        Ok(())
    }

    /// A lossless list's `stills`, as a held list's would go but every run a
    /// lossless body in place of a WebP: each whole tile the client's cache
    /// keeps a draw of its slot, the run coded around it, and every whole
    /// tile of a run not one colour stored after it. A run is cut where its
    /// tiles go from moving to still: a moving one is a MEDIAN, its
    /// residuals ([`median::pack`]), and a still one a STILL, its planar
    /// bitmap in the still stream ([`planar::StillStream`]). Laid out first, in order, the
    /// cache's record moved as the rectangles will move the client's; then
    /// every run coded on rayon's threads where there are two or more and
    /// two runs to code, the moving ones framed; and written in order, the
    /// still ones through the stream. Nothing is owed: every pixel goes exact.
    fn lossless_stills(
        &mut self,
        stills: Vec<Area>,
        now: Duration,
        out: &mut Out,
    ) -> Result<(), TilesError> {
        enum Piece {
            Run {
                area: Area,
                still: bool,
                stores: Vec<(Area, u16)>,
            },
            Cached {
                part: Area,
                slot: u16,
            },
        }
        let mut plan = Vec::new();
        for area in stills {
            let tiles: Vec<StillTile> = tiles_of(area)
                .map(|(cell, part)| {
                    let whole =
                        part.w() == TILE && part.h() == TILE && part == self.cell_area(cell);
                    (
                        cell,
                        part,
                        whole.then(|| tile_key(&self.mirror, self.stride, part)),
                    )
                })
                .collect();
            // Every slot drawn from is touched before any of the run is
            // stored, as a held list's are.
            let hits: Vec<Option<u16>> = tiles
                .iter()
                .map(|(_, _, key)| {
                    key.and_then(|(hash, _)| self.cache.find(hash))
                        .filter(|(_, kept)| *kept)
                        .map(|(slot, _)| slot)
                })
                .collect();
            for slot in hits.iter().flatten() {
                self.cache.touch(*slot);
            }
            let (cache, churn) = (&mut self.cache, &self.churn);
            let moving = |cell: (u16, u16)| churn.recent(cell, now) >= CHURN_MOVING;
            let mut run_of = |tiles: &[StillTile], plan: &mut Vec<Piece>| {
                for run in tiles.chunk_by(|a, b| moving(a.0) == moving(b.0)) {
                    let (first, last) = (run[0], run[run.len() - 1]);
                    let stores = run.iter().filter_map(|(_, part, key)| match key {
                        Some((hash, false)) => Some((*part, cache.store(*hash, true))),
                        _ => None,
                    });
                    plan.push(Piece::Run {
                        area: Area {
                            right: last.1.right,
                            ..first.1
                        },
                        still: !moving(first.0),
                        stores: stores.collect(),
                    });
                }
            };
            let mut run = 0;
            for (i, hit) in hits.into_iter().enumerate() {
                let Some(slot) = hit else { continue };
                run_of(&tiles[run..i], &mut plan);
                run = i + 1;
                plan.push(Piece::Cached {
                    part: tiles[i].1,
                    slot,
                });
            }
            run_of(&tiles[run..], &mut plan);
        }
        let (mirror, stride) = (&self.mirror, self.stride);
        // A moving run's body, framed, or a still one's bitmap, for the stream.
        let code = |&(area, still): &(Area, bool)| {
            let pixels = &mirror[usize::from(area.top) * stride + usize::from(area.left) * 4..];
            if still {
                Ok(planar::bitmap(pixels, stride, area.w(), area.h()))
            } else {
                median::pack(pixels, stride, area.w(), area.h())
            }
        };
        let runs: Vec<(Area, bool)> = plan
            .iter()
            .filter_map(|piece| {
                if let Piece::Run { area, still, .. } = piece {
                    Some((*area, *still))
                } else {
                    None
                }
            })
            .collect();
        let coded: Vec<std::io::Result<Vec<u8>>> =
            if runs.len() >= 2 && rayon::current_num_threads() >= 2 {
                runs.par_iter().map(code).collect()
            } else {
                runs.iter().map(code).collect()
            };
        let mut coded = coded.into_iter();
        for piece in plan {
            match piece {
                Piece::Run {
                    area,
                    still,
                    stores,
                } => {
                    let coded = coded
                        .next()
                        .expect("a body for every run")
                        .map_err(TilesError::Lossless)?;
                    let from = out.out.len();
                    let header = if still {
                        TileHeader::Still { len: 0 }
                    } else {
                        TileHeader::Median { len: 0 }
                    };
                    let at = out.header(area, header);
                    if still {
                        let stream = match &mut self.still_stream {
                            Some(stream) => stream,
                            None => self
                                .still_stream
                                .insert(planar::StillStream::new().map_err(TilesError::Lossless)?),
                        };
                        stream.push(&coded, out.out).map_err(TilesError::Lossless)?;
                    } else {
                        out.out.extend_from_slice(&coded);
                    }
                    out.length(from, at, header)?;
                    for (part, slot) in stores {
                        out.header(part, TileHeader::Store { slot });
                    }
                }
                Piece::Cached { part, slot } => {
                    out.header(part, TileHeader::Cached { slot });
                }
            }
        }
        Ok(())
    }

    /// The progressive still of a run: each tile a first pass, a whole tile
    /// the client's cache keeps a draw of its slot, which leaves the client
    /// holding its pixels and no coefficients.
    fn progressive_still(
        &mut self,
        area: Area,
        now: Duration,
        out: &mut Out,
    ) -> Result<(), TilesError> {
        let tiles: Vec<StillTile> = tiles_of(area)
            .map(|(cell, part)| {
                let whole = part.w() == TILE && part.h() == TILE && part == self.cell_area(cell);
                (
                    cell,
                    part,
                    whole.then(|| {
                        self.keys
                            .remove(&cell)
                            .unwrap_or_else(|| tile_key(&self.mirror, self.stride, part))
                    }),
                )
            })
            .collect();
        for (cell, part, key) in tiles {
            if let Some((slot, kept)) = key.and_then(|(hash, _)| self.cache.find(hash)) {
                self.cache.touch(slot);
                out.header(part, TileHeader::Cached { slot });
                self.progressive
                    .as_mut()
                    .expect("progressive")
                    .forget_tile(cell);
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
    /// tile's key, as one WebP, or as many as its width takes ([`WEBP_TILES`]),
    /// and store every whole tile of each that is not one colour in the
    /// client's cache, after it.
    fn coded(
        &mut self,
        tiles: &[StillTile],
        change: Option<Duration>,
        out: &mut Out,
    ) -> Result<(), TilesError> {
        // A WebP has a longest side: a run wider than it is as many, cut on
        // the tile grid.
        if tiles.len() > WEBP_TILES {
            for run in tiles.chunks(WEBP_TILES) {
                self.coded(run, change, out)?;
            }
            return Ok(());
        }
        let (Some(first), Some(last)) = (tiles.first(), tiles.last()) else {
            return Ok(());
        };
        let area = Area {
            right: last.1.right,
            ..first.1
        };
        let from = out.out.len();
        let header = TileHeader::Webp { len: 0 };
        let at = out.header(area, header);
        let pixels =
            &self.mirror[usize::from(area.top) * self.stride + usize::from(area.left) * 4..];
        if let Err(e) = webp::encode(pixels, self.stride, area.w(), area.h(), out.out) {
            out.out.truncate(from);
            out.count -= 1;
            return Err(e.into());
        }
        out.length(from, at, header)?;
        if let Some(now) = change {
            let each = ((out.out.len() - from) / tiles.len()) as u32;
            for (cell, _, _) in tiles {
                self.churn.spend(*cell, now, each);
            }
        }
        for (cell, part, key) in tiles {
            if area.contains(self.cell_area(*cell)) {
                self.debts.remove(cell);
            }
            if let Some((hash, false)) = key {
                let slot = self.cache.store(*hash, true);
                out.header(*part, TileHeader::Store { slot });
            }
        }
        Ok(())
    }

    /// The regions laid out again from the churn, and every stream with
    /// pixels waiting coded.
    fn round(&mut self, now: Duration, out: &mut Out) -> Result<(), TilesError> {
        // Ahead of the layout, which would end one of them as idle or for
        // another region and lay that out at once, with no word that the
        // coder makes no frame.
        self.expire_unstarted(now);
        let moving = self.churn.moving(now);
        self.retune(&moving, now, out)?;
        let stride = self.stride;
        for live in &mut self.live {
            live.carried = false;
            if !live.dirty {
                continue;
            }
            let at = usize::from(live.area.top) * stride + usize::from(live.area.left) * 4;
            // The outline, in the mirror for as long as the frame is read
            // from it: over the damage alone on a frame after the first,
            // whose coder keeps the rest of it from the frames before.
            let strips = self.debug.then(|| outline(live.area, self.outline));
            self.under.clear();
            for (strip, paint) in strips
                .iter()
                .flatten()
                .flat_map(|strip| [(strip, false), (strip, true)])
            {
                let colour = live.cause.colour();
                for y in strip.top..=strip.bottom {
                    let row = usize::from(y) * stride;
                    let pixels = &mut self.mirror[row + usize::from(strip.left) * 4
                        ..row + (usize::from(strip.right) + 1) * 4];
                    if paint {
                        for pixel in pixels.as_chunks_mut::<4>().0 {
                            pixel.copy_from_slice(&colour);
                        }
                    } else {
                        self.under.extend_from_slice(pixels);
                    }
                }
            }
            let from = out.out.len();
            // Whether the rectangle was begun, which a failed read never does.
            let mut headed = false;
            let header = if live.started {
                TileHeader::Frame {
                    stream: live.id,
                    len: 0,
                }
            } else {
                TileHeader::Start {
                    stream: live.id,
                    len: 0,
                }
            };

            let coded = live
                .stream
                .read_bgrx(&self.mirror[at..], stride, Some(&live.pending))
                .and_then(|()| {
                    headed = true;
                    let header_at = out.header(live.area, header);
                    Ok((
                        live.stream.encode(!live.started, self.ceiling, out.out)?,
                        header_at,
                    ))
                });
            // The mirror as the desktop is again, the strips put back in the
            // order they were kept, the last first.
            let mut kept = self.under.len();
            for strip in strips.iter().flatten().rev() {
                for y in (strip.top..=strip.bottom).rev() {
                    let row = usize::from(y) * stride;
                    let pixels = &mut self.mirror[row + usize::from(strip.left) * 4
                        ..row + (usize::from(strip.right) + 1) * 4];
                    kept -= pixels.len();
                    pixels.copy_from_slice(&self.under[kept..kept + pixels.len()]);
                }
            }
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

    /// The streams that have not started within [`START_WITHIN`] ended,
    /// whatever moves in them, their rectangles owed whole
    /// ([`Self::recover`]), and no region laid out for [`START_RETRY`]
    /// from now.
    fn expire_unstarted(&mut self, now: Duration) {
        let before = self.live.len();
        for live in self.live.extract_if(.., |live| {
            !live.started && now.saturating_sub(live.since) >= START_WITHIN
        }) {
            self.recovery.push(live.area);
        }
        if self.live.len() != before {
            self.no_streams_until = now + START_RETRY;
            self.cover();
        }
    }

    fn cover(&mut self) {
        self.covered = self
            .live
            .iter()
            .flat_map(|live| live.cells.iter().copied())
            .collect();
    }

    fn retune(
        &mut self,
        moving: &[(u16, u16)],
        now: Duration,
        out: &mut Out,
    ) -> Result<(), TilesError> {
        if !self.live.is_empty()
            && self
                .retuned_at
                .is_some_and(|at| now.saturating_sub(at) < RETUNE)
        {
            return Ok(());
        }
        self.retuned_at = Some(now);
        if self.picture != TilesPicture::Held {
            return Ok(());
        }
        // A region whose motion has kept to a smaller box for
        // [`SHRINK_AFTER`] is judged again from here, as one a single change
        // began is: what was dear when it was laid out may have stopped, and
        // what still moves in it be a bar, which no region is laid out again
        // for. Not while no region may be laid out.
        if now >= self.no_streams_until {
            for live in &mut self.live {
                if live
                    .smaller
                    .is_some_and(|(since, _)| now.saturating_sub(since) >= SHRINK_AFTER)
                {
                    live.dear = false;
                }
            }
        }
        // Worth a stream: a region whose motion is known dear carries some
        // of it already; or its stills came to more than a link is owed
        // lossless; or, under a region yet to be judged, where there are no
        // stills to count, some of its tiles coded to see. A region a
        // single change began is judged from the layout after its first,
        // [`RETUNE`] on ([`Self::held_stills`]): by then most of the churn's
        // window is since the change, and its second, after which nothing
        // moving in it ends it, is not yet up. What was found dear under
        // one is not coded again: none of a region that cannot be ended for
        // a region of it, every stream being taken, is coded at every
        // layout.
        let judged: HashMap<(u16, u16), Option<u8>> = self
            .live
            .iter()
            .flat_map(|live| {
                let id = (!live.dear).then_some(live.id);
                live.cells.iter().map(move |cell| (*cell, id))
            })
            .collect();
        let mut found_dear: Vec<u8> = Vec::new();
        let worthy = components(moving, self.stream_cells, |cells| {
            let mut under: Vec<u8> = Vec::new();
            for cell in cells {
                match judged.get(cell) {
                    Some(None) => return true,
                    Some(Some(id)) if !under.contains(id) => under.push(*id),
                    _ => {}
                }
            }
            if under.is_empty() {
                return cells
                    .iter()
                    .map(|cell| u64::from(self.churn.spent(*cell)))
                    .sum::<u64>()
                    >= MOVING_BYTES;
            }
            let dear = self.probe(cells, now) >= MOVING_BYTES / 2;
            if dear {
                found_dear.extend(under);
            }
            dear
        });
        let busy: HashSet<(u16, u16)> = moving
            .iter()
            .copied()
            .filter(|cell| worthy.iter().any(|c| c.bbox.contains(*cell)))
            .collect();
        for live in &mut self.live {
            if live.cells.iter().any(|cell| busy.contains(cell)) {
                live.moving_at = now;
            }
            live.dear |= found_dear.contains(&live.id);
        }
        let size = self.size;
        // Each box wanted, and how many tiles move in it.
        let wanted: Vec<(Area, u32, CellBox)> = merge(worthy, MAX_STREAMS)
            .into_iter()
            .filter_map(|c| Some((c.bbox.to_area(size)?, c.moving, c.bbox)))
            .collect();
        let (mut next, mut old): (Vec<Live>, Vec<Live>) = std::mem::take(&mut self.live)
            .into_iter()
            .partition(|live| {
                let mut touched = wanted
                    .iter()
                    .filter(|(area, ..)| live.area.intersect(*area).is_some());
                // A region a single change began is the box around all of
                // it: kept for what moves in it only where the tiles that
                // move are half of it or more, as a merge's box is, and
                // otherwise ended for regions of what moves.
                let moving: u32 = touched.clone().map(|(_, moving, _)| moving).sum();
                touched.clone().next().is_some()
                    && touched.all(|(area, ..)| live.area.contains(*area))
                    && (live.cause == Cause::Moving
                        || live.cells.len() <= (MERGE_WASTE * moving) as usize)
            });
        let outlined = self.debug.then_some(self.outline);
        for live in &mut next {
            live.moving_at = now;
            // Kept for what moves in it, which only one judged is.
            if live.cause != Cause::Moving {
                live.cause = Cause::Moving;
                // Its outline is another colour from here: read again.
                for strip in outlined.iter().flat_map(|thick| outline(live.area, *thick)) {
                    live.dirty = true;
                    live.pending.push(strip.within(live.area));
                }
            }
        }
        let mut unsent = Vec::new();
        // A region kept whose motion has kept to a smaller box for
        // [`SHRINK_AFTER`] is laid out again over the box around all that
        // moved in it since: what a scroll or a window's opening made a
        // video of is the video's own region soon after, and the rest of it
        // settles. Not while no region may be laid out, or the box would be
        // stills; and the stream it has is ended only once the coder has
        // opened the one over the box, a coder that refuses that leaving it
        // as it is, its motion counted from here again.
        let mut at = 0;
        while at < next.len() {
            let live = &mut next[at];
            let moved = wanted
                .iter()
                .filter(|(area, ..)| live.area.contains(*area))
                .map(|(.., bbox)| *bbox)
                .reduce(|a, b| a.union(&b));
            let cells = live.cells.len() as u32;
            let within = |bbox: &CellBox| bbox.area() * MERGE_WASTE <= cells;
            live.smaller = match (moved, live.smaller) {
                (Some(bbox), Some((since, all))) if within(&all.union(&bbox)) => {
                    Some((since, all.union(&bbox)))
                }
                (Some(bbox), None) if within(&bbox) => Some((now, bbox)),
                _ => None,
            };
            let shrunk = live
                .smaller
                .filter(|(since, _)| now.saturating_sub(*since) >= SHRINK_AFTER)
                .and_then(|(_, all)| all.to_area(size))
                .filter(|_| now >= self.no_streams_until);
            let Some(area) = shrunk else {
                at += 1;
                continue;
            };
            let id = next[at].id;
            match self.build(id, area, now, Cause::Moving) {
                Ok(over) => {
                    next.swap_remove(at)
                        .end(out, &mut unsent, &mut self.recovery);
                    next.push(over);
                }
                Err(_) => {
                    next[at].smaller = None;
                    at += 1;
                }
            }
        }
        for (area, ..) in wanted {
            if next.iter().any(|live| live.area.contains(area)) {
                continue;
            }
            if next.len() < MAX_STREAMS && now >= self.no_streams_until {
                for live in old.extract_if(.., |live| live.area.intersect(area).is_some()) {
                    live.end(out, &mut unsent, &mut self.recovery);
                }
                let id = (0..=u8::MAX)
                    .find(|id| !next.iter().chain(&old).any(|live| live.id == *id))
                    .expect("at most a few streams live");
                next.push(self.build(id, area, now, Cause::Moving)?);
            }
        }
        for mut live in old {
            let idle = now.saturating_sub(live.moving_at) >= STREAM_IDLE;
            let overlaps = next
                .iter()
                .any(|kept| kept.area.intersect(live.area).is_some());
            if !idle && !overlaps && next.len() < MAX_STREAMS {
                // Kept for now, though for nothing that moves within it:
                // what its motion had kept to is counted anew.
                live.smaller = None;
                next.push(live);
            } else {
                live.end(out, &mut unsent, &mut self.recovery);
            }
        }
        self.live = next;
        self.cover();
        self.owe(&unsent);
        Ok(())
    }

    /// What the changes of `cells`, a moving component's tiles, would come to
    /// as stills over the churn's window, for one a region carries, which
    /// has no stills to count: at most [`PROBE_TILES`] of them coded as a
    /// WebP each and thrown away, and what a tile of those came to taken for
    /// each change of each tile in the window.
    fn probe(&self, cells: &[(u16, u16)], now: Duration) -> u64 {
        let mut scratch = Vec::new();
        let (mut bytes, mut coded) = (0u64, 0u64);
        for cell in cells.iter().step_by(cells.len().div_ceil(PROBE_TILES)) {
            let part = self.cell_area(*cell);
            let pixels =
                &self.mirror[usize::from(part.top) * self.stride + usize::from(part.left) * 4..];
            scratch.clear();
            if webp::encode(pixels, self.stride, part.w(), part.h(), &mut scratch).is_ok() {
                bytes += scratch.len() as u64;
                coded += 1;
            }
        }
        let changes: u64 = cells
            .iter()
            .map(|cell| u64::from(self.churn.recent(*cell, now)))
            .sum();
        (bytes * changes).checked_div(coded).unwrap_or(0)
    }

    /// What streams that ended did not carry, owed as stills where no stream
    /// live from here does.
    fn owe(&mut self, unsent: &[Area]) {
        let runs = runs_outside(unsent, &self.covered);
        self.stills.extend(runs);
    }

    /// The rectangles of the streams that ended without starting, sent as
    /// stills where no stream that has started carries them, whatever they
    /// come to: a region over what is past an update's stills would be the
    /// one that never started again, its tiles owed anew and never sent,
    /// where a stream that makes no frame is why it never started. A stream
    /// laid out over them that has not started either has shown the client
    /// nothing of them, so they go all the same, as runs along a row, and
    /// its START draws over them when it comes. Their tiles' debts are paid
    /// by them.
    fn recover(&mut self, now: Duration, out: &mut Out) -> Result<(), TilesError> {
        let areas = std::mem::take(&mut self.recovery);
        if areas.is_empty() {
            return Ok(());
        }
        let started: HashSet<(u16, u16)> = self
            .live
            .iter()
            .filter(|live| live.started)
            .flat_map(|live| live.cells.iter().copied())
            .collect();
        for run in runs_outside(&areas, &started) {
            self.still(run, now, false, out)?;
        }
        Ok(())
    }

    /// A stream of the region coder over `area`, every tile of it owed from now.
    fn build(
        &mut self,
        id: u8,
        area: Area,
        now: Duration,
        cause: Cause,
    ) -> Result<Live, TilesError> {
        let stream = self
            .coder
            .open(area.w(), area.h())
            .map_err(TilesError::Region)?;
        let cells: Vec<(u16, u16)> = tiles_of(area).map(|(cell, _)| cell).collect();
        for cell in &cells {
            self.debts.insert(*cell, now);
        }
        Ok(Live {
            id,
            area,
            cells,
            stream,
            pending: Vec::new(),
            dirty: true,
            started: false,
            carried: false,
            moving_at: now,
            cause,
            since: now,
            dear: cause == Cause::Moving,
            smaller: None,
        })
    }

    /// A held list's `stills`, each a run of tiles along one row, at most
    /// [`HELD_BYTES`] of them as stills in one update where the read `moved`
    /// nothing: a change past that goes as video whole, a region over the
    /// box around all of it in whole tiles ([`Self::build`]), its START a
    /// keyframe at the
    /// dial in this update after the stills, carried on by its frames while
    /// it keeps changing and ended when nothing in it has moved for a second,
    /// as any region; every tile of the box owed its still from now, which
    /// the settle pays at the link's pace ([`Self::paced_budget`]) once the
    /// stream is over, the cache's from their slots — among them the stills
    /// this update carried, which the frame draws over and which filled
    /// their slots. So the client has the whole of a change at once, lossy:
    /// a picture that starts over, and the first frames of a video, which
    /// went as the desktop whole in stills until its region formed, 1.9 MB an
    /// update on a 1080p video frame against the 0.2 of its keyframe, and
    /// here are its region from the first frame, the frames after it the
    /// stream's own. Where the read moved pixels the stills are what a scroll
    /// or a drag uncovered, which the list promises lossless, and a stream
    /// for every strip would be a coder opened an update: they all go as
    /// stills; and so does the rest where the coder refuses the stream or
    /// every stream is taken. A region live within the box is ended and the
    /// new one carries its tiles on, as a retune's does ([`Self::retune`]):
    /// no two regions over one tile. Whether a region was added, whose START
    /// is owed a round ([`Self::round`]).
    fn held_stills(
        &mut self,
        stills: Vec<Area>,
        moved: bool,
        now: Duration,
        out: &mut Out,
    ) -> Result<bool, TilesError> {
        self.precode(&stills);
        let from = out.out.len();
        let mut rest: Vec<Area> = Vec::new();
        for area in &stills {
            if moved
                || out.out.len() - from < HELD_BYTES
                || self.live.len() >= MAX_STREAMS
                || now < self.no_streams_until
            {
                self.still(*area, now, true, out)?;
            } else {
                rest.push(*area);
            }
        }
        if rest.is_empty() {
            return Ok(false);
        }
        // The box in whole tiles: a region's cells are covered whole, and a
        // change in the rest of a tile a pixel-aligned box cut would be
        // neither still nor frame while the stream lives.
        let around = stills.iter().fold(stills[0], |a, b| Area {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        });
        let around = CellBox {
            c0: around.left / TILE,
            r0: around.top / TILE,
            c1: around.right / TILE,
            r1: around.bottom / TILE,
        }
        .to_area(self.size)
        .expect("tiles of the desktop");
        let id = (0..=u8::MAX)
            .find(|id| !self.live.iter().any(|live| live.id == *id))
            .expect("at most a few streams live");
        match self.build(id, around, now, Cause::Burst) {
            Ok(live) => {
                let mut unsent = Vec::new();
                for old in self
                    .live
                    .extract_if(.., |old| old.area.intersect(around).is_some())
                {
                    old.end(out, &mut unsent, &mut self.recovery);
                }
                self.live.push(live);
                self.cover();
                self.owe(&unsent);
                // The layouts count from here, so that one comes [`RETUNE`]
                // on, where the region is judged, and not just short of its
                // second, where a tick that finds it idle could end it
                // first. The other regions' layout is later by as long as
                // it has been since the last, and each is taken to have
                // moved that much later, or a tick would find one idle that
                // only the layout it waits for says is moving.
                let late = self
                    .retuned_at
                    .map_or(Duration::ZERO, |at| now.saturating_sub(at));
                for other in &mut self.live {
                    other.moving_at = (other.moving_at + late).min(now);
                }
                self.retuned_at = Some(now);
                Ok(true)
            }
            Err(_) => {
                for area in rest {
                    self.still(area, now, true, out)?;
                }
                Ok(false)
            }
        }
    }

    /// `bytes` the reads brought at `now` counted among what the link has been
    /// carrying, the updates before [`PACE_WINDOW`] ago dropped, and the pace
    /// raised to it while anything is owed: the most the desktop's own
    /// changes were found sending over a second, whether or not a tick had
    /// anything to pay then.
    fn observe_pace(&mut self, now: Duration, bytes: usize) {
        while self
            .sent
            .front()
            .is_some_and(|(at, _)| *at + PACE_WINDOW <= now)
        {
            self.sent.pop_front();
        }
        if bytes > 0 {
            self.sent.push_back((now, bytes));
        }
        if self.owed() {
            self.pace = self.pace.max(self.carrying());
        }
    }

    /// What the link has been carrying, in bytes a second: the updates of the
    /// last [`PACE_WINDOW`].
    fn carrying(&self) -> usize {
        let bytes: usize = self.sent.iter().map(|(_, bytes)| bytes).sum();
        bytes * 1000 / PACE_WINDOW.as_millis() as usize
    }

    /// The bytes the reads brought within `within` of `now`: what the
    /// desktop's own changes have had of the link, which a tick fills up to
    /// the pace and no further.
    fn brought_within(&self, now: Duration, within: Duration) -> usize {
        self.sent
            .iter()
            .filter(|(at, _)| *at + within > now)
            .map(|(_, bytes)| bytes)
            .sum()
    }

    /// How far apart a held list's ticks come: [`CLEANUP_TICK`], or as soon
    /// as the pace has [`SETTLE_MOST`] going where that is sooner, down to
    /// [`SETTLE_TICK_LEAST`].
    fn tick_interval(&self) -> Duration {
        if self.pace == 0 {
            return CLEANUP_TICK;
        }
        Duration::from_secs_f64(SETTLE_MOST as f64 / self.pace as f64)
            .clamp(SETTLE_TICK_LEAST, CLEANUP_TICK)
    }

    /// The bytes a held list's tick may send: the eased pace's share of the
    /// tick's interval, less what the reads sent in it, between
    /// [`SETTLE_FLOOR`]'s share of it and [`SETTLE_MOST`]. The pace is the
    /// most the desktop's own changes were found sending over a second since
    /// something was last owed — a stream's frames, a picture's keyframe and
    /// stills, and none of the ticks, which would raise the rate they keep
    /// to — and the settle eases in to it: its rate rises from nothing to
    /// the pace over the [`SETTLE_EASE`] after it began paying, the floor
    /// under it. So a desktop that stops settles at no more than the rate
    /// its motion had of the link, reached a step at a time: the link sees
    /// no more than it did a moment before, and no step up. While the
    /// desktop moves the reads have the pace, and the tick its floor.
    fn paced_budget(&mut self, now: Duration) -> usize {
        self.pace = self.pace.max(self.carrying());
        let interval = self.tick_interval();
        let from = *self.easing_from.get_or_insert(now);
        let eased = ((now.saturating_sub(from) + interval).as_secs_f64()
            / SETTLE_EASE.as_secs_f64())
        .min(1.0);
        let share = (self.pace as f64 * eased * interval.as_secs_f64()) as usize;
        let floor =
            (SETTLE_FLOOR as f64 * interval.as_secs_f64() / CLEANUP_TICK.as_secs_f64()) as usize;
        share
            .saturating_sub(self.brought_within(now, interval))
            .clamp(floor, SETTLE_MOST)
    }

    /// What a held list's settle takes `part` of the tile `cell` to cost: how
    /// many of its pixels differ from the one to their left, the first of
    /// each row among them, which is what a lossless coder has to spell out;
    /// or `None` for a whole tile the cache keeps lossless, which is a draw.
    fn settle_activity(&self, cell: (u16, u16), part: Area) -> Option<usize> {
        if part.w() == TILE
            && part.h() == TILE
            && part == self.cell_area(cell)
            && self
                .cache
                .find(tile_key(&self.mirror, self.stride, part).0)
                .is_some_and(|(_, kept)| kept)
        {
            return None;
        }
        let mut differ = 0;
        for y in part.top..=part.bottom {
            let row = usize::from(y) * self.stride;
            let pixels = self.mirror
                [row + usize::from(part.left) * 4..row + (usize::from(part.right) + 1) * 4]
                .as_chunks::<4>()
                .0;
            differ += 1 + pixels
                .windows(2)
                .filter(|pair| pair[0][..3] != pair[1][..3])
                .count();
        }
        Some(differ)
    }

    /// The streams whose motion stopped ended, and the tiles still for
    /// [`CLEANUP_IDLE`] settled — a WebP, or a progressive tile's next
    /// upgrade — as runs along a row, the oldest
    /// runs first until the tick's bytes are sent: on a held list its eased
    /// pace's share ([`Self::paced_budget`]), each run cut to what is left of
    /// it by what its tiles are taken to cost ([`Self::settle_activity`]), so
    /// no tick is past its bytes by more than a piece that cost more than it
    /// was taken to; on a walked one [`SETTLE_BYTES`] while
    /// the walk stands at
    /// the ceiling, below it [`SETTLE_BYTES_LEAST`] while the desktop moves,
    /// and once it is quiet the bytes the link was found to take
    /// ([`Self::delivered`]); of the [`SETTLE_TILES`]
    /// longest owed — none while the walk is
    /// coarse, since the link is behind on what it has, until the desktop has
    /// been still for [`COARSE_QUIET`]. Whether it stopped short of a tile
    /// that was ready.
    fn tick(&mut self, now: Duration, out: &mut Out) -> Result<bool, TilesError> {
        let mut unsent = Vec::new();
        // A stream that has not started within its second, whatever moves
        // in it and however often the layouts keep it, of which the client
        // holds nothing; and one nothing has moved in for its own.
        self.expire_unstarted(now);
        for live in self
            .live
            .extract_if(.., |live| now.saturating_sub(live.moving_at) >= STREAM_IDLE)
        {
            live.end(out, &mut unsent, &mut self.recovery);
        }
        self.cover();
        self.owe(&unsent);
        // Ahead of the settle: a stream that never started left its tiles
        // owed since it was laid out, which the settle would pay in part
        // and the recovery then send again whole.
        self.recover(now, out)?;
        if self.coarse && now.saturating_sub(self.changed_at) < COARSE_QUIET {
            return Ok(false);
        }

        let mut ready: Vec<((u16, u16), Duration)> = self
            .debts
            .iter()
            .filter(|(cell, at)| {
                !self.covered.contains(*cell) && now.saturating_sub(**at) >= CLEANUP_IDLE
            })
            .map(|(cell, at)| (*cell, *at))
            .collect();
        // The coarsest progressive tiles first, so the picture is refined
        // all over a stage at a time, as the host's is; then the longest owed.
        let stage = |cell: (u16, u16)| {
            self.progressive
                .as_ref()
                .and_then(|enc| enc.stage_of(cell))
                .unwrap_or(0)
        };
        ready.sort_unstable_by_key(|&(cell, at)| (stage(cell), at, cell.1, cell.0));
        let mut more = ready.len() > SETTLE_TILES;
        ready.truncate(SETTLE_TILES);
        ready.sort_unstable_by_key(|((col, row), _)| (*row, *col));
        // Each run with the coarsest stage of its tiles and the time its
        // oldest was owed from, and sent coarsest, then oldest, first.
        let mut runs: Vec<(CellBox, (usize, Duration))> = Vec::new();
        for ((col, row), at) in ready {
            let first = (stage((col, row)), at);
            match runs.last_mut() {
                Some((run, oldest)) if run.r0 == row && run.c1.checked_add(1) == Some(col) => {
                    run.c1 = col;
                    *oldest = (*oldest).min(first);
                }
                _ => runs.push((CellBox::of((col, row)), first)),
            }
        }
        runs.sort_by_key(|(run, oldest)| (*oldest, run.r0, run.c0));
        // At the ceiling the walk found the link with room: the most. Below
        // it the link is short of bytes, for the motion itself while the
        // desktop moves: the least; once it is quiet, what the link was found
        // to take.
        let budget = if self.picture == TilesPicture::Held {
            if runs.is_empty() {
                // Nothing to pay: the next settle eases in from its start.
                self.easing_from = None;
                self.settle_rate = SETTLE_RATE_GUESS;
                return Ok(more);
            }
            self.paced_budget(now)
        } else if self.quality >= self.ceiling {
            SETTLE_BYTES
        } else if self.quiet(now) {
            self.settle_bytes
        } else {
            SETTLE_BYTES_LEAST
        };
        let from = out.out.len();
        'runs: for (run, _) in runs {
            if out.out.len() - from >= budget {
                more = true;
                break;
            }
            if let Some(area) = run.to_area(self.size) {
                if self.progressive.is_some() {
                    // A whole tile not one colour that settles is stored
                    // settled, as a WebP is. The bytes are counted a tile at
                    // a time, a row of them being up to 60 of a 4K desktop.
                    for (cell, part) in tiles_of(area) {
                        if out.out.len() - from >= budget {
                            more = true;
                            break 'runs;
                        }
                        let settled =
                            self.progressive_tile(cell, ProgressivePass::Upgrade, now, out)?;
                        more |= !settled;
                        if settled
                            && part == self.cell_area(cell)
                            && part.w() == TILE
                            && part.h() == TILE
                            && let (hash, false) = tile_key(&self.mirror, self.stride, part)
                        {
                            let slot = self.cache.store(hash, true);
                            out.header(part, TileHeader::Store { slot });
                        }
                    }
                } else {
                    // The run cut to what is left of the budget, each tile
                    // taken to cost what its pixels' changes along their
                    // rows did of the piece before, and one the cache keeps
                    // its draw: a row of a photograph is more than a tick's
                    // bytes, and sent whole would be the burst the pace is
                    // there to keep off the link.
                    let parts: Vec<(Area, Option<usize>)> = tiles_of(area)
                        .map(|(cell, part)| (part, self.settle_activity(cell, part)))
                        .collect();
                    let mut at = 0;
                    while at < parts.len() {
                        let left = budget.saturating_sub(out.out.len() - from);
                        if left == 0 {
                            more = true;
                            break 'runs;
                        }
                        // Half of what is left at most, so a piece that costs
                        // more than it was taken to overshoots by little; its
                        // first tile whatever it costs.
                        let (mut end, mut cost, mut activity) = (at, 0.0, 0);
                        while let Some((_, tile)) = parts.get(end) {
                            let more = match tile {
                                Some(activity) => {
                                    SETTLE_TILE_LEAST + *activity as f64 * self.settle_rate
                                }
                                None => SETTLE_TILE_LEAST,
                            };
                            if end > at && cost + more > left as f64 / 2.0 {
                                break;
                            }
                            cost += more;
                            activity += tile.unwrap_or(0);
                            end += 1;
                        }
                        let piece = Area {
                            left: parts[at].0.left,
                            right: parts[end - 1].0.right,
                            ..area
                        };
                        let before = out.out.len();
                        self.still(piece, now, false, out)?;
                        // A piece with little in it says little of the next.
                        if activity >= usize::from(TILE) {
                            let rate = (out.out.len() - before) as f64 / activity as f64;
                            self.settle_rate = rate.max(self.settle_rate / 2.0);
                        }
                        at = end;
                    }
                }
            }
        }
        Ok(more)
    }
}

/// The encoder read back as a client would: every rectangle parsed by the
/// client's parser, each WebP by `image-webp`'s decoder ([`super::webp::decoder`]), plain Rust
/// that shares nothing with the libwebp it was coded by, each progressive tile by the tests' own RemoteFX Progressive
/// decoder ([`super::progressive::decoder`]), and each region by reading back the
/// frames of a coder standing in for the daemon's H.264 one, whose own tests
/// decode it with libavcodec.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rfb::client::{self, RectBody, ServerMsg, TileBody};

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
        /// The progressive tiles' decoder, the tests' own.
        decoder: super::progressive::decoder::Decoder,
        progressives: usize,
        medians: usize,
        /// The still stream as the page reads it.
        stills: super::planar::decoder::StillReader,
        still_count: usize,
        /// The STILLs that started the stream over.
        still_starts: usize,
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
            Self {
                w: usize::from(w),
                canvas: vec![0; usize::from(w) * usize::from(h) * 4],
                streams: HashMap::new(),
                slots: HashMap::new(),
                decoder: Default::default(),
                progressives: 0,
                medians: 0,
                stills: Default::default(),
                still_count: 0,
                still_starts: 0,
                webps: 0,
                starts: 0,
                frames: 0,
                ends: 0,
                copies: 0,
                cached: 0,
                stored: 0,
                bytes: 0,
            }
        }

        /// Packed RGB drawn at the rectangle.
        fn paste(&mut self, (x, y, w, h): Place, rgb: &[u8]) {
            for row in 0..usize::from(h) {
                for col in 0..usize::from(w) {
                    let at = ((usize::from(y) + row) * self.w + usize::from(x) + col) * 4;
                    let from = (row * usize::from(w) + col) * 3;
                    self.canvas[at..at + 3].copy_from_slice(&[
                        rgb[from + 2],
                        rgb[from + 1],
                        rgb[from],
                    ]);
                }
            }
        }

        /// One update of `count` rectangles, as the encoder appended them.
        fn draw(&mut self, count: u16, rects: &[u8]) {
            let mut wire = crate::rfb::msg::update_header(count).to_vec();
            wire.extend_from_slice(rects);
            self.bytes += rects.len();
            let (ServerMsg::Update(rects), used) =
                client::parse(&wire).unwrap().expect("a whole update")
            else {
                panic!("an update")
            };
            assert_eq!(used, wire.len(), "the update is its rectangles");
            // An update's median bodies read together, as the page reads a batch's.
            let mut medians = super::median::decoder::unpack_each(
                &rects
                    .iter()
                    .filter_map(|rect| match &rect.body {
                        RectBody::Tiles(TileBody::Median(body)) => {
                            Some((&body[..], usize::from(rect.width), usize::from(rect.height)))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            )
            .into_iter();
            for rect in rects {
                let RectBody::Tiles(body) = rect.body else {
                    panic!("a tiles rectangle")
                };
                let (x, y, w, h) = (rect.x, rect.y, rect.width, rect.height);
                match body {
                    TileBody::Webp(webp) => {
                        self.webps += 1;
                        let (pw, ph, rgb) = super::webp::decoder::decode(&webp);
                        assert_eq!(
                            (pw, ph),
                            (u32::from(w), u32::from(h)),
                            "a WebP of the rectangle"
                        );
                        self.paste((x, y, w, h), &rgb);
                    }
                    TileBody::Start { stream, frame } => {
                        self.starts += 1;
                        assert!(
                            !self.streams.contains_key(&stream),
                            "stream {stream} started while live"
                        );
                        assert_eq!(frame[0], Fake::KEYFRAME, "a stream starts at its keyframe");
                        self.streams.insert(stream, (x, y, w, h));
                        self.region(stream, &frame);
                    }
                    TileBody::Frame { stream, frame } => {
                        self.frames += 1;
                        assert_eq!(
                            self.streams.get(&stream),
                            Some(&(x, y, w, h)),
                            "a frame of a stream live over its rectangle"
                        );
                        assert_eq!(frame[0], Fake::FRAME, "a keyframe after the start");
                        self.region(stream, &frame);
                    }
                    TileBody::End { stream } => {
                        self.ends += 1;
                        assert_eq!(
                            self.streams.remove(&stream),
                            Some((x, y, w, h)),
                            "the end of a stream live over its rectangle"
                        );
                    }
                    TileBody::Copy {
                        x: from_x,
                        y: from_y,
                    } => {
                        self.copies += 1;
                        let line = usize::from(w) * 4;
                        assert!(
                            usize::from(from_x) * 4 + line <= self.w * 4
                                && (usize::from(from_y) + usize::from(h)) * self.w * 4
                                    <= self.canvas.len(),
                            "a copy from inside the desktop"
                        );
                        // Copied out first, so a source the rectangle overlaps is read whole.
                        let source: Vec<u8> = (0..usize::from(h))
                            .flat_map(|row| {
                                let at = ((usize::from(from_y) + row) * self.w
                                    + usize::from(from_x))
                                    * 4;
                                self.canvas[at..at + line].to_vec()
                            })
                            .collect();
                        for (row, pixels) in source.chunks_exact(line).enumerate() {
                            let at = ((usize::from(y) + row) * self.w + usize::from(x)) * 4;
                            self.canvas[at..at + line].copy_from_slice(pixels);
                        }
                    }
                    TileBody::Store { slot } => {
                        self.stored += 1;
                        assert!(
                            (w, h) == (TILE, TILE) && x % TILE == 0 && y % TILE == 0,
                            "a store of one tile on the grid, not {w}x{h}+{x}+{y}"
                        );
                        let line = usize::from(TILE) * 4;
                        let tile: Vec<u8> = (0..usize::from(TILE))
                            .flat_map(|row| {
                                self.canvas
                                    [((usize::from(y) + row) * self.w + usize::from(x)) * 4..]
                                    [..line]
                                    .to_vec()
                            })
                            .collect();
                        self.slots.insert(slot, tile);
                    }
                    TileBody::Cached { slot } => {
                        self.cached += 1;
                        assert_eq!((w, h), (TILE, TILE), "a cached draw of one tile");
                        let tile = self.slots.get(&slot).unwrap_or_else(|| {
                            panic!("slot {slot} drawn from before it was stored")
                        });
                        let line = usize::from(TILE) * 4;
                        for (row, pixels) in tile.chunks_exact(line).enumerate() {
                            let at = ((usize::from(y) + row) * self.w + usize::from(x)) * 4;
                            self.canvas[at..at + line].copy_from_slice(pixels);
                        }
                    }
                    TileBody::Median(_) => {
                        self.medians += 1;
                        let rgb = medians
                            .next()
                            .expect("a result for every median body")
                            .expect("a median body of the rectangle");
                        self.paste((x, y, w, h), &rgb);
                    }
                    TileBody::Still(body) => {
                        self.still_count += 1;
                        self.still_starts +=
                            usize::from(body.starts_with(&super::planar::decoder::ZSTD_MAGIC));
                        let rgb = self
                            .stills
                            .unpack(&body, usize::from(w), usize::from(h))
                            .expect("the still stream's bitmap of the rectangle");
                        self.paste((x, y, w, h), &rgb);
                    }
                    TileBody::Progressive(block) => {
                        self.progressives += 1;
                        let cell =
                            wlshare_proto::progressive::tile_of(&block).expect("a tile block");
                        assert!(
                            x == cell.0 * TILE && y == cell.1 * TILE && w <= TILE && h <= TILE,
                            "the block's tile is the rectangle's: {cell:?} at {w}x{h}+{x}+{y}"
                        );
                        let shown = self.decoder.decode(&wlshare_proto::progressive::stream(
                            0,
                            &[(x, y, w, h)],
                            &[&block],
                        ));
                        let [(_, tile)] = &shown[..] else {
                            panic!("one tile")
                        };
                        let line = usize::from(w) * 4;
                        for row in 0..usize::from(h) {
                            let at = ((usize::from(y) + row) * self.w + usize::from(x)) * 4;
                            self.canvas[at..at + line]
                                .copy_from_slice(&tile[row * usize::from(TILE) * 4..][..line]);
                        }
                    }
                }
            }
        }

        fn region(&mut self, stream: u8, frame: &[u8]) {
            let (x, y, w, h) = self.streams[&stream];
            let line = usize::from(w) * 4;
            assert_eq!(
                frame.len(),
                1 + line * usize::from(h),
                "a fake frame is the region's pixels"
            );
            for (row, pixels) in frame[1..].chunks_exact(line).enumerate() {
                let at = ((usize::from(y) + row) * self.w + usize::from(x)) * 4;
                self.canvas[at..at + line].copy_from_slice(pixels);
            }
        }

        /// How many pixels differ from `pixels` in B, G or R.
        fn off(&self, pixels: &[u8]) -> usize {
            self.canvas
                .as_chunks::<4>()
                .0
                .iter()
                .zip(pixels.as_chunks::<4>().0)
                .filter(|(a, b)| a[..3] != b[..3])
                .count()
        }

        /// How close the page is to `pixels`, the PSNR over B, G and R.
        fn psnr(&self, pixels: &[u8]) -> f64 {
            let se: f64 = self
                .canvas
                .as_chunks::<4>()
                .0
                .iter()
                .zip(pixels.as_chunks::<4>().0)
                .flat_map(|(a, b)| a[..3].iter().zip(&b[..3]))
                .map(|(a, b)| f64::from(a.abs_diff(*b)).powi(2))
                .sum();
            if se == 0.0 {
                f64::INFINITY
            } else {
                10.0 * (255.0f64.powi(2) * (self.canvas.len() / 4 * 3) as f64 / se).log10()
            }
        }

        /// How far the page is from `pixels`: the sum over every sample of B,
        /// G and R of how much it differs.
        fn distance(&self, pixels: &[u8]) -> u64 {
            self.canvas
                .as_chunks::<4>()
                .0
                .iter()
                .zip(pixels.as_chunks::<4>().0)
                .flat_map(|(a, b)| a[..3].iter().zip(&b[..3]))
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum()
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
            pixels[at..at + usize::from(rect.width) * 4].copy_from_slice(
                &fill[row * usize::from(rect.width) * 4..][..usize::from(rect.width) * 4],
            );
        }
    }

    /// Paint `rect` of a `w`-wide picture as a page of black glyphs on white,
    /// lines of dots and gaps: text, two colours with sharp edges.
    fn glyphs(pixels: &mut [u8], w: u16, rect: Rect) {
        for row in 0..usize::from(rect.height) {
            for col in 0..usize::from(rect.width) {
                let at =
                    ((usize::from(rect.y) + row) * usize::from(w) + usize::from(rect.x) + col) * 4;
                let v = if (col * 7 + row * 13) % 11 < 2 && row % 12 < 9 {
                    0
                } else {
                    255
                };
                pixels[at..at + 4].copy_from_slice(&[v, v, v, 0]);
            }
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A list that holds the dial.
    fn held() -> TilesPicture {
        TilesPicture::Held
    }

    /// A list that lets the dial walk: every tile progressive.
    fn progressive() -> TilesPicture {
        TilesPicture::Progressive
    }

    /// A list that asks for every tile lossless: planar where it is still,
    /// median residuals where it moves.
    fn lossless() -> TilesPicture {
        TilesPicture::Lossless
    }

    /// A desktop a progressive tile settles close to: a soft gradient with
    /// text over it.
    fn soft(w: u16, h: u16, seed: u32) -> Vec<u8> {
        let mut pixels: Vec<u8> = (0..usize::from(h))
            .flat_map(|y| {
                (0..usize::from(w)).flat_map(move |x| {
                    [
                        (x / 2 + seed as usize) as u8,
                        (y / 2) as u8,
                        ((x + y) / 4) as u8,
                        0,
                    ]
                })
            })
            .collect();
        glyphs(
            &mut pixels,
            w,
            Rect {
                x: 16,
                y: 8,
                width: w - 32,
                height: h / 2,
            },
        );
        pixels
    }

    /// A region coder standing in for the daemon's H.264 one: its frames are
    /// the region's pixels, each sample's lowest bit dropped, so a stream's
    /// picture is not lossless, behind a byte that says keyframe or not. It can
    /// be told to refuse every stream, or to fail one after so many frames, at
    /// its read or at its encode.
    struct Fake {
        refuse: bool,
        /// Every stream makes no frame of what it reads.
        mute: bool,
        fail_after: Option<usize>,
        fail_read_after: Option<usize>,
    }

    impl Fake {
        const KEYFRAME: u8 = 0xC0;
        const FRAME: u8 = 0xDE;

        fn coder() -> Box<dyn H264Coder> {
            Box::new(Self {
                refuse: false,
                mute: false,
                fail_after: None,
                fail_read_after: None,
            })
        }
    }

    struct FakeStream {
        w: usize,
        h: usize,
        picture: Vec<u8>,
        filled: bool,
        mute: bool,
        frames: usize,
        fail_after: Option<usize>,
        fail_read_after: Option<usize>,
    }

    impl H264Coder for Fake {
        fn open(&mut self, width: u16, height: u16) -> Result<Box<dyn H264Stream>, H264Error> {
            if self.refuse {
                return Err("the device is gone".into());
            }
            let (w, h) = (usize::from(width), usize::from(height));
            Ok(Box::new(FakeStream {
                w,
                h,
                picture: vec![0; w * h * 4],
                filled: false,
                mute: self.mute,
                frames: 0,
                fail_after: self.fail_after,
                fail_read_after: self.fail_read_after,
            }))
        }
    }

    impl H264Stream for FakeStream {
        fn read_bgrx(
            &mut self,
            pixels: &[u8],
            stride: usize,
            changed: Option<&[Rect]>,
        ) -> Result<(), H264Error> {
            if self
                .fail_read_after
                .is_some_and(|after| self.frames >= after)
            {
                return Err("the upload failed".into());
            }
            let whole = [Rect {
                x: 0,
                y: 0,
                width: self.w as u16,
                height: self.h as u16,
            }];
            let rects = match changed {
                Some(rects) if self.filled => rects,
                _ => &whole,
            };
            for r in rects {
                for row in 0..usize::from(r.height) {
                    let (to, from) = (
                        ((usize::from(r.y) + row) * self.w + usize::from(r.x)) * 4,
                        (usize::from(r.y) + row) * stride + usize::from(r.x) * 4,
                    );
                    self.picture[to..to + usize::from(r.width) * 4]
                        .copy_from_slice(&pixels[from..from + usize::from(r.width) * 4]);
                }
            }
            self.filled = true;
            Ok(())
        }

        fn encode(
            &mut self,
            keyframe: bool,
            _quality: u8,
            out: &mut Vec<u8>,
        ) -> Result<bool, H264Error> {
            if self.fail_after.is_some_and(|after| self.frames >= after) {
                return Err("the encoder hung".into());
            }
            if self.mute {
                return Ok(false);
            }
            out.push(if keyframe || self.frames == 0 {
                Fake::KEYFRAME
            } else {
                Fake::FRAME
            });
            out.extend(self.picture.iter().map(|sample| sample & !1));
            self.frames += 1;
            Ok(true)
        }
    }

    /// Read and encode, and draw what came out.
    fn step(
        encoder: &mut TilesEncoder,
        page: &mut Page,
        pixels: &[u8],
        changed: Option<&[Rect]>,
        now: Duration,
    ) -> u16 {
        update(encoder, page, pixels, changed, now).rects
    }

    /// Read and encode, and draw what came out, and say what the update was.
    fn update(
        encoder: &mut TilesEncoder,
        page: &mut Page,
        pixels: &[u8],
        changed: Option<&[Rect]>,
        now: Duration,
    ) -> Update {
        let stride = usize::from(encoder.size().0) * 4;
        encoder.read(pixels, stride, changed, now).unwrap();
        let mut out = Vec::new();
        let update = encoder.encode(now, &mut out).unwrap();
        encoder.delivered(Duration::ZERO);
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
            encoder.delivered(Duration::ZERO);
            page.draw(update.rects, &out);
            assert!(now < ms(60_000), "never settled");
        }
        now
    }

    /// The first picture of a held list, at 0, and the settle after it where
    /// it was more than an update's stills and went as video first; lossless
    /// either way. When it was.
    fn begin(encoder: &mut TilesEncoder, page: &mut Page, pixels: &[u8]) -> Duration {
        let first = update(encoder, page, pixels, None, ms(0));
        assert!(
            first.keyframe,
            "a picture sent whole crosses the link as a keyframe"
        );
        let at = settle(encoder, page, ms(0));
        assert_eq!(
            page.off(pixels),
            0,
            "the first picture lossless, at once or settled"
        );
        at
    }

    /// A first picture within an update's stills goes whole as WebP, a band at a
    /// time, lossless; a palette where a band has few colours and RGB where it
    /// has many, both read back.
    #[test]
    fn a_first_picture_is_sent_lossless_in_bands() {
        let (w, h) = (200u16, 150u16);
        let mut pixels = noise(w, h, 1);
        // The last band, rows 128–149, one flat colour, which a palette holds;
        // the two above it noise, which none does.
        for row in 128..usize::from(h) {
            pixels[row * usize::from(w) * 4..][..usize::from(w) * 4]
                .copy_from_slice(&[10, 20, 30, 0].repeat(usize::from(w)));
        }
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let first = update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 5,
                y: 5,
                width: 1,
                height: 1,
            }]),
            ms(0),
        );
        assert_eq!(
            (first.rects, page.webps, page.stored),
            (9, 3, 6),
            "the whole picture in three bands, whatever the read was told, and its six whole tiles stored"
        );
        assert!(
            first.keyframe,
            "a picture sent whole crosses the link as a keyframe"
        );
        assert_eq!(page.off(&pixels), 0);
        assert!(
            encoder.tick_at().is_none(),
            "a picture sent lossless owes nothing"
        );
        paint(
            &mut pixels,
            w,
            Rect {
                x: 70,
                y: 70,
                width: 10,
                height: 10,
            },
            9,
        );
        let next = update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 70,
                y: 70,
                width: 10,
                height: 10,
            }]),
            ms(100),
        );
        assert!(
            next.rects == 1 && !next.keyframe,
            "a tile that changed is no keyframe: {next:?}"
        );
    }

    /// A first pass below the full stage is owed its upgrades once still;
    /// coarse, they wait until the desktop has been quiet.
    #[test]
    fn a_first_pass_is_owed_its_upgrades_which_wait_while_coarse() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 12);
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let now = settle(&mut encoder, &mut page, ms(0));
        assert!(!encoder.owed());

        encoder.set_pace(30, false);
        let busy = Rect {
            x: 64,
            y: 0,
            width: 64,
            height: 64,
        };
        paint(&mut pixels, w, busy, 13);
        let sent = page.progressives;
        let first = update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[busy]),
            now + ms(100),
        );
        assert_eq!(
            (page.progressives - sent, first.keyframe),
            (1, false),
            "one tile that changed, no keyframe"
        );
        assert!(
            encoder.owed() && encoder.tick_at().is_some(),
            "a first pass below the full stage is owed"
        );
        let coarse = page.distance(&pixels);
        let now = settle(&mut encoder, &mut page, now + ms(100));
        assert!(
            page.distance(&pixels) < coarse,
            "finer once upgraded: {} from {coarse}",
            page.distance(&pixels)
        );
        assert!(!encoder.owed());

        // Coarse: the upgrades wait.
        encoder.set_pace(20, true);
        paint(&mut pixels, w, busy, 14);
        let changed = now + ms(100);
        step(&mut encoder, &mut page, &pixels, Some(&[busy]), changed);
        let sent = page.progressives;
        let mut out = Vec::new();
        for at in [1000, 1750] {
            assert_eq!(
                encoder.encode(changed + ms(at), &mut out).unwrap().rects,
                0,
                "nothing is paid while coarse and lately changed"
            );
        }
        assert!(encoder.owed(), "the debt stands");
        // Quiet long enough, it is paid though the walk is still coarse: a
        // still desktop tells the walk nothing that would lift it.
        let paid_at = settle(&mut encoder, &mut page, changed + ms(1750));
        assert!(paid_at >= changed + COARSE_QUIET, "paid at {paid_at:?}");
        assert!(
            page.progressives > sent,
            "upgraded while coarse, once quiet"
        );
        assert!(!encoder.owed());
    }

    /// On a progressive list every rectangle is a progressive tile, a COPY,
    /// a CACHED or a STORE: no WebP or stream, even where the desktop
    /// moves. Each settles to the full stage, close to the desktop, and owes
    /// nothing then.
    #[test]
    fn a_progressive_list_is_sent_progressive_tiles_alone() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = soft(w, h, 0);
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let first = update(&mut encoder, &mut page, &pixels, None, ms(0));
        assert!(first.keyframe, "the first picture is one");
        assert_eq!(
            (page.progressives, page.webps),
            (8, 0),
            "every tile a progressive tile"
        );
        settle(&mut encoder, &mut page, ms(0));
        assert!(!encoder.owed(), "settled at full");
        assert!(
            page.psnr(&pixels) > 40.0,
            "settled close: {:.1} dB",
            page.psnr(&pixels)
        );

        // Moving: no stream, every frame progressive tiles.
        encoder.set_pace(40, false);
        let busy = Rect {
            x: 64,
            y: 0,
            width: 128,
            height: 64,
        };
        for n in 0..12 {
            paint(&mut pixels, w, busy, n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[busy]),
                ms(100 + 50 * u64::from(n)),
            );
        }
        assert_eq!(
            (page.starts, page.webps),
            (0, 0),
            "no stream, no still but progressive"
        );
        assert!(encoder.owed(), "below the ceiling, owed the stages after");
        encoder.set_pace(80, false);
        pixels = soft(w, h, 3);
        step(&mut encoder, &mut page, &pixels, None, ms(1000));
        settle(&mut encoder, &mut page, ms(1000));
        assert!(!encoder.owed());
        assert!(
            page.psnr(&pixels) > 40.0,
            "settled close again: {:.1} dB",
            page.psnr(&pixels)
        );
    }

    /// A lossless list is sent exact tiles alone, still or moving: what is
    /// still planar in the still stream, what moves a MEDIAN each, every
    /// update exact, no H.264 stream however long a region moves, and
    /// nothing owed.
    #[test]
    fn a_lossless_list_is_sent_exact_tiles_alone() {
        let (w, h) = (256u16, 150u16);
        let mut pixels = soft(w, h, 0);
        let mut encoder = TilesEncoder::new(w, h, lossless(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let first = update(&mut encoder, &mut page, &pixels, None, ms(0));
        assert!(first.keyframe, "the first picture is one");
        assert_eq!(
            (
                page.still_count,
                page.medians,
                page.webps,
                page.progressives
            ),
            (3, 0, 0, 0),
            "the whole picture in three bands of still runs"
        );
        assert_eq!(page.still_starts, 1, "one stream");
        assert_eq!(page.off(&pixels), 0, "exact");
        assert!(
            !encoder.owed() && encoder.tick_at().is_none(),
            "a lossless picture owes nothing"
        );

        let busy = Rect {
            x: 64,
            y: 0,
            width: 128,
            height: 64,
        };
        for n in 0..20 {
            paint(&mut pixels, w, busy, n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[busy]),
                ms(100 + 50 * u64::from(n)),
            );
            assert_eq!(page.off(&pixels), 0, "every frame exact");
        }
        assert_eq!(
            (page.starts, page.webps, page.progressives),
            (0, 0, 0),
            "no H.264 stream: what moves is lossless too"
        );
        assert!(
            page.medians > 0 && page.still_count > 3,
            "what moves a MEDIAN, once it has moved a while, {} of them, and still before",
            page.medians
        );
        assert_eq!(page.still_starts, 1, "one still stream throughout");
        assert!(!encoder.owed(), "nothing owed while it moves either");
        let (medians, stills, cached) = (page.medians, page.still_count, page.cached);
        // Painted with the colours it held before: drawn from the cache.
        paint(&mut pixels, w, busy, 0);
        step(&mut encoder, &mut page, &pixels, Some(&[busy]), ms(2000));
        assert_eq!(page.off(&pixels), 0);
        assert!(
            page.cached > cached && page.medians == medians && page.still_count == stills,
            "a tile shown before is a slot drawn"
        );

        // The list names the held picture: the picture whole again, from the
        // cache where it keeps a tile, its tiles stored lossless.
        let (stored, bytes) = (page.cached, page.bytes);
        encoder.set_picture(held());
        step(&mut encoder, &mut page, &pixels, None, ms(3000));
        assert_eq!(page.off(&pixels), 0);
        assert!(
            page.cached > stored && page.bytes - bytes < 4096,
            "drawn from the cache: {} tiles in {} bytes",
            page.cached - stored,
            page.bytes - bytes
        );
    }

    /// The still stream starts over wherever the picture does, and wherever
    /// the client's cache is gone: a client that missed a rectangle holds
    /// none of it.
    #[test]
    fn a_lossless_lists_still_stream_starts_over_with_the_picture_and_the_cache() {
        let (w, h) = (192u16, 64u16);
        let mut encoder = TilesEncoder::new(w, h, lossless(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &soft(w, h, 0), None, ms(0));
        step(&mut encoder, &mut page, &soft(w, h, 1), None, ms(1000));
        assert_eq!(
            (page.still_count, page.still_starts),
            (2, 1),
            "the stream continued"
        );
        encoder.forget();
        step(&mut encoder, &mut page, &soft(w, h, 2), None, ms(2000));
        assert_eq!(
            (page.still_count, page.still_starts),
            (3, 2),
            "the picture started over, and the stream with it"
        );
        encoder.forget_cache();
        step(&mut encoder, &mut page, &soft(w, h, 3), None, ms(3000));
        assert_eq!(
            (page.still_count, page.still_starts),
            (4, 3),
            "the cache gone, and the stream with it"
        );
        assert_eq!(page.off(&soft(w, h, 3)), 0);
    }

    /// A walked list's tiles are not lossless: a lossless list draws none
    /// that it stored, and one stored lossless again is drawn.
    #[test]
    fn a_lossless_list_draws_no_tile_a_walked_one_stored() {
        let (w, h) = (128u16, 64u16);
        let mut pixels = soft(w, h, 0);
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        settle(&mut encoder, &mut page, ms(0));
        encoder.set_picture(lossless());
        step(&mut encoder, &mut page, &pixels, None, ms(5000));
        assert_eq!(
            (page.cached, page.still_count),
            (0, 1),
            "nothing a walked list stored is drawn on a lossless one"
        );
        assert_eq!(page.off(&pixels), 0, "the picture exact");
        same_tile(&mut pixels, w, (0, 0), (64, 0));
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 64,
                y: 0,
                width: 64,
                height: 64,
            }]),
            ms(5100),
        );
        assert_eq!(page.cached, 1, "a slot stored lossless is drawn");
        assert_eq!(page.off(&pixels), 0);
    }

    /// A lossless page scrolled is a copy and the strip it uncovered.
    #[test]
    fn a_lossless_scroll_is_a_copy() {
        let (w, h) = (256u16, 256u16);
        let document = noise(w, 3 * h, 21);
        let mut encoder = TilesEncoder::new(w, h, lossless(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(
            &mut encoder,
            &mut page,
            &scrolled(&document, w, 0, h),
            None,
            ms(0),
        );
        let pixels = scrolled(&document, w, 40, h);
        update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }]),
            ms(100),
        );
        assert_eq!(page.copies, 1);
        assert_eq!(page.off(&pixels), 0, "the page exact");
        assert!(!encoder.owed(), "a copy of lossless pixels owes nothing");
    }

    /// A settled page scrolled is copies of settled tiles, which are settled
    /// where they land: only the strip the scroll uncovered is owed its
    /// upgrades, and the settle ends at the picture a client sent it fresh
    /// would hold.
    #[test]
    fn a_progressive_copy_of_a_settled_page_is_not_sent_again() {
        let (w, h) = (256u16, 256u16);
        let document = noise(w, 3 * h, 21);
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(
            &mut encoder,
            &mut page,
            &scrolled(&document, w, 0, h),
            None,
            ms(0),
        );
        let now = settle(&mut encoder, &mut page, ms(0));
        assert!(!encoder.owed());
        let pixels = scrolled(&document, w, 24, h);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }]),
            now + ms(100),
        );
        assert_eq!(page.copies, 1, "scrolled as a copy");
        // The tiles the copy draws whole are settled; the strip it uncovered,
        // the last row of tiles, is owed its upgrades.
        assert!(
            encoder.debts.keys().all(|&(_, row)| row == 3),
            "owed {:?}",
            encoder.debts.keys().collect::<Vec<_>>()
        );
        settle(&mut encoder, &mut page, now + ms(200));
        let mut fresh = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut sent = Page::new(w, h);
        step(&mut fresh, &mut sent, &pixels, None, ms(0));
        settle(&mut fresh, &mut sent, ms(0));
        let (scrolled_db, fresh_db) = (page.psnr(&pixels), sent.psnr(&pixels));
        assert!(
            scrolled_db > fresh_db - 0.5,
            "the scrolled page settled at {scrolled_db:.1} dB, a fresh one at {fresh_db:.1}"
        );
    }

    /// A page scrolled below the ceiling is copies of tiles owed their
    /// upgrades: each tile a copy draws over is sent again from what the
    /// desktop shows there, as a first pass, and the settle ends at the
    /// picture a client sent it fresh would hold, not at the tile the copy
    /// drew over.
    #[test]
    fn a_progressive_copy_is_sent_again_from_the_desktop() {
        let (w, h) = (256u16, 256u16);
        let document = noise(w, 3 * h, 21);
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        encoder.set_pace(30, false);
        step(
            &mut encoder,
            &mut page,
            &scrolled(&document, w, 0, h),
            None,
            ms(0),
        );
        let mut pixels = Vec::new();
        for (n, top) in [24, 48, 72, 96].into_iter().enumerate() {
            pixels = scrolled(&document, w, top, h);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[Rect {
                    x: 0,
                    y: 0,
                    width: w,
                    height: h,
                }]),
                ms(100 * (n as u64 + 1)),
            );
        }
        assert_eq!(page.copies, 4, "scrolled as copies");
        encoder.set_pace(80, false);
        settle(&mut encoder, &mut page, ms(500));
        assert!(!encoder.owed());
        let mut fresh = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut sent = Page::new(w, h);
        step(&mut fresh, &mut sent, &pixels, None, ms(0));
        settle(&mut fresh, &mut sent, ms(0));
        let (scrolled_db, fresh_db) = (page.psnr(&pixels), sent.psnr(&pixels));
        assert!(
            scrolled_db > fresh_db - 0.5,
            "the scrolled page settled at {scrolled_db:.1} dB, a fresh one at {fresh_db:.1}"
        );
    }

    /// A tile shown again is a draw of its cache slot, which leaves the
    /// client holding no coefficients the encoder counts on: the next change
    /// there is a first pass whole, and the page ends right.
    #[test]
    fn a_progressive_tile_drawn_from_the_cache_starts_over() {
        let (w, h) = (256u16, 128u16);
        let (a, b) = (soft(w, h, 0), soft(w, h, 90));
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &a, None, ms(0));
        settle(&mut encoder, &mut page, ms(0));
        step(&mut encoder, &mut page, &b, None, ms(1000));
        settle(&mut encoder, &mut page, ms(1000));
        let cached = page.cached;
        step(&mut encoder, &mut page, &a, None, ms(2000));
        assert!(
            page.cached > cached,
            "the first picture again, from the cache"
        );
        settle(&mut encoder, &mut page, ms(2000));
        assert!(page.psnr(&a) > 40.0, "{:.1} dB", page.psnr(&a));
        let mut c = a.clone();
        paint(
            &mut c,
            w,
            Rect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
            4,
        );
        step(
            &mut encoder,
            &mut page,
            &c,
            Some(&[Rect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            }]),
            ms(3000),
        );
        settle(&mut encoder, &mut page, ms(3000));
        assert!(!encoder.owed());
        assert!(page.psnr(&c) > 30.0, "{:.1} dB", page.psnr(&c));
    }

    /// A tile that settles by its upgrades is stored settled: shown again,
    /// it is a draw of its slot that owes nothing.
    #[test]
    fn a_progressive_tile_settled_by_upgrades_is_kept_settled() {
        let (w, h) = (256u16, 128u16);
        let (a, b) = (noise(w, h, 3), noise(w, h, 4));
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &a, None, ms(0));
        assert!(encoder.owed(), "first passes below the full stage");
        settle(&mut encoder, &mut page, ms(0));
        step(&mut encoder, &mut page, &b, None, ms(1000));
        settle(&mut encoder, &mut page, ms(1000));
        let (cached, progressives) = (page.cached, page.progressives);
        step(&mut encoder, &mut page, &a, None, ms(2000));
        assert_eq!(
            (page.cached - cached, page.progressives - progressives),
            (8, 0),
            "every tile from the cache"
        );
        assert!(!encoder.owed(), "kept settled");
    }

    /// Below the ceiling, on a quiet desktop, a tick sends the oldest owed
    /// until its bytes are spent, and the next is owed at once:
    /// [`SETTLE_BYTES_LEAST`] at first, twice that after an update that
    /// crossed quickly, half after one that queued, and the least again once
    /// the walk steps down.
    #[test]
    fn a_tick_sends_the_oldest_until_its_bytes_are_spent() {
        let (w, h) = (1024u16, 1024u16);
        let mut pixels = noise(w, h, 16);
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        encoder.set_pace(20, false);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let row = Rect {
            x: 0,
            y: 0,
            width: w,
            height: 64,
        };
        paint(&mut pixels, w, row, 17);
        step(&mut encoder, &mut page, &pixels, Some(&[row]), ms(300));
        assert_eq!(
            encoder.debts.len(),
            256,
            "every tile owed, the top row from later"
        );
        // The top row's tiles owed from the read that changed them, which no
        // tick has upgraded since.
        let top = |encoder: &TilesEncoder| {
            (0..16).all(|col| encoder.debts.get(&(col, 0)) == Some(&ms(300)))
        };

        // Each tick's bytes, and the tiles it sent, its fence back in `took`.
        let tick = |encoder: &mut TilesEncoder, page: &mut Page, took: Duration| {
            let mut out = Vec::new();
            // Quiet from the top row's change on.
            let at = encoder.tick_at().unwrap().max(ms(300) + CLEANUP_IDLE);
            let sent = page.progressives;
            let tick = encoder.encode(at, &mut out).unwrap();
            encoder.delivered(took);
            page.draw(tick.rects, &out);
            assert_eq!(encoder.tick_at(), Some(at), "more owed at once");
            (at, out.len(), page.progressives - sent)
        };
        let (at, first, tiles) = tick(&mut encoder, &mut page, Duration::ZERO);
        assert!(at >= CLEANUP_IDLE, "the first owed once still: {at:?}");
        let tile = first / tiles;
        assert!(
            first >= SETTLE_BYTES_LEAST && first < SETTLE_BYTES_LEAST + 2 * tile,
            "{first} bytes, {tiles} tiles"
        );
        assert!(top(&encoder), "the oldest first");
        let (_, quick, _) = tick(&mut encoder, &mut page, SETTLE_LATE);
        assert!(
            quick >= 2 * SETTLE_BYTES_LEAST && quick < 2 * SETTLE_BYTES_LEAST + 2 * tile,
            "twice after a quick one: {quick}"
        );
        let (_, late, _) = tick(&mut encoder, &mut page, Duration::ZERO);
        assert!(
            late >= SETTLE_BYTES_LEAST && late < SETTLE_BYTES_LEAST + 2 * tile,
            "half after a late one: {late}"
        );
        assert!(top(&encoder), "the oldest first");
        // Twice that after a quick one, until the walk steps down: then the
        // least again.
        encoder.set_pace(10, false);
        let (_, stepped, _) = tick(&mut encoder, &mut page, Duration::ZERO);
        assert!(
            stepped >= SETTLE_BYTES_LEAST && stepped < SETTLE_BYTES_LEAST + 2 * tile,
            "the least after a step down: {stepped}"
        );
        // At the ceiling, the most.
        encoder.set_pace(90, false);
        let (at, most, _) = tick(&mut encoder, &mut page, SETTLE_LATE);
        assert!(
            most >= SETTLE_BYTES && most < SETTLE_BYTES + 2 * tile,
            "at the ceiling: {most}"
        );
        settle(&mut encoder, &mut page, at);
        assert!(!encoder.owed());
    }

    /// A change of more than an update's stills where nothing moved — the
    /// first picture of a desktop of noise — goes as video: the stills up to
    /// the budget and one run over, then a region over the box around the
    /// rest, its START in the same update, every tile of the box owed; the
    /// stream ends once nothing in it has moved for a second, and the settle
    /// pays them at the pace, eased in to it, a tick at a time never on the
    /// heels of the last, none past its bytes and a tile, until the picture
    /// is lossless and every whole tile stored.
    #[test]
    fn a_change_past_the_stills_goes_as_video_and_settles_at_the_pace() {
        let (w, h) = (640u16, 384u16);
        let pixels = noise(w, h, 71);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let mut out = Vec::new();
        encoder
            .read(&pixels, usize::from(w) * 4, None, ms(0))
            .unwrap();
        let first = encoder.encode(ms(0), &mut out).unwrap();
        // The stills, then the region's START over the box around the rows
        // they did not reach.
        let mut wire = crate::rfb::msg::update_header(first.rects).to_vec();
        wire.extend_from_slice(&out);
        let Some((ServerMsg::Update(rects), _)) = client::parse(&wire).unwrap() else {
            panic!("an update")
        };
        let start = rects.last().expect("rectangles");
        assert!(
            matches!(
                start.body,
                RectBody::Tiles(TileBody::Start { stream: 0, .. })
            ),
            "the region's START last"
        );
        assert!(
            rects[..rects.len() - 1].iter().all(|rect| matches!(
                rect.body,
                RectBody::Tiles(TileBody::Webp(_) | TileBody::Store { .. })
            )),
            "the stills ahead of it"
        );
        assert_eq!(
            (start.x, start.y, start.width, start.height),
            (0, 0, w, h),
            "the box around the whole change, the stills' rows too"
        );
        page.draw(first.rects, &out);
        assert!(first.keyframe, "a picture sent whole is a keyframe");
        let stills: usize = page.bytes;
        assert!(
            page.webps >= 2 && page.webps <= 3,
            "the stills up to the budget, a row's run each: {} WebPs",
            page.webps
        );
        assert!(page.off(&pixels) > 0, "the rest a frame of video, lossy");
        assert_eq!(page.streams.len(), 1, "the region live");
        assert!(
            encoder.pace >= stills,
            "the pace is what the update brought, though no tick had a tile to pay: {}",
            encoder.pace
        );
        assert!(encoder.owed(), "every tile of the box owed its still");
        let whole = usize::from(w / TILE) * usize::from(h / TILE);
        assert!(
            page.stored < whole,
            "{} of {whole} stored so far",
            page.stored
        );

        // The settle: ticks apart, none past its bytes and a tile, the first
        // of them small and the rest rising to the pace.
        let mut last: Option<Duration> = None;
        let mut largest = 0;
        let mut paid: Vec<usize> = Vec::new();
        while let Some(at) = encoder.tick_at() {
            if let Some(last) = last {
                assert!(
                    at >= last + SETTLE_TICK_LEAST,
                    "a tick on the heels of the last: {last:?} then {at:?}"
                );
            }
            out.clear();
            let tick = encoder.encode(at, &mut out).unwrap();
            let before = page.bytes;
            page.draw(tick.rects, &out);
            largest = largest.max(page.bytes - before);
            if page.bytes - before > 1024 {
                paid.push(page.bytes - before);
            }
            last = Some(at);
            assert!(at < ms(60_000), "never settled");
        }
        // A tile of noise is its pixels and a little.
        let tile = usize::from(TILE) * usize::from(TILE) * 3 + 1024;
        assert!(
            largest <= SETTLE_MOST + tile,
            "a tick of {largest} bytes, past {SETTLE_MOST} and a tile of {tile}"
        );
        assert!(stills >= HELD_BYTES, "the first update's stills: {stills}");
        assert!(
            paid.len() >= 6 && paid[0] <= SETTLE_MOST / 4 && paid[0] * 3 <= largest,
            "the settle eases in: {paid:?}"
        );
        assert!(
            paid[..paid.len() - 1]
                .windows(2)
                .all(|pair| pair[1] + tile >= pair[0]),
            "and does not fall back before its last tick: {paid:?}"
        );
        assert_eq!(page.off(&pixels), 0, "the picture settled lossless");
        assert_eq!(page.stored, whole, "every whole tile stored");
        assert_eq!(
            (page.starts, page.ends, page.frames),
            (1, 1, 0),
            "the region, ended idle, carried no frame after its start"
        );
        assert!(
            last.unwrap() >= STREAM_IDLE,
            "settled after the region's idle second: {last:?}"
        );
    }

    /// A row wider than a WebP's longest side goes as two, cut on the tile
    /// grid, every whole tile stored where it is.
    #[test]
    fn a_row_wider_than_a_webp_is_cut_on_the_tile_grid() {
        let (w, h) = (webp::MAX_SIDE + 100, 64u16);
        // Bars of a few colours, a tile and a half wide: little to code.
        let pixels: Vec<u8> = (0..usize::from(h))
            .flat_map(|_| (0..usize::from(w)).flat_map(|x| [(x / 96 % 5) as u8 * 50, 40, 200, 0]))
            .collect();
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let first = update(&mut encoder, &mut page, &pixels, None, ms(0));
        assert!(first.keyframe);
        assert_eq!(
            (page.webps, page.starts),
            (2, 0),
            "the row in two WebPs, and no region"
        );
        assert_eq!(page.off(&pixels), 0, "the row lossless");
        assert!(encoder.tick_at().is_none(), "and nothing owed");
    }

    /// The frames of a video before its region forms by churn: the first
    /// frame past an update's stills is its region, and the frames after
    /// it go as the stream's own, no still of them, where they went as the
    /// desktop whole in stills until the churn said moving.
    #[test]
    fn a_video_that_starts_is_a_region_from_its_first_frame() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = vec![0; usize::from(w) * usize::from(h) * 4];
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        assert_eq!(page.starts, 0, "a black desktop within an update's stills");
        let whole = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        for n in 1..=8u32 {
            paint(&mut pixels, w, whole, 300 + n);
            let (webps, frames) = (page.webps, page.frames);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[whole]),
                t0 + ms(u64::from(n) * 50),
            );
            if n == 1 {
                assert_eq!(
                    page.starts, 1,
                    "the first frame past the stills is a region"
                );
                assert!(
                    page.webps > webps,
                    "with the stills up to the budget ahead of it"
                );
            } else {
                assert_eq!(
                    (page.starts, page.frames - frames, page.webps - webps),
                    (1, 1, 0),
                    "frame {n}: the stream's own, no still"
                );
            }
        }
        settle(&mut encoder, &mut page, t0 + ms(400));
        assert_eq!(page.off(&pixels), 0, "settled lossless");
    }

    /// A change past the stills around a region already live takes the
    /// region over, as a retune does: the live one ended, the new one over
    /// the box, no two live regions over one tile.
    #[test]
    fn a_change_past_the_stills_around_a_live_region_takes_it_over() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = vec![0; usize::from(w) * usize::from(h) * 4];
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let video = Rect {
            x: 192,
            y: 64,
            width: 256,
            height: 192,
        };
        for n in 1..=10u32 {
            paint(&mut pixels, w, video, 500 + n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[video]),
                t0 + ms(u64::from(n) * 50),
            );
        }
        assert_eq!((page.starts, page.ends), (1, 0), "the video's region live");
        let before = encoder.live[0].area;
        let whole = noise(w, h, 73);
        step(
            &mut encoder,
            &mut page,
            &whole,
            Some(&[Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }]),
            t0 + ms(550),
        );
        assert_eq!(
            (page.starts, page.ends),
            (2, 1),
            "the video's region ended, the box around the change started"
        );
        assert_eq!(encoder.live.len(), 1, "one region live");
        assert_eq!(
            encoder.live[0].area,
            Area::whole(encoder.size),
            "over the whole change"
        );
        assert!(
            encoder.live[0].area.intersect(before).is_some(),
            "the video within it"
        );
        settle(&mut encoder, &mut page, t0 + ms(550));
        assert_eq!(page.off(&whole), 0, "settled lossless");
    }

    /// A change past the stills that ends partway through a row of tiles is
    /// a region over whole tiles: its cells are covered whole, so a change
    /// in the rest of an edge tile must be the region's frame, or it would
    /// be neither still nor frame while the stream lives.
    #[test]
    fn a_region_past_the_stills_is_whole_tiles() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = vec![0; usize::from(w) * usize::from(h) * 4];
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        // All but the bottom half of the last row of tiles.
        let most = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h - TILE / 2,
        };
        paint(&mut pixels, w, most, 600);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[most]),
            t0 + ms(100),
        );
        assert_eq!(page.starts, 1, "past the stills: a region");
        assert_eq!(
            encoder.live[0].area,
            Area::whole(encoder.size),
            "over whole tiles, the last row's bottom half too"
        );
        // A change in that bottom half: the region's frame.
        let strip = Rect {
            x: 0,
            y: h - TILE / 2,
            width: w,
            height: TILE / 2,
        };
        paint(&mut pixels, w, strip, 601);
        let (frames, webps) = (page.frames, page.webps);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[strip]),
            t0 + ms(150),
        );
        assert_eq!(
            (page.frames - frames, page.webps - webps),
            (1, 0),
            "the strip a frame of the stream"
        );
        settle(&mut encoder, &mut page, t0 + ms(150));
        assert_eq!(page.off(&pixels), 0, "settled lossless");
    }

    /// A held list's tick keeps to the pace: the most the reads were found
    /// sending over a second since something was last owed, a tick's share
    /// of its interval less what the reads sent in it, never under the
    /// floor's share nor over the most a tick sends, the ticks sooner where
    /// the pace has that much going in less than a quarter of a second; and
    /// eases in to it, its rate rising over the second after the settle
    /// began paying.
    #[test]
    fn a_held_tick_eases_in_to_the_pace() {
        let mut encoder = TilesEncoder::new(64, 64, held(), Fake::coder()).unwrap();
        encoder.debts.insert((0, 0), ms(0));
        // A settle that began a second and more ago: eased in.
        encoder.easing_from = Some(ms(0));
        assert_eq!(encoder.tick_interval(), CLEANUP_TICK, "no pace yet");
        assert_eq!(
            encoder.paced_budget(ms(1000)),
            SETTLE_FLOOR,
            "nothing carried: the floor"
        );

        // 10 KiB in the last second: a share under the floor.
        encoder.sent.push_back((ms(900), 10 * 1024));
        assert_eq!(encoder.paced_budget(ms(1000)), SETTLE_FLOOR);
        assert_eq!(encoder.pace, 10 * 1024);

        // A stream at 2 MB a second: the ticks every 66 ms, the full budget
        // once the reads' own bytes are out of the interval, and nothing
        // beyond the floor's share of the interval while they are in it.
        encoder.sent.clear();
        encoder.sent.push_back((ms(950), 2_000_000));
        let interval = Duration::from_secs_f64(SETTLE_MOST as f64 / 2e6);
        let floor =
            (SETTLE_FLOOR as f64 * interval.as_secs_f64() / CLEANUP_TICK.as_secs_f64()) as usize;
        assert_eq!(
            encoder.paced_budget(ms(1000)),
            floor,
            "the reads have the pace"
        );
        assert!(
            floor < SETTLE_FLOOR / 3,
            "a sooner tick's floor is its share"
        );
        assert_eq!(encoder.pace, 2_000_000);
        assert_eq!(encoder.tick_interval(), interval);
        encoder.sent.clear();
        assert_eq!(
            encoder.paced_budget(ms(1200)),
            SETTLE_MOST,
            "the full budget once the reads are out of the interval, the pace kept"
        );

        // A settle that begins now starts from the floor, is half way there
        // half a second on, and at the pace from a second on.
        encoder.easing_from = None;
        let first = encoder.paced_budget(ms(1200));
        assert_eq!(
            encoder.easing_from,
            Some(ms(1200)),
            "eased from its first tick"
        );
        assert!(
            first >= floor && first < SETTLE_MOST / 8,
            "the first tick of a settle: {first}"
        );
        let mut last = first;
        for at in (1250..=2200).step_by(50) {
            let budget = encoder.paced_budget(ms(at));
            assert!(budget >= last, "never a step down: {budget} at {at}");
            assert!(
                budget - last <= SETTLE_MOST / 10,
                "nor a step up of more than a tick's twentieth of the pace: {last} to {budget} at {at}"
            );
            last = budget;
        }
        let half = encoder.paced_budget(ms(1700));
        assert!(
            half > SETTLE_MOST * 2 / 5 && half < SETTLE_MOST * 2 / 3,
            "half a second on: {half}"
        );
        assert_eq!(encoder.paced_budget(ms(2200)), SETTLE_MOST, "a second on");

        // The pace is the most found: a slower second does not lower it; a
        // faster one raises it, and the interval shortens to its least.
        encoder.sent.push_back((ms(2300), 100_000));
        encoder.paced_budget(ms(2400));
        assert_eq!(encoder.pace, 2_000_000);
        encoder.sent.push_back((ms(2300), 20_000_000));
        encoder.paced_budget(ms(2400));
        assert_eq!(encoder.pace, 20_100_000);
        assert_eq!(encoder.tick_interval(), SETTLE_TICK_LEAST);

        // Paid off, the pace is forgotten with the next encode, and the
        // next settle eases in again.
        encoder.debts.clear();
        encoder.encode(ms(2500), &mut Vec::new()).unwrap();
        assert_eq!(encoder.pace, 0, "nothing owed: no pace");
        assert_eq!(encoder.easing_from, None, "nothing to pay: no ease");
    }

    /// A scroll of a page of noise uncovers more than an update's stills, and
    /// they all go as stills: what a scroll uncovers the list promises
    /// lossless, and a stream a strip would be a coder opened an update.
    #[test]
    fn a_scroll_past_the_stills_stays_stills() {
        let (w, h) = (1024u16, 512u16);
        let document = noise(w, 3 * h, 72);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &scrolled(&document, w, 0, h));
        let starts = page.starts;
        let pixels = scrolled(&document, w, 128, h);
        let before = page.bytes;
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }]),
            t0 + ms(100),
        );
        assert_eq!(page.copies, 1, "the scroll a copy");
        assert!(
            page.bytes - before > HELD_BYTES,
            "two rows of noise uncovered: {} bytes",
            page.bytes - before
        );
        assert_eq!(page.starts, starts, "no stream for them");
        assert_eq!(page.off(&pixels), 0, "lossless at once");
        assert!(!encoder.owed());
    }

    /// The damage is compared with what the client holds: a redraw of the same
    /// pixels sends nothing, and one tile that changed is one WebP of it.
    #[test]
    fn only_tiles_that_changed_are_sent() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 2);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let whole = [Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }];
        assert_eq!(
            step(&mut encoder, &mut page, &pixels, Some(&whole), t0 + ms(100)),
            0,
            "a redraw of what the client holds"
        );
        paint(
            &mut pixels,
            w,
            Rect {
                x: 70,
                y: 70,
                width: 10,
                height: 10,
            },
            9,
        );
        let bytes = page.bytes;
        assert_eq!(
            step(&mut encoder, &mut page, &pixels, Some(&whole), t0 + ms(200)),
            2,
            "one tile changed: its WebP and its store"
        );
        assert!(
            page.bytes - bytes < 64 * 64 * 3 + 200,
            "more than one tile was sent"
        );
        assert_eq!(page.off(&pixels), 0);
        // The framebuffer's rows may be farther apart than the picture's.
        let stride = usize::from(w) * 4 + 64;
        let mut wide = vec![0xEE; stride * usize::from(h)];
        for row in 0..usize::from(h) {
            wide[row * stride..][..usize::from(w) * 4]
                .copy_from_slice(&pixels[row * usize::from(w) * 4..][..usize::from(w) * 4]);
        }
        encoder
            .read(&wide, stride, Some(&whole), t0 + ms(300))
            .unwrap();
        assert_eq!(
            encoder.encode(t0 + ms(300), &mut Vec::new()).unwrap().rects,
            0,
            "the same picture at another stride"
        );
        assert!(matches!(
            encoder.read(&wide[..100], stride, None, t0 + ms(400)),
            Err(TilesError::Buffer { .. })
        ));
    }

    /// What moves becomes a stream of its own, told only its damage, which
    /// ends once the motion stops, and every tile it carried is sent lossless
    /// after its end: the picture ends lossless, and owes nothing.
    #[test]
    fn what_moves_streams_and_settles_lossless() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = noise(w, h, 3);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let webps = page.webps;
        // A 192×128 video, every 50 ms for two seconds.
        let video = Rect {
            x: 128,
            y: 64,
            width: 192,
            height: 128,
        };
        for n in 1..=40u32 {
            paint(&mut pixels, w, video, 100 + n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[video]),
                t0 + ms(u64::from(n) * 50),
            );
        }
        assert_eq!(
            (page.starts, page.ends),
            (2, 1),
            "one stream live, after the first picture's"
        );
        assert!(page.frames > 20, "{} frames of it", page.frames);
        assert!(
            page.webps - webps <= 16,
            "{} WebPs while it moved",
            page.webps - webps
        );
        assert!(
            encoder.owed() && encoder.tick_at().is_some(),
            "a live stream owes its end"
        );
        assert!(page.off(&pixels) > 0, "a stream's picture is not lossless");
        settle(&mut encoder, &mut page, t0 + ms(2000));
        assert_eq!(page.ends, 2, "the stream ended");
        assert_eq!(page.off(&pixels), 0, "the picture settled lossless");
        assert!(page.streams.is_empty());
    }

    /// With the outline asked for, a region's frames carry it, inside its
    /// rectangle and two of the layout's pixels thick, the read's damage
    /// short of it or not; the encoder's own picture never does, so the
    /// stills after are the desktop's and the picture settles lossless, the
    /// outline gone.
    #[test]
    fn a_region_outlined_settles_without_its_outline() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = noise(w, h, 3);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        encoder.set_debug(true);
        encoder.set_scale(2.0);
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        // A video well inside the tiles its region is laid out over, so no
        // frame's damage reaches the region's edge: 20 tiles, a stream at 2.
        let video = Rect {
            x: 70,
            y: 70,
            width: 308,
            height: 244,
        };
        for n in 1..=40u32 {
            paint(&mut pixels, w, video, 100 + n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[video]),
                t0 + ms(u64::from(n) * 50),
            );
        }
        assert_eq!(page.streams.len(), 1, "one stream live");
        // The region is tiles (1,1) to (5,4): 64,64 to 383,319.
        let at = |x: usize, y: usize| {
            let i = (y * usize::from(w) + x) * 4;
            [page.canvas[i], page.canvas[i + 1], page.canvas[i + 2]]
        };
        // Magenta as the stand-in coder leaves it: what moved was a
        // region for it by the end, whichever way it began.
        let magenta = [254, 0, 254];
        for (x, y) in [
            (64, 64),
            (67, 200),
            (383, 319),
            (200, 316),
            (380, 100),
            (300, 67),
        ] {
            assert_eq!(at(x, y), magenta, "the outline at {x},{y}");
        }
        for (x, y) in [(68, 68), (379, 315), (63, 64), (384, 200), (200, 320)] {
            assert_ne!(at(x, y), magenta, "no outline at {x},{y}");
        }
        settle(&mut encoder, &mut page, t0 + ms(2000));
        assert_eq!(
            page.off(&pixels),
            0,
            "the picture settled lossless, the outline gone"
        );
    }

    /// The fewest moving tiles a stream needs is an area of the layout: at
    /// scale 2 a spinner over 3×3 tiles, 96 of the layout's pixels across,
    /// stays stills, lossless throughout, where at 1 it streams; a video at 2
    /// four times a widget's tiles streams as at 1.
    #[test]
    fn a_stream_needs_an_area_of_the_layout() {
        assert_eq!(
            [0.5, 1.0, 1.25, 1.5, 2.0, 3.0].map(stream_cells),
            [5, 5, 8, 12, 20, 45]
        );
        let (w, h) = (640u16, 384u16);
        let spin = |scale: f64, moving: Rect| {
            let mut pixels = noise(w, h, 4);
            let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
            encoder.set_scale(scale);
            let mut page = Page::new(w, h);
            let t0 = begin(&mut encoder, &mut page, &pixels);
            let mut lossless = true;
            for n in 1..=40u32 {
                paint(&mut pixels, w, moving, 500 + n);
                step(
                    &mut encoder,
                    &mut page,
                    &pixels,
                    Some(&[moving]),
                    t0 + ms(u64::from(n) * 50),
                );
                lossless &= page.off(&pixels) == 0;
            }
            // Less the first picture's one frame of video.
            (page.starts - 1, lossless)
        };
        let spinner = Rect {
            x: 96,
            y: 96,
            width: 160,
            height: 160,
        };
        assert_eq!(spin(1.0, spinner).0, 1, "9 tiles at 1 stream");
        assert_eq!(
            spin(2.0, spinner),
            (0, true),
            "9 tiles at 2 are stills, lossless throughout"
        );
        let video = Rect {
            x: 128,
            y: 64,
            width: 384,
            height: 256,
        };
        assert_eq!(spin(2.0, video).0, 1, "24 tiles at 2 stream");
    }

    /// Paint `rect` of a `w`-wide picture with frame `n` of a striped
    /// progress bar: two flat colours, the stripes a few pixels along each
    /// frame.
    fn stripes(pixels: &mut [u8], w: u16, rect: Rect, n: u32) {
        for row in 0..usize::from(rect.height) {
            for col in 0..usize::from(rect.width) {
                let at =
                    ((usize::from(rect.y) + row) * usize::from(w) + usize::from(rect.x) + col) * 4;
                let v = if ((col + row + n as usize * 3) / 12).is_multiple_of(2) {
                    [250, 165, 96, 0]
                } else {
                    [254, 219, 191, 0]
                };
                pixels[at..at + 4].copy_from_slice(&v);
            }
        }
    }

    /// A stream needs its stills to have been dear: a striped bar across
    /// eight tiles, all of it moving, more tiles than a stream needs, costs
    /// little as stills and stays stills, lossless throughout, where the
    /// same tiles of a video stream.
    #[test]
    fn what_moves_cheaply_is_no_stream() {
        let (w, h) = (640u16, 384u16);
        let bar = Rect {
            x: 64,
            y: 64,
            width: 512,
            height: 64,
        };
        let play = |frame: fn(&mut [u8], u16, Rect, u32)| {
            let mut pixels = vec![255; usize::from(w) * usize::from(h) * 4];
            let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
            let mut page = Page::new(w, h);
            let t0 = begin(&mut encoder, &mut page, &pixels);
            assert_eq!(page.starts, 0, "a flat first picture is stills");
            let mut lossless = true;
            for n in 1..=40u32 {
                frame(&mut pixels, w, bar, n);
                step(
                    &mut encoder,
                    &mut page,
                    &pixels,
                    Some(&[bar]),
                    t0 + ms(u64::from(n) * 50),
                );
                lossless &= page.off(&pixels) == 0;
            }
            (page.starts, lossless)
        };
        assert_eq!(play(stripes), (0, true), "a bar is stills, lossless");
        assert_eq!(play(paint).0, 1, "a video of its tiles streams");
    }

    /// A region one large change began is kept only for what moves in it
    /// that would be dear as stills, which some of its tiles are coded to
    /// find, there being no stills under a region to count: with a striped
    /// bar moving in it, it ends a second on as with nothing moving, and
    /// the bar is stills; with a video over a part of it, it ends for a
    /// region of the video; with a video over all of it, it is kept.
    #[test]
    fn a_region_one_change_began_is_kept_for_what_is_dear() {
        let (w, h) = (640u16, 384u16);
        let whole = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        let play = |moving: Rect, frame: fn(&mut [u8], u16, Rect, u32)| {
            let mut pixels = vec![255; usize::from(w) * usize::from(h) * 4];
            let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
            let mut page = Page::new(w, h);
            let t0 = begin(&mut encoder, &mut page, &pixels);
            // A window of noise opens over the whole desktop: one change
            // past an update's stills, a region around it.
            paint(&mut pixels, w, whole, 9);
            frame(&mut pixels, w, moving, 0);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[whole]),
                t0 + ms(50),
            );
            assert_eq!(
                (page.starts, page.streams.len()),
                (1, 1),
                "a region around it"
            );
            for n in 1..=60u32 {
                frame(&mut pixels, w, moving, n);
                step(
                    &mut encoder,
                    &mut page,
                    &pixels,
                    Some(&[moving]),
                    t0 + ms(50 + u64::from(n) * 50),
                );
            }
            (page.starts, page.ends, page.streams.len())
        };
        let bar = Rect {
            x: 64,
            y: 64,
            width: 512,
            height: 64,
        };
        assert_eq!(
            play(bar, stripes),
            (1, 1, 0),
            "ended with a bar moving in it, which is stills"
        );
        let video = Rect {
            x: 128,
            y: 64,
            width: 192,
            height: 128,
        };
        assert_eq!(
            play(video, paint),
            (2, 1, 1),
            "ended for a region of the video in it"
        );
        assert_eq!(
            play(whole, paint),
            (1, 0, 1),
            "kept for a video over all of it"
        );
    }

    /// A region one large change began is judged before its second is up
    /// whenever the layout before it was: a video over all of it at seven
    /// frames a second, begun a moment after a layout with nothing live,
    /// is kept, where the layouts counted from that one came just short of
    /// half a second in, too soon to judge, and next after the tick that
    /// ends a region nothing has kept for a second.
    #[test]
    fn a_region_one_change_began_is_judged_before_its_second() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = vec![255; usize::from(w) * usize::from(h) * 4];
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels) + ms(5000);
        // A layout with nothing live, 50 ms before the change.
        let dot = Rect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        };
        stripes(&mut pixels, w, dot, 1);
        step(&mut encoder, &mut page, &pixels, Some(&[dot]), t0 - ms(50));
        let whole = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        let mut out = Vec::new();
        for n in 0..=14u64 {
            let now = t0 + ms(n * 150);
            // The ticks owed before the frame, as a session sends them.
            while let Some(at) = encoder.tick_at().filter(|at| *at < now) {
                out.clear();
                let update = encoder.encode(at, &mut out).unwrap();
                page.draw(update.rects, &out);
            }
            paint(&mut pixels, w, whole, 20 + n as u32);
            step(&mut encoder, &mut page, &pixels, Some(&[whole]), now);
        }
        assert_eq!(
            (page.starts, page.ends, page.streams.len()),
            (1, 0, 1),
            "one stream throughout"
        );
    }

    /// A stream that has made no frame within its second ends though what
    /// it covers keeps moving and the layouts would keep it: a video over
    /// the whole desktop on a coder that makes no frame is stills from then,
    /// every frame of it whole and lossless, with no region laid out again
    /// until the coder is tried once more.
    #[test]
    fn a_stream_that_does_not_start_in_time_gives_way_to_stills() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = vec![255; usize::from(w) * usize::from(h) * 4];
        let mut encoder = TilesEncoder::new(
            w,
            h,
            held(),
            Box::new(Fake {
                refuse: false,
                mute: true,
                fail_after: None,
                fail_read_after: None,
            }),
        )
        .unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let whole = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        let mut out = Vec::new();
        for n in 1..=60u64 {
            let now = t0 + ms(n * 50);
            while let Some(at) = encoder.tick_at().filter(|at| *at < now) {
                out.clear();
                let update = encoder.encode(at, &mut out).unwrap();
                page.draw(update.rects, &out);
            }
            paint(&mut pixels, w, whole, 30 + n as u32);
            step(&mut encoder, &mut page, &pixels, Some(&[whole]), now);
            if n == 15 {
                assert_eq!(encoder.live.len(), 1, "a region, which makes no frame");
                assert!(page.off(&pixels) > 0, "and the client holds none of it");
            }
            if n >= 30 {
                assert!(encoder.live.is_empty(), "no region at frame {n}");
                assert_eq!(page.off(&pixels), 0, "frame {n} whole, as stills");
            }
        }
        assert_eq!((page.starts, page.ends), (0, 0), "no stream ever started");
    }

    /// A region whose motion keeps to a smaller box is laid out again over
    /// that box two seconds on: a video that filled the desktop, as a page
    /// scrolled with a video on it does, and then moves in a part of it
    /// alone, is that part's region, its own keyframe, and the rest settles
    /// lossless while it plays; one that moves over more than half its region
    /// keeps its stream.
    #[test]
    fn a_region_shrinks_to_what_still_moves() {
        let (w, h) = (640u16, 384u16);
        let whole = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        let play = |part: Rect| {
            let mut pixels = noise(w, h, 5);
            let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
            let mut page = Page::new(w, h);
            let t0 = begin(&mut encoder, &mut page, &pixels);
            let (starts, ends) = (page.starts, page.ends);
            // All of it for two seconds, then the part alone for six.
            for n in 1..=160u32 {
                let moving = if n <= 40 { whole } else { part };
                paint(&mut pixels, w, moving, 300 + n);
                step(
                    &mut encoder,
                    &mut page,
                    &pixels,
                    Some(&[moving]),
                    t0 + ms(u64::from(n) * 50),
                );
                if n == 75 {
                    assert_eq!(
                        (page.starts - starts, page.ends - ends),
                        (1, 0),
                        "one region still, under two seconds after the motion shrank"
                    );
                }
            }
            let area = encoder
                .live
                .first()
                .map(|live| live.area.within(Area::whole((w, h))));
            // What the stream no longer covers is settled while it plays.
            let outside = usize::from(w) * 4 * 300;
            let settled = page.canvas[outside..] == pixels[outside..];
            (page.starts - starts, page.ends - ends, area, settled)
        };
        let part = Rect {
            x: 128,
            y: 64,
            width: 192,
            height: 128,
        };
        assert_eq!(
            play(part),
            (2, 1, Some(part), true),
            "laid out again over the part, the rest lossless"
        );
        let most = Rect {
            x: 0,
            y: 0,
            width: 576,
            height: 384,
        };
        let (starts, ends, area, _) = play(most);
        assert_eq!(
            (starts, ends, area),
            (1, 0, Some(whole)),
            "kept where what moves is over half of it"
        );
    }

    /// A region is judged again once its motion has kept to a smaller box:
    /// a video over the whole desktop that stops with a striped bar still
    /// moving on it, which kept the region for as long as it moved, is
    /// ended two seconds and the region's own second on, no region laid out
    /// for the bar, which is stills, and the desktop settles lossless
    /// around it while it moves.
    #[test]
    fn a_region_left_with_what_is_cheap_ends() {
        let (w, h) = (640u16, 384u16);
        let whole = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        let bar = Rect {
            x: 64,
            y: 64,
            width: 512,
            height: 64,
        };
        let mut pixels = vec![255; usize::from(w) * usize::from(h) * 4];
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        // A video over all of it for two seconds, then the bar alone for
        // eight.
        for n in 1..=200u32 {
            let moving = if n <= 40 { whole } else { bar };
            if n <= 40 {
                paint(&mut pixels, w, whole, 300 + n);
            } else {
                stripes(&mut pixels, w, bar, n);
            }
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[moving]),
                t0 + ms(u64::from(n) * 50),
            );
            if n == 75 {
                assert_eq!(
                    (page.starts, page.ends),
                    (1, 0),
                    "kept by the bar, under two seconds after the video stopped"
                );
            }
        }
        assert_eq!(
            (page.starts, page.ends, page.streams.len()),
            (1, 1, 0),
            "ended, and no region for the bar"
        );
        assert_eq!(page.off(&pixels), 0, "lossless under the bar and around it");
    }

    /// A region to be laid out again over a smaller box keeps the stream it
    /// has where the coder refuses the one over the box: the session goes
    /// on, the video in the region it had.
    #[test]
    fn a_region_the_coder_refuses_to_shrink_keeps_its_stream() {
        /// A coder that opens `left` streams and refuses the rest.
        struct Few {
            left: usize,
        }
        impl H264Coder for Few {
            fn open(&mut self, width: u16, height: u16) -> Result<Box<dyn H264Stream>, H264Error> {
                if self.left == 0 {
                    return Err("no surface left".into());
                }
                self.left -= 1;
                Fake::coder().open(width, height)
            }
        }
        let (w, h) = (640u16, 384u16);
        let whole = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        let part = Rect {
            x: 128,
            y: 64,
            width: 192,
            height: 128,
        };
        let mut pixels = vec![255; usize::from(w) * usize::from(h) * 4];
        let mut encoder = TilesEncoder::new(w, h, held(), Box::new(Few { left: 1 })).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        assert_eq!(page.starts, 0, "a flat first picture is stills");
        // A video over all of it for two seconds, then over the part alone
        // for six: laid out again over the part twice, and refused twice.
        for n in 1..=160u32 {
            let moving = if n <= 40 { whole } else { part };
            paint(&mut pixels, w, moving, 300 + n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[moving]),
                t0 + ms(u64::from(n) * 50),
            );
        }
        assert_eq!(
            (page.starts, page.ends, page.streams.len()),
            (1, 0, 1),
            "the one stream throughout"
        );
        assert_eq!(
            encoder.live[0].area.within(Area::whole((w, h))),
            whole,
            "over all it was laid out over"
        );
    }

    /// A region one large change begins counts the layouts from itself, and
    /// leaves the regions beside it the time that takes from theirs: a
    /// video playing at seven frames a second is not found idle by a tick
    /// because a window opened beside it and put off the layout that says it
    /// moves, whichever of its frames the window opens on.
    #[test]
    fn a_region_one_change_begins_ends_none_beside_it() {
        let (w, h) = (640u16, 384u16);
        let video = Rect {
            x: 0,
            y: 0,
            width: 192,
            height: 192,
        };
        let window = Rect {
            x: 320,
            y: 0,
            width: 320,
            height: 384,
        };
        for opens in 12..=16u64 {
            let mut pixels = vec![255; usize::from(w) * usize::from(h) * 4];
            let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
            let mut page = Page::new(w, h);
            let t0 = begin(&mut encoder, &mut page, &pixels);
            for n in 1..=opens + 14 {
                paint(&mut pixels, w, video, 40 + n as u32);
                if n < opens {
                    step(
                        &mut encoder,
                        &mut page,
                        &pixels,
                        Some(&[video]),
                        t0 + ms(n * 150),
                    );
                    continue;
                }
                if n == opens {
                    assert_eq!(
                        (page.starts, page.ends),
                        (1, 0),
                        "the video a region before the window opens at frame {opens}"
                    );
                }
                // The window, of a video itself, so that its region is kept.
                paint(&mut pixels, w, window, 90 + n as u32);
                step(
                    &mut encoder,
                    &mut page,
                    &pixels,
                    Some(&[video, window]),
                    t0 + ms(n * 150),
                );
            }
            assert_eq!(
                (page.starts, page.ends, page.streams.len()),
                (2, 0, 2),
                "both streams throughout, the window opened at frame {opens}"
            );
        }
    }

    /// A stream out of its second to start is counted against the coder
    /// whichever of the layout and the tick comes to it first: one with only
    /// a striped bar moving in it, which the layout would end as idle a
    /// second after it was laid out, ahead of that update's tick, still
    /// holds the next region off.
    #[test]
    fn a_stream_the_layout_would_end_unstarted_holds_the_next_off() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = vec![255; usize::from(w) * usize::from(h) * 4];
        let mut encoder = TilesEncoder::new(
            w,
            h,
            held(),
            Box::new(Fake {
                refuse: false,
                mute: true,
                fail_after: None,
                fail_read_after: None,
            }),
        )
        .unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let whole = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        let bar = Rect {
            x: 64,
            y: 64,
            width: 512,
            height: 64,
        };
        paint(&mut pixels, w, whole, 9);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[whole]),
            t0 + ms(50),
        );
        assert_eq!(encoder.live.len(), 1, "a region around the change");
        // The bar every 50 ms: the update a second on has a layout, and
        // the stream is out of its second at it.
        for n in 1..=20u32 {
            stripes(&mut pixels, w, bar, n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[bar]),
                t0 + ms(50 + u64::from(n) * 50),
            );
        }
        assert!(encoder.live.is_empty(), "ended at its second");
        assert_eq!(
            encoder.no_streams_until,
            t0 + ms(1050) + START_RETRY,
            "and no region for a while after"
        );
        assert_eq!(page.off(&pixels), 0, "the change whole, as stills");
    }

    /// A region whose stream never made a frame ends at its second with the
    /// client holding nothing of the change that began it: all of it goes
    /// as stills in that update, though they are past an update's stills,
    /// and no region is laid out over them again, which would be the one
    /// that never started, owed anew every second and never sent.
    #[test]
    fn a_region_that_never_started_is_sent_as_stills() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = vec![255; usize::from(w) * usize::from(h) * 4];
        let mut encoder = TilesEncoder::new(
            w,
            h,
            held(),
            Box::new(Fake {
                refuse: false,
                mute: true,
                fail_after: None,
                fail_read_after: None,
            }),
        )
        .unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let whole = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        paint(&mut pixels, w, whole, 9);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[whole]),
            t0 + ms(50),
        );
        assert_eq!(encoder.live.len(), 1, "a region around the change");
        assert!(page.off(&pixels) > 0, "which made no frame of it");
        let webps = page.webps;
        let at = settle(&mut encoder, &mut page, t0 + ms(50));
        assert!(
            at <= t0 + ms(50) + STREAM_IDLE + CLEANUP_TICK,
            "ended at its second, and nothing owed after: {at:?}"
        );
        assert_eq!(page.off(&pixels), 0, "the change whole, as stills");
        assert_eq!(
            page.webps - webps,
            4,
            "a still a row of tiles, none sent twice: the two rows the \
             change's own update carried are draws of the cache"
        );
        assert_eq!((page.starts, page.ends), (0, 0), "no stream ever started");
        assert!(encoder.live.is_empty() && !encoder.owed());
    }

    /// A stream that ends with damage no frame of it carried leaves that
    /// damage as stills in the same update, not the stream's last frame from
    /// before it.
    #[test]
    fn a_stream_that_ends_sends_what_it_did_not_carry() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = noise(w, h, 5);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let video = Rect {
            x: 128,
            y: 64,
            width: 192,
            height: 128,
        };
        for n in 1..=40u32 {
            paint(&mut pixels, w, video, 300 + n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[video]),
                t0 + ms(u64::from(n) * 50),
            );
        }
        assert_eq!(
            (page.starts, page.ends),
            (2, 1),
            "one stream live, after the first picture's"
        );
        // Its motion last seen at 1.8 s. A tile of it redrawn twice once that
        // stopped, which is no motion but is carried, its tiles owed the
        // ceiling from then; then one change under it in the round that ends
        // it, idle, before any frame of it carries the change, and before its
        // tiles are owed their stills.
        let now_and_then = Rect {
            x: 128,
            y: 64,
            width: 64,
            height: 64,
        };
        for (n, at) in [(1, 2600), (2, 2750)] {
            paint(&mut pixels, w, now_and_then, 900 + n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[now_and_then]),
                t0 + ms(at),
            );
        }
        assert_eq!(page.ends, 1, "the stream still live");
        let change = Rect {
            x: 256,
            y: 128,
            width: 64,
            height: 64,
        };
        paint(&mut pixels, w, change, 999);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[change]),
            t0 + ms(3100),
        );
        assert_eq!(page.ends, 2, "the stream ended");
        let row = |y: usize| {
            (y * usize::from(w) + usize::from(change.x)) * 4
                ..(y * usize::from(w) + usize::from(change.x + change.width)) * 4
        };
        let held = (usize::from(change.y)..usize::from(change.y + change.height)).all(|y| {
            page.canvas[row(y)]
                .as_chunks::<4>()
                .0
                .iter()
                .zip(pixels[row(y)].as_chunks::<4>().0)
                .all(|(a, b)| a[..3] == b[..3])
        });
        assert!(
            held,
            "the change is held lossless, not the stream's last frame"
        );
    }

    /// A stream's START is no keyframe: a region's IDR is the load a desktop
    /// that moves keeps sending, where the picture whole again is a burst
    /// once.
    #[test]
    fn a_stream_that_starts_is_no_keyframe() {
        let (w, h) = (640u16, 384u16);
        let mut pixels = noise(w, h, 7);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let video = Rect {
            x: 0,
            y: 0,
            width: 192,
            height: 128,
        };
        for n in 1..=12u32 {
            paint(&mut pixels, w, video, 500 + n);
            let made = page.starts;
            let one = update(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[video]),
                t0 + ms(u64::from(n) * 50),
            );
            assert!(!one.keyframe, "frame {n}, a START among it or not: {one:?}");
            if page.starts > made {
                assert!(
                    one.rects > 0 && !one.tick,
                    "the START is what the read brought: {one:?}"
                );
            }
        }
        assert_eq!(
            (page.starts, page.ends),
            (2, 1),
            "a stream live, after the first picture's"
        );
    }

    /// An update is the tick's when all it sends is what the desktop
    /// stopping owes — a progressive tile's upgrade, a stream's end —
    /// with a read before it that changed nothing or none, and not once a
    /// read brought anything; the session hands the walk no verdict of it.
    /// Stopped where nothing changed for half a second before it, and not
    /// between a typist's keys.
    #[test]
    fn an_update_of_the_tick_alone_says_so() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 21);
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let first = update(&mut encoder, &mut page, &pixels, None, ms(0));
        assert!(first.keyframe && !first.tick, "{first:?}");
        let now = settle(&mut encoder, &mut page, ms(0));
        encoder.set_pace(30, false);
        let busy = Rect {
            x: 64,
            y: 0,
            width: 64,
            height: 64,
        };
        paint(&mut pixels, w, busy, 22);
        let lossy = update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[busy]),
            now + ms(100),
        );
        assert!(
            lossy.rects == 2 && !lossy.tick,
            "what the read brought, and its store: {lossy:?}"
        );
        // The read the session makes with nothing changed, as it does at a
        // tick, until the debt is old enough to be paid.
        let paid = loop {
            let at = encoder
                .tick_at()
                .expect("a first pass below the full stage is owed");
            let paid = update(&mut encoder, &mut page, &pixels, Some(&[]), at);
            if paid.rects > 0 {
                break paid;
            }
            assert!(paid.tick);
        };
        assert!(
            paid.rects > 0 && paid.tick && paid.stopped,
            "the debt, an upgrade, the desktop stopped: {paid:?}"
        );
        let now = settle(&mut encoder, &mut page, now + ms(100));
        assert!(!encoder.owed());

        // A tick between a typist's keys: the first key's debt, paid while
        // the second's change is under half a second old.
        let other = Rect {
            x: 192,
            y: 64,
            width: 64,
            height: 64,
        };
        paint(&mut pixels, w, busy, 25);
        update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[busy]),
            now + ms(100),
        );
        paint(&mut pixels, w, other, 26);
        update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[other]),
            now + ms(500),
        );
        let between = loop {
            let at = encoder.tick_at().expect("owed");
            let paid = update(&mut encoder, &mut page, &pixels, Some(&[]), at);
            if paid.rects > 0 {
                break paid;
            }
        };
        assert!(
            between.tick && !between.stopped,
            "the tick's alone, the desktop not stopped: {between:?}"
        );
        let now = settle(&mut encoder, &mut page, now + ms(600));

        // A change read at the tick's time is the read's, debts and all.
        paint(&mut pixels, w, busy, 23);
        update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[busy]),
            now + ms(100),
        );
        let at = encoder.tick_at().expect("owed again");
        paint(&mut pixels, w, other, 24);
        let both = update(&mut encoder, &mut page, &pixels, Some(&[other]), at);
        assert!(
            both.rects > 0 && !both.tick,
            "a change and the tick: {both:?}"
        );
    }

    /// A new walk ends every live stream before the picture starts over.
    #[test]
    fn a_new_walk_ends_the_live_streams() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 31);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let video = Rect {
            x: 0,
            y: 0,
            width: 256,
            height: 192,
        };
        for n in 1..=12u32 {
            paint(&mut pixels, w, video, 700 + n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[video]),
                t0 + ms(u64::from(n) * 50),
            );
        }
        assert_eq!(
            (page.starts, page.ends),
            (2, 1),
            "a stream live, after the first picture's"
        );
        encoder.set_picture(progressive());
        let whole = update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[video]),
            t0 + ms(700),
        );
        assert!(whole.keyframe, "the picture starts over: {whole:?}");
        assert_eq!(page.ends, 2, "the live stream ended");
        assert!(page.streams.is_empty());
    }

    /// A redraw of what a live stream carries, the pixels the client holds,
    /// codes no frame of it.
    #[test]
    fn a_redraw_inside_a_stream_codes_nothing() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 5);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let video = Rect {
            x: 0,
            y: 0,
            width: 256,
            height: 192,
        };
        for n in 1..=12u32 {
            paint(&mut pixels, w, video, 300 + n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[video]),
                t0 + ms(u64::from(n) * 50),
            );
        }
        assert_eq!(
            (page.starts, page.ends),
            (2, 1),
            "a stream live, after the first picture's"
        );
        let frames = page.frames;
        assert_eq!(
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[video]),
                t0 + ms(650)
            ),
            0,
            "a redraw of the stream's pixels was sent"
        );
        assert_eq!(page.frames, frames);
    }

    /// A client that asks for the picture whole is sent every stream's end and
    /// the picture again: where the stream was, as WebP; where it was not, the
    /// tiles of the first picture, from the cache.
    #[test]
    fn a_picture_forgotten_ends_its_streams_and_starts_over() {
        let (w, h) = (512u16, 256u16);
        let mut pixels = noise(w, h, 4);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let video = Rect {
            x: 0,
            y: 0,
            width: 256,
            height: 128,
        };
        for n in 1..=20u32 {
            paint(&mut pixels, w, video, 200 + n);
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[video]),
                t0 + ms(u64::from(n) * 50),
            );
        }
        assert_eq!((page.starts, page.ends), (2, 1));
        encoder.forget();
        assert_eq!(
            encoder.tick_at(),
            Some(Duration::ZERO),
            "the ends are owed at once, read or no read"
        );
        let (webps, cached) = (page.webps, page.cached);
        step(&mut encoder, &mut page, &pixels, None, t0 + ms(1100));
        assert_eq!(
            (page.starts, page.ends),
            (2, 2),
            "the stream ended, and the picture again within an update's stills"
        );
        assert_eq!(
            page.webps - webps,
            2,
            "the two bands the stream crossed as WebP"
        );
        assert_eq!(
            page.cached - cached,
            24,
            "every other tile from its slot, the last two bands whole"
        );
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
    /// is the same slot again, and the page holds it losslessly.
    #[test]
    fn a_tile_shown_again_is_drawn_from_the_cache() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 31);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        assert_eq!(
            page.stored, 8,
            "every whole tile of the first picture stored"
        );

        let text = Rect {
            x: 64,
            y: 0,
            width: 64,
            height: 64,
        };
        glyphs(&mut pixels, w, text);
        let (webps, stored) = (page.webps, page.stored);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[text]),
            t0 + ms(100),
        );
        assert_eq!(
            (page.webps - webps, page.stored - stored, page.cached),
            (1, 1, 0),
            "a tile of text: a WebP, stored"
        );

        same_tile(&mut pixels, w, (64, 0), (192, 64));
        let (webps, stored) = (page.webps, page.stored);
        let shown = update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 192,
                y: 64,
                width: 64,
                height: 64,
            }]),
            t0 + ms(200),
        );
        assert_eq!(
            (
                shown.rects,
                page.webps - webps,
                page.stored - stored,
                page.cached
            ),
            (1, 0, 0, 1),
            "the same text elsewhere: drawn from the cache, no still, nothing stored"
        );
        assert_eq!(page.off(&pixels), 0);
        assert!(!encoder.owed(), "a tile stored lossless is owed nothing");

        // Painted over and shown again: the slot still has it.
        paint(&mut pixels, w, text, 32);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[text]),
            t0 + ms(300),
        );
        glyphs(&mut pixels, w, text);
        let webps = page.webps;
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[text]),
            t0 + ms(400),
        );
        assert_eq!(
            (page.webps - webps, page.cached),
            (0, 2),
            "shown a third time, from the cache again"
        );
        assert_eq!(page.off(&pixels), 0);

        // A tile the damage covers in part is a still of that part, which
        // the cache neither keeps nor draws: the client's tile is not whole
        // in it.
        let part = Rect {
            x: 200,
            y: 70,
            width: 10,
            height: 10,
        };
        paint(&mut pixels, w, part, 33);
        let (webps, stored) = (page.webps, page.stored);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[part]),
            t0 + ms(500),
        );
        assert_eq!(
            (page.webps - webps, page.stored - stored, page.cached),
            (1, 0, 2),
            "a part of a tile: a WebP of it, not stored"
        );
        assert_eq!(page.off(&pixels), 0);
    }

    /// A tile of one colour is never stored: a WebP of it is a few bytes,
    /// and a slot is worth more than that.
    #[test]
    fn a_flat_tile_is_not_stored() {
        let (w, h) = (128u16, 64u16);
        let mut pixels = noise(w, h, 41);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        assert_eq!(page.stored, 2);
        let flat = Rect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        };
        for row in 0..64 {
            pixels[row * usize::from(w) * 4..][..64 * 4]
                .copy_from_slice(&[10, 20, 30, 0].repeat(64));
        }
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[flat]),
            t0 + ms(100),
        );
        assert_eq!(
            (page.webps, page.stored),
            (2, 2),
            "the flat tile's WebP, and no store"
        );
        same_tile(&mut pixels, w, (0, 0), (64, 0));
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 64,
                y: 0,
                width: 64,
                height: 64,
            }]),
            t0 + ms(200),
        );
        assert_eq!(
            (page.webps, page.stored, page.cached),
            (3, 2, 0),
            "the same flat tile elsewhere: a WebP again, and no cache"
        );
        assert_eq!(page.off(&pixels), 0);
    }

    /// On a walked list a tile stored below the full stage is drawn from the
    /// cache, and the tile it draws is owed the full stage, as a copy of one
    /// is; the settle sends both whole or upgraded and stores the tile again
    /// in its slot, settled, so a third draw of it owes nothing.
    #[test]
    fn a_tile_cached_below_the_full_stage_is_owed_it() {
        let (w, h) = (256u16, 128u16);
        let mut pixels = noise(w, h, 51);
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        let now = settle(&mut encoder, &mut page, ms(0));

        encoder.set_pace(30, false);
        let text = Rect {
            x: 64,
            y: 0,
            width: 64,
            height: 64,
        };
        glyphs(&mut pixels, w, text);
        let (stored, cached) = (page.stored, page.cached);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[text]),
            now + ms(100),
        );
        assert_eq!(
            (page.stored - stored, page.cached - cached),
            (1, 0),
            "a first pass at the dial, stored"
        );
        same_tile(&mut pixels, w, (64, 0), (192, 64));
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 192,
                y: 64,
                width: 64,
                height: 64,
            }]),
            now + ms(200),
        );
        assert_eq!(
            page.cached - cached,
            1,
            "the same text elsewhere, below the full stage: drawn from the cache"
        );
        assert!(encoder.owed(), "both tiles are owed the full stage");
        let coarse = page.distance(&pixels);

        encoder.set_pace(80, false);
        let now = settle(&mut encoder, &mut page, now + ms(200));
        assert!(page.distance(&pixels) < coarse, "finer at the full stage");
        assert!(!encoder.owed());

        // Below the ceiling again, the tile settled is drawn from the cache
        // and owed nothing: it is the full stage's picture.
        encoder.set_pace(30, false);
        same_tile(&mut pixels, w, (64, 0), (0, 64));
        let cached = page.cached;
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 0,
                y: 64,
                width: 64,
                height: 64,
            }]),
            now + ms(100),
        );
        assert_eq!(page.cached - cached, 1);
        assert!(
            !encoder.owed(),
            "a tile drawn from a settled slot is owed nothing"
        );
    }

    /// A picture that starts over keeps the cache, as the client does: what
    /// the client was shown is drawn from its slots, the whole picture with
    /// no still. Once the client says its cache is gone, the picture is
    /// stills again, every whole tile stored anew.
    #[test]
    fn a_picture_forgotten_is_drawn_from_the_cache() {
        let (w, h) = (128u16, 64u16);
        let mut pixels = noise(w, h, 61);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let text = Rect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        };
        glyphs(&mut pixels, w, text);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[text]),
            t0 + ms(100),
        );
        assert_eq!((page.webps, page.stored, page.cached), (2, 3, 0));
        encoder.forget();
        step(&mut encoder, &mut page, &pixels, None, t0 + ms(200));
        assert_eq!(
            (page.webps, page.stored, page.cached),
            (2, 3, 2),
            "the picture again, whole, every tile from its slot"
        );
        assert_eq!(page.off(&pixels), 0);
        assert!(
            !encoder.owed(),
            "drawn from slots stored lossless, owed nothing"
        );

        encoder.forget_cache();
        encoder.forget();
        page.slots.clear();
        step(&mut encoder, &mut page, &pixels, None, t0 + ms(300));
        assert_eq!(
            (page.webps, page.stored, page.cached),
            (3, 5, 2),
            "the cache gone: the picture as a still, every whole tile stored anew"
        );
        same_tile(&mut pixels, w, (0, 0), (64, 0));
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 64,
                y: 0,
                width: 64,
                height: 64,
            }]),
            t0 + ms(400),
        );
        assert_eq!(
            (page.webps, page.cached),
            (3, 3),
            "and from then on the cache serves again"
        );
        assert_eq!(page.off(&pixels), 0);
    }

    /// A new size is a new encoder, which draws from what the old one
    /// stored: the tiles the client was shown before are slots drawn where
    /// they now stand, and only what is new is a still.
    #[test]
    fn a_new_size_keeps_the_cache() {
        let (w, h) = (128u16, 64u16);
        let pixels = noise(w, h, 62);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        assert_eq!(page.stored, 2);

        // The same two tiles, swapped, beside a third that is new.
        let wide = 192u16;
        let mut resized = noise(wide, h, 63);
        for (from, to) in [(0, 64), (64, 0)] {
            for row in 0..usize::from(h) {
                let src = (row * usize::from(w) + from) * 4;
                let dst = (row * usize::from(wide) + to) * 4;
                resized[dst..dst + 64 * 4].copy_from_slice(&pixels[src..src + 64 * 4]);
            }
        }
        let mut grown = TilesEncoder::new(wide, h, held(), Fake::coder()).unwrap();
        grown.keep_cache(encoder);
        let mut bigger = Page::new(wide, h);
        bigger.slots = std::mem::take(&mut page.slots);
        step(&mut grown, &mut bigger, &resized, None, t0 + ms(100));
        assert_eq!(
            (bigger.webps, bigger.cached, bigger.stored),
            (1, 2, 1),
            "two tiles from their slots, the new one a still, stored"
        );
        assert_eq!(bigger.off(&resized), 0);
    }

    /// A walked list's tiles are stored as a progressive tile left them,
    /// which a held list is not owed: switched to one, the picture again is
    /// stills, lossless, stored again in the slots they were in, and a tile
    /// shown after that is drawn from its slot.
    #[test]
    fn a_held_list_draws_no_tile_a_walked_one_stored() {
        let (w, h) = (128u16, 64u16);
        let mut pixels = soft(w, h, 0);
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &pixels, None, ms(0));
        settle(&mut encoder, &mut page, ms(0));
        let stored = page.stored;
        assert!(stored >= 2, "every tile stored, settled");

        encoder.set_picture(held());
        step(&mut encoder, &mut page, &pixels, None, ms(5000));
        assert_eq!(
            page.cached, 0,
            "nothing a walked list stored is drawn on a held one"
        );
        assert_eq!(page.off(&pixels), 0, "the picture lossless");
        assert_eq!(page.stored - stored, 2, "and stored again, lossless");
        assert_eq!(page.slots.len(), 2, "in the slots they were in");

        same_tile(&mut pixels, w, (0, 0), (64, 0));
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 64,
                y: 0,
                width: 64,
                height: 64,
            }]),
            ms(5100),
        );
        assert_eq!(page.cached, 1, "a lossless slot is drawn on a held list");
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
            let (x, y) = (
                col * usize::from(TILE) + i % 64,
                row * usize::from(TILE) + (i / 64) % 64,
            );
            pixels[(y * w + x) * 4..][..4].copy_from_slice(&[(i / 4096) as u8 + 1, 7, 9, 0]);
        }
        pixels
    }

    /// A tile drawn from a slot that the stores of the run ahead of it would
    /// take as the least recently used: the draw names the slot first, so
    /// the stores take others, and the page holds the picture losslessly. The
    /// picture has 64 tiles more than the cache has slots, so its first 64
    /// tiles' slots were taken again by its last 64, and the slot of the
    /// first tile of the second row is the least recently used.
    #[test]
    fn a_draw_keeps_its_slot_from_the_stores_ahead_of_it() {
        let (cols, rows) = (64u16, 65u16);
        let (w, h) = (cols * TILE, rows * TILE);
        let mut pixels = dots(cols, rows);
        // More than an update's stills, which would go as video; the coder
        // refuses, so the picture goes as stills, every tile stored in order.
        let mut encoder = TilesEncoder::new(
            w,
            h,
            held(),
            Box::new(Fake {
                refuse: true,
                mute: false,
                fail_after: None,
                fail_read_after: None,
            }),
        )
        .unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        assert_eq!(
            (page.starts, page.stored),
            (0, usize::from(cols) * usize::from(rows))
        );
        // The second row painted over: 63 tiles new, which take 63 slots, and
        // the last the first tile's old content, in the slot they would take first.
        let second = Rect {
            x: 0,
            y: TILE,
            width: w,
            height: TILE,
        };
        let old: Vec<u8> = (0..usize::from(TILE))
            .flat_map(|row| {
                pixels[((usize::from(TILE) + row) * usize::from(w)) * 4..][..usize::from(TILE) * 4]
                    .to_vec()
            })
            .collect();
        paint(
            &mut pixels,
            w,
            Rect {
                width: w - TILE,
                ..second
            },
            71,
        );
        for (row, line) in old.chunks_exact(usize::from(TILE) * 4).enumerate() {
            let at = ((usize::from(TILE) + row) * usize::from(w) + usize::from(w - TILE)) * 4;
            pixels[at..at + line.len()].copy_from_slice(line);
        }
        let (webps, stored) = (page.webps, page.stored);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[second]),
            t0 + ms(100),
        );
        assert_eq!(
            (page.webps - webps, page.stored - stored, page.cached),
            (1, 63, 1),
            "63 tiles stored and the last drawn from its slot"
        );
        assert_eq!(
            page.off(&pixels),
            0,
            "the drawn tile is what the slot held when it was named"
        );
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
        assert_eq!(
            cache.store(1_000_000, true),
            2,
            "slot 2 is the least recently used"
        );
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
    fn play_to_failure(
        encoder: &mut TilesEncoder,
        page: &mut Page,
        pixels: &mut [u8],
        w: u16,
        frames: u32,
        from: Duration,
    ) -> Option<TilesError> {
        let video = Rect {
            x: 0,
            y: 0,
            width: 256,
            height: 192,
        };
        let stride = usize::from(w) * 4;
        for n in 1..=frames {
            paint(pixels, w, video, 500 + n);
            let now = from + ms(u64::from(n) * 50);
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
        let mut encoder = TilesEncoder::new(
            w,
            h,
            held(),
            Box::new(Fake {
                refuse: true,
                mute: false,
                fail_after: None,
                fail_read_after: None,
            }),
        )
        .unwrap();
        let mut page = Page::new(w, h);
        let first = update(&mut encoder, &mut page, &pixels, None, ms(0));
        assert!(
            first.keyframe && page.starts == 0 && page.webps > 0 && page.off(&pixels) == 0,
            "a first picture the coder refuses goes as stills, lossless at once: {first:?}"
        );
        let failed = play_to_failure(&mut encoder, &mut page, &mut pixels, w, 20, ms(0));
        assert!(matches!(failed, Some(TilesError::Region(_))), "{failed:?}");
        assert_eq!(page.starts, 0, "no stream started");
    }

    /// A stream that fails midway, at its encode or at its read, fails the
    /// encode it was coded in.
    #[test]
    fn a_region_stream_that_fails_fails_the_encode() {
        let (w, h) = (512u16, 256u16);
        for (seed, fake) in [
            (
                8,
                Fake {
                    refuse: false,
                    mute: false,
                    fail_after: Some(4),
                    fail_read_after: None,
                },
            ),
            (
                10,
                Fake {
                    refuse: false,
                    mute: false,
                    fail_after: None,
                    fail_read_after: Some(4),
                },
            ),
        ] {
            let mut pixels = noise(w, h, seed);
            let mut encoder = TilesEncoder::new(w, h, held(), Box::new(fake)).unwrap();
            let mut page = Page::new(w, h);
            let t0 = begin(&mut encoder, &mut page, &pixels);
            let failed = play_to_failure(&mut encoder, &mut page, &mut pixels, w, 40, t0);
            assert!(matches!(failed, Some(TilesError::Region(_))), "{failed:?}");
            assert_eq!(
                (page.starts, page.frames, page.ends),
                (2, 3, 1),
                "the stream's four frames after the first picture's, then the failure"
            );
        }
    }

    /// Rows `top` on of a `w`-wide document, `h` of them: a page scrolled to `top`.
    fn scrolled(document: &[u8], w: u16, top: usize, h: u16) -> Vec<u8> {
        let line = usize::from(w) * 4;
        document[top * line..(top + usize::from(h)) * line].to_vec()
    }

    /// A page scrolled is a copy of what the client holds, and only the rows
    /// it uncovered go as stills, a band of tiles: the page lossless after it,
    /// for a fraction of the picture's bytes.
    #[test]
    fn a_scroll_is_a_copy() {
        let (w, h) = (256u16, 256u16);
        let document = noise(w, 3 * h, 21);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &scrolled(&document, w, 0, h));
        let (whole, webps) = (page.bytes, page.webps);
        let pixels = scrolled(&document, w, 40, h);
        let scroll = update(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }]),
            t0 + ms(100),
        );
        assert_eq!(
            (page.copies, page.webps - webps),
            (1, 1),
            "a copy, and the band the rows uncovered are in, a still"
        );
        assert!(!scroll.keyframe);
        assert_eq!(page.off(&pixels), 0, "the page lossless");
        assert!(
            page.bytes - whole < whole / 3,
            "{} bytes for a scroll, {whole} for the picture",
            page.bytes - whole
        );
        assert!(!encoder.owed(), "a copy of lossless pixels owes nothing");
    }

    /// A scroll that goes on stays copies, the tiles it moves no churn, so it
    /// never becomes a region's stream, and up as well as down.
    #[test]
    fn a_scroll_that_goes_on_is_no_stream() {
        let (w, h) = (320u16, 192u16);
        let document = noise(w, 4 * h, 22);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &scrolled(&document, w, 0, h));
        let tops = (1..=12)
            .map(|n| n * 24)
            .chain((0..12).rev().map(|n| n * 24));
        for (n, top) in tops.enumerate() {
            let pixels = scrolled(&document, w, top, h);
            let copies = page.copies;
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&[Rect {
                    x: 0,
                    y: 0,
                    width: w,
                    height: h,
                }]),
                t0 + ms(50 * (n as u64 + 1)),
            );
            assert_eq!(page.copies, copies + 1, "a copy at row {top}");
            let bad: Vec<usize> = (0..usize::from(h))
                .filter(|y| {
                    page.canvas[y * usize::from(w) * 4..(y + 1) * usize::from(w) * 4]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(
                            pixels[y * usize::from(w) * 4..(y + 1) * usize::from(w) * 4]
                                .as_chunks::<4>()
                                .0,
                        )
                        .any(|(a, b)| a[..3] != b[..3])
                })
                .collect();
            assert_eq!(
                page.off(&pixels),
                0,
                "the page lossless at row {top}: rows {bad:?} webps {}",
                page.webps
            );
        }
        assert_eq!(page.starts, 0, "no stream");
    }

    /// A window dragged over text is a copy of where it was: what it
    /// uncovered and the tiles its edges cut go as stills.
    #[test]
    fn a_window_dragged_is_a_copy() {
        let (w, h) = (384u16, 256u16);
        let mut desktop = vec![0; usize::from(w) * usize::from(h) * 4];
        glyphs(
            &mut desktop,
            w,
            Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            },
        );
        let window = noise(160, 112, 23);
        let place = |at: (u16, u16)| {
            let mut pixels = desktop.clone();
            for row in 0..112 {
                let to = ((usize::from(at.1) + row) * usize::from(w) + usize::from(at.0)) * 4;
                pixels[to..to + 160 * 4]
                    .copy_from_slice(&window[row * 160 * 4..(row + 1) * 160 * 4]);
            }
            pixels
        };
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &place((40, 30)));
        let pixels = place((110, 90));
        let moved = Rect {
            x: 40,
            y: 30,
            width: 160 + 70,
            height: 112 + 60,
        };
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[moved]),
            t0 + ms(100),
        );
        assert_eq!(page.copies, 1, "the window");
        assert_eq!(page.off(&pixels), 0);
        assert!(!encoder.owed());
    }

    /// A window whose every run is found at too many places for the scan to
    /// agree on any, dragged on as it was dragged before, is a copy at the
    /// offset it moved by then.
    #[test]
    fn a_window_dragged_on_is_a_copy_where_it_went_before() {
        let (w, h) = (512u16, 384u16);
        let desktop = vec![128; usize::from(w) * usize::from(h) * 4];
        let place = |at: (u16, u16)| {
            let mut pixels = desktop.clone();
            for row in 0..128 {
                for col in 0..192 {
                    let v = if (col % 8 < 3) ^ (row % 6 < 2) {
                        40
                    } else {
                        220
                    };
                    let to =
                        ((usize::from(at.1) + row) * usize::from(w) + usize::from(at.0) + col) * 4;
                    pixels[to..to + 4].copy_from_slice(&[v, v, v, 0]);
                }
            }
            pixels
        };
        let drag = |copied_by: Vec<(i32, i32)>| {
            let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
            let mut page = Page::new(w, h);
            step(&mut encoder, &mut page, &place((40, 30)), None, ms(0));
            encoder.copied_by = copied_by;
            let pixels = place((110, 90));
            let moved = Rect {
                x: 40,
                y: 30,
                width: 192 + 70,
                height: 128 + 60,
            };
            step(&mut encoder, &mut page, &pixels, Some(&[moved]), ms(100));
            assert_eq!(page.off(&pixels), 0);
            page.copies
        };
        assert_eq!(drag(Vec::new()), 0, "no run the scan finds says where");
        assert!(drag(vec![(-70, -60)]) > 0, "where it went before");
    }

    /// Two reads before an encode, a scroll each: the second's copy reads
    /// none of the rows the first uncovered, whose still is not sent yet, so
    /// the page is lossless after the one update that carries both.
    #[test]
    fn a_copy_reads_only_what_the_client_holds() {
        let (w, h) = (256u16, 256u16);
        let document = noise(w, 3 * h, 24);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &scrolled(&document, w, 0, h));
        let stride = usize::from(w) * 4;
        let all = [Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }];
        encoder
            .read(
                &scrolled(&document, w, 40, h),
                stride,
                Some(&all),
                t0 + ms(100),
            )
            .unwrap();
        let pixels = scrolled(&document, w, 100, h);
        encoder
            .read(&pixels, stride, Some(&all), t0 + ms(110))
            .unwrap();
        let mut out = Vec::new();
        let update = encoder.encode(t0 + ms(110), &mut out).unwrap();
        page.draw(update.rects, &out);
        assert_eq!(page.copies, 2, "both scrolls");
        assert_eq!(page.off(&pixels), 0, "the page lossless");
    }

    /// A picture with no edge that repeats, which a scroll's copy finds at
    /// one offset alone: three smooth ramps, one of them a product.
    fn ramps(w: u16, h: u16) -> Vec<u8> {
        (0..usize::from(h))
            .flat_map(|y| {
                (0..usize::from(w)).flat_map(move |x| {
                    [
                        ((x + 2 * y) / 3 % 256) as u8,
                        (x * y / 40 % 256) as u8,
                        ((3 * x + y) / 5 % 256) as u8,
                        0,
                    ]
                })
            })
            .collect()
    }

    /// On a walked list a copy from a settled source is settled: only the
    /// strip the scroll uncovered is owed, and the settle sends that alone.
    #[test]
    fn a_walked_copy_from_a_settled_source_owes_only_the_uncovered_strip() {
        let (w, h) = (256u16, 256u16);
        let document = ramps(w, 3 * h);
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(
            &mut encoder,
            &mut page,
            &scrolled(&document, w, 0, h),
            None,
            ms(0),
        );
        let now = settle(&mut encoder, &mut page, ms(0)) + ms(100);
        assert!(!encoder.owed(), "the first picture, settled, owes nothing");
        let pixels = scrolled(&document, w, 40, h);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }]),
            now,
        );
        assert_eq!(page.copies, 1);
        // 40 rows uncovered, all in the last row of tiles.
        let mut owed: Vec<_> = encoder.debts.keys().copied().collect();
        owed.sort_unstable();
        assert_eq!(
            owed,
            [(0, 3), (1, 3), (2, 3), (3, 3)],
            "the uncovered strip alone is owed"
        );
        let sent = page.progressives;
        settle(&mut encoder, &mut page, now);
        assert!(page.progressives > sent, "the uncovered strip sent");
        assert!(!encoder.owed());
    }

    /// A page that churns everywhere, as a scroll the search missed twice
    /// leaves it, is still looked among while a copy was found within the
    /// churn's window, and not once none was: a video churns so, and moves
    /// nothing.
    #[test]
    fn a_scroll_the_search_missed_is_looked_for_again() {
        let (w, h) = (256u16, 512u16);
        let document = noise(w, 4 * h, 34);
        let whole = [Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }];
        let scroll = |recently: bool| {
            let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
            let mut page = Page::new(w, h);
            step(
                &mut encoder,
                &mut page,
                &scrolled(&document, w, 0, h),
                None,
                ms(0),
            );
            let now = settle(&mut encoder, &mut page, ms(0)) + ms(100);
            step(
                &mut encoder,
                &mut page,
                &scrolled(&document, w, 24, h),
                Some(&whole),
                now,
            );
            assert_eq!(page.copies, 1, "the first scroll a copy");
            for (cell, _) in tiles_of(Area::whole((w, h))) {
                encoder.churn.observe(cell, now + ms(100));
                encoder.churn.observe(cell, now + ms(200));
            }
            if !recently {
                encoder.copied_at = None;
            }
            step(
                &mut encoder,
                &mut page,
                &scrolled(&document, w, 48, h),
                Some(&whole),
                now + ms(300),
            );
            page.copies
        };
        assert_eq!(scroll(true), 2, "looked among while scrolling");
        assert_eq!(scroll(false), 1, "churn left out otherwise");
    }

    /// An offset the scan found that grows into no copy is no reason to stop
    /// looking: the rows it stepped over are scanned, and the scroll is a
    /// copy. Thirty-two tiles, a probe each on the row of its column's
    /// residue, scanned every fourth row: the page scrolls 24 rows but for
    /// its first column, new, the only probes the scan would find at that
    /// offset, and two of whose runs the old picture holds 100 rows down,
    /// alone.
    #[test]
    fn an_offset_that_grows_no_copy_is_looked_past() {
        let (w, h) = (256u16, 512u16);
        let line = usize::from(w) * 4;
        let mut old = noise(w, h, 35);
        let mut new = noise(w, h, 36);
        for y in 0..usize::from(h) - 24 {
            new[y * line + 64 * 4..(y + 1) * line]
                .copy_from_slice(&old[(y + 24) * line + 64 * 4..(y + 25) * line]);
        }
        for top in [0, 64] {
            let at = |y: usize| y * line + 24 * 4;
            old[at(top + 100)..at(top + 100) + 16 * 4]
                .copy_from_slice(&new[at(top)..at(top) + 16 * 4]);
        }
        let mut encoder = TilesEncoder::new(w, h, progressive(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(&mut encoder, &mut page, &old, None, ms(0));
        let now = settle(&mut encoder, &mut page, ms(0)) + ms(100);
        step(
            &mut encoder,
            &mut page,
            &new,
            Some(&[Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }]),
            now,
        );
        assert_eq!(page.copies, 1, "the scroll a copy");
    }

    /// Beside a region's stream, a scroll's copy neither draws on the tiles
    /// the stream carries nor reads them, and the page is lossless outside it.
    #[test]
    fn a_copy_keeps_off_a_stream() {
        let (w, h) = (512u16, 256u16);
        let document = noise(256, 3 * h, 26);
        let mut pixels = noise(w, h, 27);
        let page_at = |pixels: &mut [u8], top: usize| {
            for row in 0..usize::from(h) {
                let to = (row * usize::from(w) + 256) * 4;
                pixels[to..to + 256 * 4]
                    .copy_from_slice(&document[(top + row) * 256 * 4..(top + row + 1) * 256 * 4]);
            }
        };
        page_at(&mut pixels, 0);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &pixels);
        let video = Rect {
            x: 0,
            y: 0,
            width: 256,
            height: 192,
        };
        let outside = |page: &Page, pixels: &[u8]| {
            (0..usize::from(h))
                .flat_map(|y| (0..usize::from(w)).map(move |x| (x, y)))
                .filter(|(x, y)| *x >= 256 || *y >= 192)
                .filter(|(x, y)| {
                    page.canvas[(y * usize::from(w) + x) * 4..][..3]
                        != pixels[(y * usize::from(w) + x) * 4..][..3]
                })
                .count()
        };
        for n in 1..=20u32 {
            paint(&mut pixels, w, video, 600 + n);
            let mut damage = vec![video];
            if n > 12 {
                page_at(&mut pixels, (n as usize - 12) * 24);
                damage.push(Rect {
                    x: 256,
                    y: 0,
                    width: 256,
                    height: h,
                });
            }
            step(
                &mut encoder,
                &mut page,
                &pixels,
                Some(&damage),
                t0 + ms(u64::from(n) * 50),
            );
            assert_eq!(
                outside(&page, &pixels),
                0,
                "the page lossless outside the stream at frame {n}"
            );
        }
        assert_eq!(
            page.starts, 2,
            "the video streams, after the first picture's frame"
        );
        assert!(
            page.copies >= 8,
            "the page beside it scrolls by copies: {}",
            page.copies
        );
    }

    /// A copy draws what the client holds of its source, so where that is lossy
    /// — the tiles a stream carried, owed their still — every tile the copy
    /// draws on is owed it too, one that did not change included, and the
    /// settle leaves the page lossless.
    #[test]
    fn a_copy_from_a_lossy_source_owes_every_tile_it_draws() {
        let (w, h) = (256u16, 320u16);
        // Noise, but for 192 rows of grey from row 64 of the document.
        let mut document = noise(w, 3 * h, 30);
        let line = usize::from(w) * 4;
        for row in 64..256 {
            document[row * line..(row + 1) * line]
                .copy_from_slice(&[0x81, 0x81, 0x81, 0].repeat(usize::from(w)));
        }
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        step(
            &mut encoder,
            &mut page,
            &scrolled(&document, w, 0, h),
            None,
            ms(0),
        );
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
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }]),
            ms(100),
        );
        assert_eq!(page.copies, 1);
        assert!(
            (0..4).all(|col| encoder.debts.contains_key(&(col, 1))),
            "the tiles the copy drew that did not change are owed"
        );
        settle(&mut encoder, &mut page, ms(100));
        assert_eq!(page.off(&pixels), 0, "the page lossless once settled");
    }

    /// A picture that changed without anything in it moving finds no copy.
    #[test]
    fn a_new_picture_is_no_copy() {
        let (w, h) = (256u16, 256u16);
        let mut encoder = TilesEncoder::new(w, h, held(), Fake::coder()).unwrap();
        let mut page = Page::new(w, h);
        let t0 = begin(&mut encoder, &mut page, &noise(w, h, 28));
        let pixels = noise(w, h, 29);
        step(
            &mut encoder,
            &mut page,
            &pixels,
            Some(&[Rect {
                x: 0,
                y: 0,
                width: w,
                height: h,
            }]),
            t0 + ms(100),
        );
        assert_eq!(page.copies, 0);
        settle(&mut encoder, &mut page, t0 + ms(100));
        assert_eq!(page.off(&pixels), 0);
    }
}
