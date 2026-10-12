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
