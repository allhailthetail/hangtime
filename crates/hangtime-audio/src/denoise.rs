//! Noise suppression with RNNoise (`nnnoiseless`), which also estimates
//! how likely each frame is to be speech. VOX uses that estimate.

use nnnoiseless::DenoiseState;

use crate::FRAME_SAMPLES;

/// RNNoise works on 10 ms frames; ours are 20 ms.
const HALF: usize = DenoiseState::FRAME_SIZE;
const _: () = assert!(HALF * 2 == FRAME_SAMPLES);

/// RNNoise expects 16-bit sample range.
const SCALE: f32 = 32768.0;

pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    input: [f32; HALF],
    output: [f32; HALF],
}

impl Default for Denoiser {
    fn default() -> Self {
        Self {
            state: DenoiseState::new(),
            input: [0.0; HALF],
            output: [0.0; HALF],
        }
    }
}

impl Denoiser {
    /// Denoises one 20 ms frame in place. Returns the probability (0 to 1)
    /// that the frame contains speech.
    pub fn process(&mut self, frame: &mut [f32; FRAME_SAMPLES]) -> f32 {
        let mut speech = 0.0f32;
        for half in frame.chunks_exact_mut(HALF) {
            for (scaled, sample) in self.input.iter_mut().zip(half.iter()) {
                *scaled = sample * SCALE;
            }
            speech = speech.max(self.state.process_frame(&mut self.output, &self.input));
            for (sample, cleaned) in half.iter_mut().zip(self.output.iter()) {
                *sample = cleaned / SCALE;
            }
        }
        speech
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_noise_scores_low_and_is_reduced() {
        let mut denoiser = Denoiser::default();
        // Deterministic white-ish noise.
        let mut seed = 1u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        let mut energy_in = 0.0;
        let mut energy_out = 0.0;
        let mut speech = 0.0f32;
        for i in 0..100 {
            let mut frame: [f32; FRAME_SAMPLES] = std::array::from_fn(|_| noise() * 0.1);
            let before: f32 = frame.iter().map(|s| s * s).sum();
            let probability = denoiser.process(&mut frame);
            // Skip the first frames while RNNoise adapts.
            if i >= 50 {
                energy_in += before;
                energy_out += frame.iter().map(|s| s * s).sum::<f32>();
                speech = speech.max(probability);
            }
        }
        assert!(
            energy_out < energy_in / 2.0,
            "in {energy_in} out {energy_out}"
        );
        assert!(speech < 0.5, "noise scored as speech: {speech}");
    }
}

/// Runs a synthetic table tap through real RNNoise and VOX. RNNoise's
/// speech estimate jumps on the tap and lingers for about 200 ms after
/// it; VOX must not key on it at any sensitivity.
#[cfg(test)]
mod tap_tests {
    use super::*;
    use crate::vox::{VOX_SENSITIVITY_MAX, VOX_SENSITIVITY_MIN, Vox, level_db};

    #[test]
    fn table_tap_does_not_key_vox() {
        for sensitivity in VOX_SENSITIVITY_MIN..=VOX_SENSITIVITY_MAX {
            let mut denoiser = Denoiser::default();
            let mut vox = Vox::new(sensitivity);
            let mut seed = 7u32;
            let mut noise = || {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
            };
            // Two seconds of quiet room with a tap every half second: a
            // sharp, decaying knock about 15 ms long.
            for i in 0..100 {
                let tap_frame = i >= 25 && i % 25 == 0;
                let mut frame: [f32; FRAME_SAMPLES] = std::array::from_fn(|n| {
                    let room = noise() * 0.003;
                    let t = n as f32 / 48_000.0;
                    let tap = if tap_frame {
                        (t * 900.0 * std::f32::consts::TAU).sin() * 0.6 * (-t / 0.004).exp()
                            + noise() * 0.5 * (-t / 0.002).exp()
                    } else {
                        0.0
                    };
                    room + tap
                });
                let level = level_db(&frame);
                let speech = denoiser.process(&mut frame);
                assert_eq!(
                    vox.push(speech, level),
                    None,
                    "keyed at level {sensitivity}"
                );
            }
        }
    }
}
