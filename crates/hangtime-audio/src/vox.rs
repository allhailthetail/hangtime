//! Voice-operated transmit: decides when to key and unkey from per-frame
//! speech probabilities.

/// Speech probability at or above which a frame counts as voice.
const THRESHOLD: f32 = 0.5;

/// Voiced frames in a row needed to key (40 ms), so clicks and bumps
/// don't trigger. Pre-roll covers the audio before keying.
const ATTACK_FRAMES: u32 = 2;

/// Quiet frames after speech before unkeying (500 ms), so VOX doesn't
/// drop out between words.
const HANG_FRAMES: u32 = 25;

/// A VOX decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoxEvent {
    /// Speech started: key up.
    Start,
    /// Speech ended (after the hang): key down.
    Stop,
}

#[derive(Debug, Default)]
pub struct Vox {
    active: bool,
    voiced_run: u32,
    quiet_run: u32,
}

impl Vox {
    /// Feeds one 20 ms frame's speech probability.
    pub fn push(&mut self, speech: f32) -> Option<VoxEvent> {
        let voiced = speech >= THRESHOLD;
        if voiced {
            self.voiced_run += 1;
            self.quiet_run = 0;
        } else {
            self.voiced_run = 0;
            self.quiet_run += 1;
        }
        if !self.active && self.voiced_run >= ATTACK_FRAMES {
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
        *self = Self::default();
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
        assert_eq!(feed(&mut vox, 0.9, 1), []);
        assert_eq!(feed(&mut vox, 0.9, 1), [VoxEvent::Start]);
        assert_eq!(feed(&mut vox, 0.9, 50), []);
        assert_eq!(feed(&mut vox, 0.1, HANG_FRAMES - 1), []);
        assert_eq!(feed(&mut vox, 0.1, 1), [VoxEvent::Stop]);
    }

    #[test]
    fn single_voiced_frame_does_not_key() {
        let mut vox = Vox::default();
        for _ in 0..10 {
            assert_eq!(feed(&mut vox, 0.9, 1), []);
            assert_eq!(feed(&mut vox, 0.1, 1), []);
        }
    }

    #[test]
    fn short_pause_between_words_keeps_transmitting() {
        let mut vox = Vox::default();
        feed(&mut vox, 0.9, 10);
        assert_eq!(feed(&mut vox, 0.1, 15), []);
        assert_eq!(feed(&mut vox, 0.9, 10), []);
        assert_eq!(feed(&mut vox, 0.1, HANG_FRAMES), [VoxEvent::Stop]);
    }
}
