//! Radio tones. Audio cues tell the user what is happening without
//! looking at the screen.

use std::f32::consts::TAU;

use crate::SAMPLE_RATE;

/// Peak amplitude of tones, well below full scale.
const LEVEL: f32 = 0.2;

/// Fade at each end of a beep, so it starts and stops without a click.
const FADE_MS: u32 = 5;

/// A tone the radio plays locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The floor is yours: start talking.
    TalkPermit,
    /// The channel is busy or the floor was lost.
    Bonk,
    /// A received transmission ended.
    Courtesy,
    /// Hang time ended; the channel is free.
    HangEnd,
}

impl Tone {
    /// The tone as 48 kHz mono samples.
    pub fn render(self) -> Vec<f32> {
        // (frequency Hz, duration ms); frequency 0 is silence.
        let parts: &[(f32, u32)] = match self {
            Self::TalkPermit => &[(1200.0, 40), (0.0, 30), (1200.0, 40)],
            Self::Bonk => &[(380.0, 250)],
            Self::Courtesy => &[(880.0, 70)],
            Self::HangEnd => &[(600.0, 50)],
        };
        let mut samples = Vec::new();
        for &(frequency, ms) in parts {
            beep(&mut samples, frequency, ms);
        }
        samples
    }
}

fn beep(out: &mut Vec<f32>, frequency: f32, ms: u32) {
    let len = (SAMPLE_RATE * ms / 1000) as usize;
    let fade = (SAMPLE_RATE * FADE_MS / 1000) as usize;
    for i in 0..len {
        if frequency == 0.0 {
            out.push(0.0);
            continue;
        }
        let edge = i.min(len - 1 - i);
        let envelope = (edge as f32 / fade as f32).min(1.0);
        let phase = i as f32 * frequency / SAMPLE_RATE as f32;
        out.push((phase * TAU).sin() * LEVEL * envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_have_expected_length_and_level() {
        let permit = Tone::TalkPermit.render();
        assert_eq!(permit.len(), (SAMPLE_RATE * 110 / 1000) as usize);
        for tone in [Tone::TalkPermit, Tone::Bonk, Tone::Courtesy, Tone::HangEnd] {
            let samples = tone.render();
            let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.1 && peak <= LEVEL, "{tone:?} peak {peak}");
        }
    }

    #[test]
    fn beeps_start_and_end_silent() {
        let bonk = Tone::Bonk.render();
        assert!(bonk[0].abs() < 1e-6);
        assert!(bonk[bonk.len() - 1].abs() < 0.01);
    }
}
