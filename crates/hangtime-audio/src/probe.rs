//! VOX diagnostics: runs audio through the same noise suppression and VOX
//! decisions as the transmitter, frame by frame, and reports what it saw.

use crate::denoise::Denoiser;
use crate::vox::{Vox, VoxEvent, level_db};
use crate::{FRAME_SAMPLES, SAMPLE_RATE};

/// What VOX saw in one 20 ms frame.
#[derive(Debug, Clone, Copy)]
pub struct FrameReport {
    /// Start of the frame, in seconds from the beginning.
    pub time: f32,
    /// Level before noise suppression, dBFS.
    pub level_db: f32,
    /// VOX's estimate of the room's noise floor, dBFS.
    pub floor_db: f32,
    /// RNNoise's speech probability.
    pub speech: f32,
    /// What VOX decided on this frame.
    pub event: Option<VoxEvent>,
}

/// Noise suppression plus VOX, fed 48 kHz mono audio.
pub struct VoxProbe {
    denoiser: Denoiser,
    vox: Vox,
    pending: Vec<f32>,
    frames: u32,
}

impl VoxProbe {
    pub fn new(sensitivity: u8) -> Self {
        Self {
            denoiser: Denoiser::default(),
            vox: Vox::new(sensitivity),
            pending: Vec::new(),
            frames: 0,
        }
    }

    /// Feeds 48 kHz mono samples; returns a report per completed frame.
    pub fn push(&mut self, samples: &[f32]) -> Vec<FrameReport> {
        self.pending.extend_from_slice(samples);
        let mut reports = Vec::new();
        let whole = self.pending.len() - self.pending.len() % FRAME_SAMPLES;
        for chunk in self.pending[..whole].chunks_exact(FRAME_SAMPLES) {
            let mut frame: [f32; FRAME_SAMPLES] = chunk.try_into().expect("exact chunk");
            let level = level_db(&frame);
            let speech = self.denoiser.process(&mut frame);
            let event = self.vox.push(speech, level);
            reports.push(FrameReport {
                time: self.frames as f32 * FRAME_SAMPLES as f32 / SAMPLE_RATE as f32,
                level_db: level,
                floor_db: self.vox.floor_db().unwrap_or(level),
                speech,
                event,
            });
            self.frames += 1;
        }
        self.pending.drain(..whole);
        reports
    }
}

/// Analyses a mic-test recording. Run by hand:
/// `HANGTIME_WAV=path cargo test -p hangtime-audio analyze_wav -- --ignored --nocapture`
#[cfg(test)]
mod wav_analysis {
    use super::*;
    use crate::Resampler;

    fn read_wav(path: &str) -> (u32, Vec<f32>) {
        let bytes = std::fs::read(path).unwrap();
        let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let samples = bytes[44..]
            .chunks_exact(2)
            .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
            .collect();
        (rate, samples)
    }

    /// Normalised autocorrelation peak over voice pitch lags (70-400 Hz).
    fn pitch_strength(window: &[f32]) -> f32 {
        let energy: f32 = window.iter().map(|s| s * s).sum();
        if energy < 1e-9 {
            return 0.0;
        }
        (120..686)
            .map(|lag| {
                let cross: f32 = window[lag..].iter().zip(window).map(|(a, b)| a * b).sum();
                let e1: f32 = window[lag..].iter().map(|s| s * s).sum();
                let e2: f32 = window[..window.len() - lag].iter().map(|s| s * s).sum();
                cross / (e1 * e2).sqrt().max(1e-12)
            })
            .fold(0.0, f32::max)
    }

    #[test]
    #[ignore = "analyses a recording named by HANGTIME_WAV"]
    fn analyze_wav() {
        let path = std::env::var("HANGTIME_WAV").expect("set HANGTIME_WAV");
        let (rate, raw) = read_wav(&path);
        let mut audio = Vec::new();
        Resampler::new(rate, SAMPLE_RATE).process(&raw, &mut audio);
        let mut probe = VoxProbe::new(crate::vox::VOX_SENSITIVITY_DEFAULT);
        let reports = probe.push(&audio);
        println!("  time   level  crest  pitch  speech");
        let window = 12_000; // 0.25 s
        for (i, chunk) in audio.chunks_exact(window).enumerate() {
            let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / window as f32).sqrt();
            let peak = chunk.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            // Pitch on the loudest 40 ms in the window.
            let loudest = chunk
                .chunks_exact(1920)
                .max_by(|a, b| {
                    let ea: f32 = a.iter().map(|s| s * s).sum();
                    let eb: f32 = b.iter().map(|s| s * s).sum();
                    ea.total_cmp(&eb)
                })
                .unwrap();
            let frames = &reports[i * 12..(i * 12 + 12).min(reports.len())];
            let speech = frames.iter().map(|r| r.speech).fold(0.0, f32::max);
            println!(
                "  {:4.2}  {:5.1}  {:5.1}  {:5.2}  {:5.2}",
                i as f32 * 0.25,
                20.0 * rms.max(1e-9).log10(),
                peak / rms.max(1e-9),
                pitch_strength(loudest),
                speech
            );
        }
    }
}
