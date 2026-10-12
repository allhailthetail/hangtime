//! Audio pipeline: device capture and playback, Opus encode and decode.
//!
//! Audio callbacks never allocate, lock, block, or log. They only move
//! samples through a lock-free ring buffer; all processing runs on a
//! dedicated audio thread. Internal format is 48 kHz mono f32.

use std::fmt;

mod codec;
mod denoise;
mod device;
mod jitter;
mod pipeline;
mod probe;
mod resample;
mod tones;
mod vox;

pub use codec::{MAX_PACKET, VoiceDecoder, VoiceEncoder};
pub use device::{Capture, DeviceInfo, DeviceLatency, Playback, list_devices};
pub use hangtime_proto::FRAME_DURATION;
pub use jitter::{JitterBuffer, Playout};
pub use pipeline::{Inbox, PACKETIZATION_DELAY, ReceiveStats, Receiver, Transmitter};
pub use probe::{FrameReport, VoxProbe};
pub use resample::Resampler;
pub use tones::Tone;
pub use vox::{
    VOX_SENSITIVITY_DEFAULT, VOX_SENSITIVITY_MAX, VOX_SENSITIVITY_MIN, VoxEvent, level_db,
    min_level_db,
};

/// Internal sample rate.
pub const SAMPLE_RATE: u32 = 48_000;

/// Samples per 20 ms frame at [`SAMPLE_RATE`].
pub const FRAME_SAMPLES: usize = 960;

#[derive(Debug)]
pub enum AudioError {
    /// No input or output device found.
    NoDevice,
    /// Devices exist but none could be opened; lists each failure.
    NoUsableDevice(String),
    /// The device only offers a sample format we don't handle.
    UnsupportedFormat(cpal::SampleFormat),
    Device(cpal::Error),
    Codec(opus::Error),
    Thread(std::io::Error),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => write!(f, "no audio device found"),
            Self::NoUsableDevice(failures) => {
                write!(f, "no audio device could be opened: {failures}")
            }
            Self::UnsupportedFormat(format) => write!(f, "unsupported sample format {format}"),
            Self::Device(e) => write!(f, "audio device: {e}"),
            Self::Codec(e) => write!(f, "opus: {e}"),
            Self::Thread(e) => write!(f, "could not start audio thread: {e}"),
        }
    }
}

impl std::error::Error for AudioError {}

impl From<cpal::Error> for AudioError {
    fn from(e: cpal::Error) -> Self {
        Self::Device(e)
    }
}

impl From<opus::Error> for AudioError {
    fn from(e: opus::Error) -> Self {
        Self::Codec(e)
    }
}
