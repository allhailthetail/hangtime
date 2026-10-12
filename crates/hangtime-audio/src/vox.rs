//! Voice-operated transmit: decides when to key and unkey from each
//! frame's speech probability and loudness.
//!
//! A frame counts as voice only when RNNoise's speech probability reaches
//! the threshold *and* the frame is clearly louder than the room's noise
//! floor. The loudness check matters: RNNoise's estimate lingers for
//! 200 ms or more after a tap or click, but the tap itself is loud for a
//! single frame, while speech stays loud. VOX keys only after a run of
//! voice frames; pre-roll covers the audio before keying.

/// Least and most sensitive levels.
pub const VOX_SENSITIVITY_MIN: u8 = 1;
pub const VOX_SENSITIVITY_MAX: u8 = 5;

/// Default level: rejects taps, keys on normal speech.
pub const VOX_SENSITIVITY_DEFAULT: u8 = 3;

/// Quiet frames after speech before unkeying (500 ms), so VOX doesn't
/// drop out between words.
const HANG_FRAMES: u32 = 25;

/// How fast the noise floor estimate rises, in dB per frame (2.5 dB/s).
/// It drops at once to any quieter frame, so pauses in speech keep it
/// near the true room level while a new steady noise (a fan) is learned
/// within seconds.
const FLOOR_RISE_DB: f32 = 0.05;

/// Sensitivity levels, least sensitive first.
struct Level {
    /// Speech probability a frame needs.
    threshold: f32,
    /// Voice frames in a row needed to key. The longest (5 frames,
    /// 100 ms) stays within the transmitter's pre-roll.
    attack_frames: u32,
    /// How far above the noise floor a frame must be, in dB.
    margin_db: f32,
}

const LEVELS: [Level; 5] = [
    Level {
        threshold: 0.90,
        attack_frames: 5,
        margin_db: 15.0,
    },
    Level {
        threshold: 0.85,
        attack_frames: 4,
        margin_db: 12.0,
    },
    Level {
        threshold: 0.75,
        attack_frames: 3,
        margin_db: 10.0,
    },
    Level {
        threshold: 0.60,
        attack_frames: 3,
        margin_db: 8.0,
    },
    Level {
        threshold: 0.50,
        attack_frames: 2,
        margin_db: 6.0,
    },
];

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
    level: usize,
    /// Estimated room noise level in dBFS; `None` until the first frame.
    floor_db: Option<f32>,
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
            level: 0,
            floor_db: None,
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
        self.level = usize::from(level - VOX_SENSITIVITY_MIN);
    }

    /// Feeds one 20 ms frame: its speech probability and its level in
    /// dBFS before noise suppression.
    pub fn push(&mut self, speech: f32, level_db: f32) -> Option<VoxEvent> {
        let level = &LEVELS[self.level];
        let floor = self.floor_db.map_or(level_db, |floor| floor.min(level_db));
        let loud = level_db >= floor + level.margin_db;
        self.floor_db = Some(floor + FLOOR_RISE_DB);

        if speech >= level.threshold && loud {
            self.voiced_run += 1;
            self.quiet_run = 0;
        } else {
            self.voiced_run = 0;
            self.quiet_run += 1;
        }
        if !self.active && self.voiced_run >= level.attack_frames {
            self.active = true;
            return Some(VoxEvent::Start);
        }
        if self.active && self.quiet_run >= HANG_FRAMES {
            self.active = false;
            return Some(VoxEvent::Stop);
        }
        None
    }

    /// The current noise floor estimate in dBFS, once a frame has been
    /// seen.
    pub fn floor_db(&self) -> Option<f32> {
        self.floor_db
    }

    /// Forgets any speech in progress, without an event. Keeps the noise
    /// floor.
    pub fn reset(&mut self) {
        self.active = false;
        self.voiced_run = 0;
        self.quiet_run = 0;
    }
}

/// RMS level of a frame in dBFS.
pub fn level_db(frame: &[f32]) -> f32 {
    let mean_square = frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32;
    10.0 * mean_square.max(1e-12).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: f32 = -60.0;
    const VOICE: f32 = -30.0;

    fn feed(vox: &mut Vox, speech: f32, level_db: f32, frames: u32) -> Vec<VoxEvent> {
        (0..frames)
            .filter_map(|_| vox.push(speech, level_db))
            .collect()
    }

    /// A quiet room for a second, so the floor settles.
    fn settled(sensitivity: u8) -> Vox {
        let mut vox = Vox::new(sensitivity);
        feed(&mut vox, 0.05, ROOM, 50);
        vox
    }

    #[test]
    fn keys_after_attack_and_unkeys_after_hang() {
        let mut vox = settled(VOX_SENSITIVITY_DEFAULT);
        assert_eq!(feed(&mut vox, 0.9, VOICE, 2), []);
        assert_eq!(feed(&mut vox, 0.9, VOICE, 1), [VoxEvent::Start]);
        assert_eq!(feed(&mut vox, 0.9, VOICE, 50), []);
        assert_eq!(feed(&mut vox, 0.1, ROOM, HANG_FRAMES - 1), []);
        assert_eq!(feed(&mut vox, 0.1, ROOM, 1), [VoxEvent::Stop]);
    }

    #[test]
    fn tap_with_lingering_speech_estimate_does_not_key() {
        // Measured on RNNoise: a tap is loud for one frame, but the speech
        // estimate stays up for many frames afterwards.
        for sensitivity in VOX_SENSITIVITY_MIN..=VOX_SENSITIVITY_MAX {
            let mut vox = settled(sensitivity);
            for _ in 0..5 {
                assert_eq!(vox.push(0.95, -17.0), None);
                assert_eq!(feed(&mut vox, 0.9, ROOM, 12), [], "level {sensitivity}");
                feed(&mut vox, 0.05, ROOM, 10);
            }
        }
    }

    #[test]
    fn speech_estimate_alone_does_not_key() {
        let mut vox = settled(VOX_SENSITIVITY_MAX);
        assert_eq!(feed(&mut vox, 0.99, ROOM, 50), []);
    }

    #[test]
    fn loudness_alone_does_not_key() {
        let mut vox = settled(VOX_SENSITIVITY_MAX);
        assert_eq!(feed(&mut vox, 0.1, VOICE, 50), []);
    }

    #[test]
    fn learns_a_new_steady_noise() {
        let mut vox = settled(VOX_SENSITIVITY_DEFAULT);
        // A fan comes on 20 dB above the room. Within a few seconds the
        // floor has caught up, and speech must clear the fan.
        feed(&mut vox, 0.05, -40.0, 500);
        assert_eq!(feed(&mut vox, 0.9, -38.0, 10), []);
        assert_eq!(feed(&mut vox, 0.9, -25.0, 3), [VoxEvent::Start]);
    }

    #[test]
    fn least_sensitive_needs_louder_longer_speech() {
        let mut vox = settled(VOX_SENSITIVITY_MIN);
        assert_eq!(
            feed(&mut vox, 0.95, -48.0, 20),
            [],
            "only 12 dB over the floor"
        );
        assert_eq!(feed(&mut vox, 0.95, VOICE, 4), []);
        assert_eq!(feed(&mut vox, 0.95, VOICE, 1), [VoxEvent::Start]);
    }

    #[test]
    fn short_pause_between_words_keeps_transmitting() {
        let mut vox = settled(VOX_SENSITIVITY_DEFAULT);
        feed(&mut vox, 0.9, VOICE, 10);
        assert_eq!(feed(&mut vox, 0.1, ROOM, 15), []);
        assert_eq!(feed(&mut vox, 0.9, VOICE, 10), []);
        assert_eq!(feed(&mut vox, 0.1, ROOM, HANG_FRAMES), [VoxEvent::Stop]);
    }

    #[test]
    fn clamps_out_of_range_levels() {
        assert_eq!(Vox::new(0).level, 0);
        assert_eq!(Vox::new(9).level, LEVELS.len() - 1);
    }

    #[test]
    fn longest_attack_fits_in_preroll() {
        let longest = LEVELS
            .iter()
            .map(|level| level.attack_frames)
            .max()
            .unwrap();
        assert!(longest as usize <= crate::pipeline::PREROLL_FRAMES);
    }

    #[test]
    fn measures_level() {
        assert!((level_db(&[0.5; 960]) - -6.02).abs() < 0.01);
        assert!(level_db(&[0.0; 960]) < -100.0);
    }
}
