//! Voice-operated transmit: decides when to key and unkey from per-frame
//! speech probabilities.
//!
//! A frame counts as voice when RNNoise's speech probability reaches the
//! threshold. VOX keys only after a run of voiced frames, which rejects
//! short transients such as taps and clicks (a syllable lasts well over
//! 100 ms). Pre-roll covers the audio before keying.

/// Least and most sensitive levels.
pub const VOX_SENSITIVITY_MIN: u8 = 1;
pub const VOX_SENSITIVITY_MAX: u8 = 5;

/// Default level: rejects taps, keys on normal speech.
pub const VOX_SENSITIVITY_DEFAULT: u8 = 3;

/// Quiet frames after speech before unkeying (500 ms), so VOX doesn't
/// drop out between words.
const HANG_FRAMES: u32 = 25;

/// (speech probability threshold, voiced frames in a row to key) for
/// each level, least sensitive first. The longest run (5 frames, 100 ms)
/// stays within the transmitter's pre-roll.
const LEVELS: [(f32, u32); 5] = [(0.95, 5), (0.90, 4), (0.80, 3), (0.65, 3), (0.50, 2)];

/// A VOX decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoxEvent {
    /// Speech started: key up.
    Start,
    /// Speech ended (after the hang): key down.
    Stop,
}

#[derive(Debug)]
pub struct Vox {
    threshold: f32,
    attack_frames: u32,
    active: bool,
    voiced_run: u32,
    quiet_run: u32,
}

impl Default for Vox {
    fn default() -> Self {
        Self::new(VOX_SENSITIVITY_DEFAULT)
    }
}

impl Vox {
    /// `sensitivity` is clamped to 1 (least) through 5 (most).
    pub fn new(sensitivity: u8) -> Self {
        let mut vox = Self {
            threshold: 0.0,
            attack_frames: 0,
            active: false,
            voiced_run: 0,
            quiet_run: 0,
        };
        vox.set_sensitivity(sensitivity);
        vox
    }

    /// Changes the sensitivity without interrupting speech in progress.
    pub fn set_sensitivity(&mut self, sensitivity: u8) {
        let level = sensitivity.clamp(VOX_SENSITIVITY_MIN, VOX_SENSITIVITY_MAX);
        (self.threshold, self.attack_frames) = LEVELS[usize::from(level - VOX_SENSITIVITY_MIN)];
    }

    /// Feeds one 20 ms frame's speech probability.
    pub fn push(&mut self, speech: f32) -> Option<VoxEvent> {
        if speech >= self.threshold {
            self.voiced_run += 1;
            self.quiet_run = 0;
        } else {
            self.voiced_run = 0;
            self.quiet_run += 1;
        }
        if !self.active && self.voiced_run >= self.attack_frames {
            self.active = true;
            return Some(VoxEvent::Start);
        }
        if self.active && self.quiet_run >= HANG_FRAMES {
            self.active = false;
            return Some(VoxEvent::Stop);
        }
        None
    }

    /// Forgets any speech in progress, without an event.
    pub fn reset(&mut self) {
        self.active = false;
        self.voiced_run = 0;
        self.quiet_run = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(vox: &mut Vox, speech: f32, frames: u32) -> Vec<VoxEvent> {
        (0..frames).filter_map(|_| vox.push(speech)).collect()
    }

    #[test]
    fn keys_after_attack_and_unkeys_after_hang() {
        let mut vox = Vox::default();
        assert_eq!(feed(&mut vox, 0.9, 2), []);
        assert_eq!(feed(&mut vox, 0.9, 1), [VoxEvent::Start]);
        assert_eq!(feed(&mut vox, 0.9, 50), []);
        assert_eq!(feed(&mut vox, 0.1, HANG_FRAMES - 1), []);
        assert_eq!(feed(&mut vox, 0.1, 1), [VoxEvent::Stop]);
    }

    #[test]
    fn default_rejects_taps() {
        let mut vox = Vox::default();
        // A tap: a confident spike for 40 ms, then quiet.
        for _ in 0..10 {
            assert_eq!(feed(&mut vox, 0.99, 2), []);
            assert_eq!(feed(&mut vox, 0.1, 5), []);
        }
    }

    #[test]
    fn default_ignores_borderline_frames() {
        let mut vox = Vox::default();
        assert_eq!(feed(&mut vox, 0.7, 50), []);
    }

    #[test]
    fn most_sensitive_keys_quickly() {
        let mut vox = Vox::new(VOX_SENSITIVITY_MAX);
        assert_eq!(feed(&mut vox, 0.55, 2), [VoxEvent::Start]);
    }

    #[test]
    fn least_sensitive_needs_confident_sustained_speech() {
        let mut vox = Vox::new(VOX_SENSITIVITY_MIN);
        assert_eq!(feed(&mut vox, 0.92, 20), []);
        assert_eq!(feed(&mut vox, 0.97, 4), []);
        assert_eq!(feed(&mut vox, 0.97, 1), [VoxEvent::Start]);
    }

    #[test]
    fn clamps_out_of_range_levels() {
        assert_eq!(
            Vox::new(0).threshold,
            Vox::new(VOX_SENSITIVITY_MIN).threshold
        );
        assert_eq!(
            Vox::new(9).threshold,
            Vox::new(VOX_SENSITIVITY_MAX).threshold
        );
    }

    #[test]
    fn short_pause_between_words_keeps_transmitting() {
        let mut vox = Vox::default();
        feed(&mut vox, 0.9, 10);
        assert_eq!(feed(&mut vox, 0.1, 15), []);
        assert_eq!(feed(&mut vox, 0.9, 10), []);
        assert_eq!(feed(&mut vox, 0.1, HANG_FRAMES), [VoxEvent::Stop]);
    }

    #[test]
    fn longest_attack_fits_in_preroll() {
        let longest = LEVELS.iter().map(|&(_, frames)| frames).max().unwrap();
        assert!(longest as usize <= crate::pipeline::PREROLL_FRAMES);
    }
}
