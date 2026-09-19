//! The desktop's sound, between the session that decodes it and the platform's
//! audio device that plays it.
//!
//! The session asks for one format — signed 16-bit stereo at 48 kHz, what a
//! Mac's output runs at and a Windows output mixes at by default, and one
//! either's audio engine converts from when a device runs at another — and
//! decodes every FLAC frame into [`Playback`] as it arrives. The device takes
//! from it on its own clock, in whatever sizes it likes, and neither waits for
//! the other: the lock is held for one copy.
//!
//! The two clocks are not the same clock, and the network between them is not
//! smooth, so the buffer has a floor and a ceiling. It starts playing only
//! once [`START`] is waiting, which absorbs the jitter of frames arriving in
//! bursts behind a big framebuffer update; running dry puts it back to waiting
//! for that much, rather than playing each frame the instant it lands and
//! stuttering on every one. Past [`CEILING`] the oldest sound is thrown away,
//! down to [`START`], so a stall that is followed by a burst costs a skip and
//! not a permanent delay.

use std::collections::VecDeque;

use wlshare_rfb::audio::{AudioFormat, SampleFormat};

/// What the session asks the server for.
pub const FORMAT: AudioFormat = AudioFormat { sample: SampleFormat::S16, channels: 2, frequency: 48_000 };

/// How much sound is waiting before any is played, in frames: 60 ms, three
/// FLAC frames.
pub const START: usize = FORMAT.frequency as usize * 60 / 1000;
/// The most sound that may wait, in frames: 300 ms. Past this the delay is the
/// listener's problem, and the oldest goes.
pub const CEILING: usize = FORMAT.frequency as usize * 300 / 1000;

/// The sound waiting to be played, as stereo frames the device takes directly.
#[derive(Debug, Default)]
pub struct Playback {
    frames: VecDeque<[f32; 2]>,
    /// Whether [`START`] has been reached since the buffer last ran dry.
    playing: bool,
}

impl Playback {
    /// Samples decoded in [`FORMAT`]: interleaved, little-endian, signed 16-bit
    /// stereo.
    pub fn push(&mut self, samples: &[u8]) {
        const SCALE: f32 = 1.0 / 32768.0;
        self.frames.extend(samples.as_chunks::<4>().0.iter().map(|frame| {
            let left = i16::from_le_bytes([frame[0], frame[1]]);
            let right = i16::from_le_bytes([frame[2], frame[3]]);
            [f32::from(left) * SCALE, f32::from(right) * SCALE]
        }));
        if self.frames.len() > CEILING {
            let over = self.frames.len() - START;
            log::debug!("{} ms of sound waiting; the oldest {over} frames are dropped", self.frames.len() * 1000 / FORMAT.frequency as usize);
            self.frames.drain(..over);
        }
        if self.frames.len() >= START {
            self.playing = true;
        }
    }

    /// Fill `left` and `right`, which are the same length, with what is next
    /// to play — silence while the buffer is filling, and for whatever it
    /// could not cover when it ran dry.
    pub fn read(&mut self, left: &mut [f32], right: &mut [f32]) {
        debug_assert_eq!(left.len(), right.len());
        let mut played = 0;
        if self.playing {
            let available = left.len().min(self.frames.len());
            for ((l, r), frame) in left.iter_mut().zip(right.iter_mut()).zip(self.frames.drain(..available)) {
                *l = frame[0];
                *r = frame[1];
                played += 1;
            }
            if played < left.len() {
                self.playing = false;
            }
        }
        left[played..].fill(0.0);
        right[played..].fill(0.0);
    }

    /// How much sound is waiting, in frames.
    pub fn waiting(&self) -> usize {
        self.frames.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `frames` of stereo where the left channel counts up from `from` and the
    /// right is its negation, so what comes out says exactly which frame it is.
    fn counting(from: i16, frames: usize) -> Vec<u8> {
        (0..frames as i16).flat_map(|n| [(from + n).to_le_bytes(), (-(from + n)).to_le_bytes()]).flatten().collect()
    }

    fn read(playback: &mut Playback, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![9.0; frames], vec![9.0; frames]);
        playback.read(&mut left, &mut right);
        (left, right)
    }

    #[test]
    fn nothing_plays_until_there_is_enough_to_ride_out_a_late_frame() {
        let mut playback = Playback::default();
        playback.push(&counting(1, START - 1));
        assert_eq!(read(&mut playback, 4), (vec![0.0; 4], vec![0.0; 4]), "still filling");
        assert_eq!(playback.waiting(), START - 1, "and none of it was taken");

        playback.push(&counting(START as i16, 1));
        let (left, right) = read(&mut playback, 2);
        assert_eq!(left, vec![1.0 / 32768.0, 2.0 / 32768.0]);
        assert_eq!(right, vec![-1.0 / 32768.0, -2.0 / 32768.0]);
    }

    /// Running dry plays what there was, then silence, and then waits for the
    /// floor again rather than playing each frame as it lands.
    #[test]
    fn running_dry_goes_back_to_filling() {
        let mut playback = Playback::default();
        playback.push(&counting(1, START));
        read(&mut playback, START - 2);
        let (left, _) = read(&mut playback, 4);
        assert_eq!(left, vec![(START - 1) as f32 / 32768.0, START as f32 / 32768.0, 0.0, 0.0]);

        playback.push(&counting(1, 960));
        assert_eq!(read(&mut playback, 4).0, vec![0.0; 4], "one frame is not enough to start again");
        playback.push(&counting(1, START - 960));
        assert_ne!(read(&mut playback, 4).0, vec![0.0; 4]);
    }

    #[test]
    fn a_burst_past_the_ceiling_drops_the_oldest_down_to_the_floor() {
        let mut playback = Playback::default();
        playback.push(&counting(0, CEILING));
        assert_eq!(playback.waiting(), CEILING, "at the ceiling is not past it");
        playback.push(&counting(CEILING as i16, 1));
        assert_eq!(playback.waiting(), START);
        let (left, _) = read(&mut playback, 1);
        assert_eq!(left, vec![(CEILING + 1 - START) as f32 / 32768.0], "what is kept is the newest");
    }

    #[test]
    fn full_scale_stays_inside_the_unit_range() {
        let mut playback = Playback::default();
        let mut samples = Vec::new();
        for _ in 0..START {
            samples.extend(i16::MIN.to_le_bytes());
            samples.extend(i16::MAX.to_le_bytes());
        }
        playback.push(&samples);
        let (left, right) = read(&mut playback, 1);
        assert_eq!(left, vec![-1.0]);
        assert!(right[0] < 1.0 && right[0] > 0.999);
    }
}
