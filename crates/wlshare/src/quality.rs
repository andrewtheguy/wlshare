//! The VP9 quality a session's link will bear, on the 1–100 dial.
//!
//! The configured `vp9_quality` is a ceiling and `vp9_quality_min` a floor;
//! between them the dial walks down when the client falls behind and back up
//! when it keeps up. The signal is how long a frame takes to be delivered and
//! decoded — the round trip of the fence that follows it, or, for a client
//! without Fence, how long writing it blocked — less the link's own floor, the
//! shortest delivery seen lately, so a distant link that keeps up reads as
//! keeping up. What is left is queueing: time spent behind frames the link or
//! the client could not take as fast as they came.
//!
//! One-directional by construction: the walk never goes above the ceiling,
//! since a link with room to spare shows no more of it than one that is merely
//! keeping up, and a finer picture than the operator asked for was never the
//! goal. Quick to give quality up, slow to take it back, so a link that is
//! intermittently bad settles at a quality it can hold rather than oscillating
//! around one it cannot. The same walk remotex runs for its own VP9 streams.
//! The one exception is [`QualityWalk::settle`], the ceiling taken back at
//! once for a desktop that went quiet coarse, which the session decides.
//!
//! Pure, and takes `now` rather than reading a clock, so every decision is
//! testable without waiting for one.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Queueing past which a frame counts as one the link could not keep up with.
const LAG_BEHIND: Duration = Duration::from_millis(60);

/// Queueing below which a frame counts as clear. Between this and
/// [`LAG_BEHIND`] is hysteresis: a link hovering there earns neither a coarser
/// picture nor its quality back.
const LAG_CLEAR: Duration = Duration::from_millis(30);

/// Consecutive behind frames before quality is given up. Two rather than one,
/// so a single unlucky frame is not a verdict about the link.
const BEHIND_FRAMES: u32 = 2;

/// Consecutive clear frames before quality is taken back, deliberately far
/// more than [`BEHIND_FRAMES`].
const CLEAR_FRAMES: u32 = 30;

/// The least time between two moves of the dial, so a burst of slow frames is
/// one decision rather than one per frame.
const ADJUST_COOLDOWN: Duration = Duration::from_secs(1);

/// How far one step down moves the dial, and one step up. Bigger down than up,
/// for the same reason [`CLEAR_FRAMES`] is bigger than [`BEHIND_FRAMES`].
const STEP_DOWN: u8 = 10;
const STEP_UP: u8 = 3;

/// Deliveries whose minimum is the link's floor. A window rather than an
/// all-time minimum so that a route change is eventually believed.
const BASELINE_WINDOW: usize = 32;

pub struct QualityWalk {
    /// The configured quality: the finest this ever asks for.
    ceiling: u8,
    /// The coarsest it may go.
    floor: u8,
    /// The quality in force.
    quality: u8,
    /// The latest delivery times, fenced ones only, for the link's floor.
    recent: VecDeque<Duration>,
    /// Consecutive behind frames, and consecutive clear ones. Only one is ever
    /// non-zero.
    behind: u32,
    clear: u32,
    /// When the dial last moved, for [`ADJUST_COOLDOWN`].
    changed_at: Option<Instant>,
}

impl QualityWalk {
    /// A walk that starts at `ceiling` and never goes below `floor`, which is
    /// at most the ceiling.
    pub fn new(ceiling: u8, floor: u8) -> Self {
        debug_assert!(floor <= ceiling, "the configuration refuses a floor above the ceiling");
        Self { ceiling, floor: floor.min(ceiling), quality: ceiling, recent: VecDeque::new(), behind: 0, clear: 0, changed_at: None }
    }

    /// The quality in force.
    pub fn quality(&self) -> u8 {
        self.quality
    }

    /// A frame's fence came back `delivery` after the frame was written.
    /// Every delivery counts towards the link's floor; only a delta frame's is
    /// a verdict, since a keyframe is the whole picture again and slow by its
    /// nature. Returns the new quality if the dial moves.
    pub fn fenced(&mut self, delivery: Duration, keyframe: bool, now: Instant) -> Option<u8> {
        if self.recent.len() == BASELINE_WINDOW {
            self.recent.pop_front();
        }
        self.recent.push_back(delivery);
        if keyframe {
            return None;
        }
        let floor = self.recent.iter().min().copied().unwrap_or_default();
        self.observe(delivery.saturating_sub(floor), now)
    }

    /// Take the ceiling back outright, for a desktop that went quiet once the
    /// client had the last frame: an idle link is the clearest evidence of
    /// room it will ever get, and one frame at the ceiling is what sharpens
    /// everything a coarser quality left behind. The cooldown restarts from
    /// here, like any other move. Returns the new quality if the dial moves.
    pub fn settle(&mut self, now: Instant) -> Option<u8> {
        self.behind = 0;
        self.clear = 0;
        if self.quality == self.ceiling {
            return None;
        }
        self.quality = self.ceiling;
        self.changed_at = Some(now);
        Some(self.ceiling)
    }

    /// Whether `quality` is below the ceiling: a frame encoded there is one a
    /// quiet desktop owes a settle for.
    pub fn coarse(&self, quality: u8) -> bool {
        quality < self.ceiling
    }

    /// A delta frame, for a client without Fence, took `blocked` to write:
    /// time the socket had no room for it, which is queueing already.
    pub fn written(&mut self, blocked: Duration, now: Instant) -> Option<u8> {
        self.observe(blocked, now)
    }

    fn observe(&mut self, lag: Duration, now: Instant) -> Option<u8> {
        if lag >= LAG_BEHIND {
            self.behind += 1;
            self.clear = 0;
        } else if lag <= LAG_CLEAR {
            self.clear += 1;
            self.behind = 0;
        } else {
            self.behind = 0;
            self.clear = 0;
        }
        if self.changed_at.is_some_and(|at| now.saturating_duration_since(at) < ADJUST_COOLDOWN) {
            return None;
        }
        let wanted = if self.behind >= BEHIND_FRAMES {
            self.quality.saturating_sub(STEP_DOWN).max(self.floor)
        } else if self.clear >= CLEAR_FRAMES {
            self.quality.saturating_add(STEP_UP).min(self.ceiling)
        } else {
            return None;
        };
        if wanted == self.quality {
            return None;
        }
        self.quality = wanted;
        self.behind = 0;
        self.clear = 0;
        self.changed_at = Some(now);
        Some(wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// A walk that has learnt a 40 ms link floor from one clear delivery.
    fn walk(ceiling: u8, floor: u8, start: Instant) -> QualityWalk {
        let mut walk = QualityWalk::new(ceiling, floor);
        assert_eq!(walk.fenced(40 * MS, false, start), None);
        walk
    }

    #[test]
    fn a_link_that_keeps_up_stays_at_the_ceiling_however_far_away() {
        let start = Instant::now();
        let mut walk = walk(60, 20, start);
        for i in 0..200 {
            assert_eq!(walk.fenced(45 * MS, false, start + i * 10 * MS), None);
        }
        assert_eq!(walk.quality(), 60);
    }

    #[test]
    fn falling_behind_gives_quality_up_down_to_the_floor_once_a_second() {
        let start = Instant::now();
        let mut walk = walk(60, 35, start);
        assert_eq!(walk.fenced(140 * MS, false, start), None, "one slow frame is not a verdict");
        assert_eq!(walk.fenced(140 * MS, false, start), Some(50));
        for _ in 0..4 {
            assert_eq!(walk.fenced(140 * MS, false, start + 500 * MS), None, "inside the cooldown");
        }
        assert_eq!(walk.fenced(140 * MS, false, start + 1100 * MS), Some(40));
        walk.fenced(140 * MS, false, start + 2200 * MS);
        assert_eq!(walk.fenced(140 * MS, false, start + 2200 * MS), Some(35), "stops at the floor");
        walk.fenced(140 * MS, false, start + 3300 * MS);
        assert_eq!(walk.fenced(140 * MS, false, start + 3300 * MS), None);
        assert_eq!(walk.quality(), 35);
    }

    #[test]
    fn quality_comes_back_slowly_and_never_past_the_ceiling() {
        let start = Instant::now();
        let mut walk = walk(60, 20, start);
        walk.fenced(140 * MS, false, start);
        assert_eq!(walk.fenced(140 * MS, false, start), Some(50));
        let mut at = start + 2 * ADJUST_COOLDOWN;
        let mut seen = Vec::new();
        for _ in 0..(CLEAR_FRAMES * 10) {
            at += 50 * MS;
            if let Some(quality) = walk.fenced(40 * MS, false, at) {
                seen.push(quality);
            }
        }
        assert_eq!(seen, [53, 56, 59, 60]);
    }

    #[test]
    fn a_keyframe_is_never_a_verdict_but_can_teach_the_floor() {
        let start = Instant::now();
        let mut walk = QualityWalk::new(60, 20);
        for _ in 0..5 {
            assert_eq!(walk.fenced(400 * MS, true, start), None);
        }
        assert_eq!(walk.quality(), 60);
        // The floor is the smallest delivery, whichever frame it was.
        walk.fenced(10 * MS, true, start);
        walk.fenced(100 * MS, false, start);
        assert_eq!(walk.fenced(100 * MS, false, start), Some(50));
    }

    #[test]
    fn the_hysteresis_band_neither_gives_up_nor_takes_back() {
        let start = Instant::now();
        let mut walk = walk(60, 20, start);
        walk.fenced(140 * MS, false, start);
        walk.fenced(140 * MS, false, start);
        // Inside one baseline window: longer, and 85 ms would be the floor.
        for i in 0..(BASELINE_WINDOW as u32 - 4) {
            assert_eq!(walk.fenced(85 * MS, false, start + ADJUST_COOLDOWN + i * 50 * MS), None);
        }
        assert_eq!(walk.quality(), 50);
    }

    #[test]
    fn settling_takes_the_ceiling_back_and_restarts_the_cooldown() {
        let start = Instant::now();
        let mut walk = walk(60, 20, start);
        walk.fenced(140 * MS, false, start);
        assert_eq!(walk.fenced(140 * MS, false, start), Some(50));
        assert!(walk.coarse(walk.quality()));
        assert_eq!(walk.settle(start + 200 * MS), Some(60), "not held by the cooldown");
        assert!(!walk.coarse(walk.quality()));
        assert_eq!(walk.settle(start + 300 * MS), None, "already there");
        walk.fenced(140 * MS, false, start + 400 * MS);
        assert_eq!(walk.fenced(140 * MS, false, start + 400 * MS), None, "the settle was a move");
        assert_eq!(walk.fenced(140 * MS, false, start + 1300 * MS), Some(50));
    }

    #[test]
    fn a_blocked_write_is_lag_without_a_floor() {
        let start = Instant::now();
        let mut walk = QualityWalk::new(60, 20);
        assert_eq!(walk.written(80 * MS, start), None);
        assert_eq!(walk.written(80 * MS, start), Some(50));
    }

    #[test]
    fn the_floor_forgets_a_route_that_is_gone() {
        let start = Instant::now();
        let mut walk = walk(60, 20, start);
        // A new route, 200 ms further: slow at first, then its own floor.
        for i in 0..(BASELINE_WINDOW as u32) {
            walk.fenced(240 * MS, false, start + i * 10 * ADJUST_COOLDOWN);
        }
        assert_eq!(walk.quality(), 20, "the old floor made the new route read as queueing");
        // Once the old floor has left the window, the new route is a link that
        // keeps up, and quality comes back.
        for i in 0..(CLEAR_FRAMES * 2) {
            walk.fenced(240 * MS, false, start + 400 * ADJUST_COOLDOWN + i * MS);
        }
        assert_eq!(walk.quality(), 23);
    }
}
