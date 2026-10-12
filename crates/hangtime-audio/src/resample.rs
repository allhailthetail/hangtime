//! Sample rate conversion at the device boundary.
//!
//! Linear interpolation: cheap and good enough for voice while devices
//! almost always run at 48 kHz anyway. It does not low-pass before
//! downsampling, so swap in a windowed-sinc resampler if aliasing is
//! ever audible.

/// Converts a mono stream from one sample rate to another.
pub struct Resampler {
    step: f64,
    pos: f64,
    prev: f32,
}

impl Resampler {
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        Self {
            step: f64::from(from_rate) / f64::from(to_rate),
            pos: 0.0,
            prev: 0.0,
        }
    }

    /// Appends the resampled form of `input` to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.step == 1.0 {
            out.extend_from_slice(input);
            return;
        }
        for &sample in input {
            while self.pos < 1.0 {
                out.push(self.prev + (sample - self.prev) * self.pos as f32);
                self.pos += self.step;
            }
            self.pos -= 1.0;
            self.prev = sample;
        }
    }

    /// Forgets the previous sample, for the start of a new stream.
    pub fn reset(&mut self) {
        self.pos = 0.0;
        self.prev = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_passes_through() {
        let mut out = Vec::new();
        Resampler::new(48_000, 48_000).process(&[0.1, 0.2, 0.3], &mut out);
        assert_eq!(out, [0.1, 0.2, 0.3]);
    }

    #[test]
    fn converts_length_by_rate_ratio() {
        let mut resampler = Resampler::new(44_100, 48_000);
        let mut out = Vec::new();
        resampler.process(&vec![0.0; 44_100], &mut out);
        assert!((47_999..=48_001).contains(&out.len()), "{}", out.len());
    }

    #[test]
    fn interpolates_between_samples() {
        let mut resampler = Resampler::new(1, 2);
        let mut out = Vec::new();
        resampler.process(&[0.0, 1.0], &mut out);
        assert_eq!(out, [0.0, 0.0, 0.0, 0.5]);
    }
}
