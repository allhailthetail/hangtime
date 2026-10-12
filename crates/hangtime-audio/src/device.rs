//! Audio device capture and playback.
//!
//! The data callbacks only convert samples and move them through a
//! lock-free ring buffer: no allocation, locking, blocking, or logging.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, InputCallbackInfo, OutputCallbackInfo, SampleFormat, SizedSample,
    StreamConfig, SupportedStreamConfig, SupportedStreamConfigRange,
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::{AudioError, SAMPLE_RATE};

/// Ring buffer length in seconds of audio at the device rate.
const RING_SECONDS: f32 = 0.5;

/// Microphone input. Samples arrive as mono f32 at [`Capture::device_rate`].
pub struct Capture {
    _stream: cpal::Stream,
    device_rate: u32,
    failed: Arc<AtomicBool>,
}

impl Capture {
    /// Opens the default input device and starts capturing.
    pub fn open_default() -> Result<(Self, Consumer<f32>), AudioError> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or(AudioError::NoDevice)?;
        let config = pick_config(
            device.supported_input_configs()?,
            device.default_input_config()?,
        );
        let device_rate = config.sample_rate();
        let (producer, consumer) = RingBuffer::new(ring_len(device_rate));
        let failed = Arc::new(AtomicBool::new(false));
        let stream = match config.sample_format() {
            SampleFormat::F32 => build_capture::<f32>(&device, config.config(), producer, &failed)?,
            SampleFormat::I16 => build_capture::<i16>(&device, config.config(), producer, &failed)?,
            SampleFormat::I32 => build_capture::<i32>(&device, config.config(), producer, &failed)?,
            SampleFormat::U16 => build_capture::<u16>(&device, config.config(), producer, &failed)?,
            other => return Err(AudioError::UnsupportedFormat(other)),
        };
        stream.play()?;
        Ok((
            Self {
                _stream: stream,
                device_rate,
                failed,
            },
            consumer,
        ))
    }

    /// Sample rate of the samples in the ring buffer.
    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// True once the device has reported an error.
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

/// Speaker output. Expects mono f32 at [`Playback::device_rate`]; plays
/// silence when the ring buffer is empty.
pub struct Playback {
    _stream: cpal::Stream,
    device_rate: u32,
    failed: Arc<AtomicBool>,
}

impl Playback {
    /// Opens the default output device and starts playing.
    pub fn open_default() -> Result<(Self, Producer<f32>), AudioError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(AudioError::NoDevice)?;
        let config = pick_config(
            device.supported_output_configs()?,
            device.default_output_config()?,
        );
        let device_rate = config.sample_rate();
        let (producer, consumer) = RingBuffer::new(ring_len(device_rate));
        let failed = Arc::new(AtomicBool::new(false));
        let stream = match config.sample_format() {
            SampleFormat::F32 => {
                build_playback::<f32>(&device, config.config(), consumer, &failed)?
            }
            SampleFormat::I16 => {
                build_playback::<i16>(&device, config.config(), consumer, &failed)?
            }
            SampleFormat::I32 => {
                build_playback::<i32>(&device, config.config(), consumer, &failed)?
            }
            SampleFormat::U16 => {
                build_playback::<u16>(&device, config.config(), consumer, &failed)?
            }
            other => return Err(AudioError::UnsupportedFormat(other)),
        };
        stream.play()?;
        Ok((
            Self {
                _stream: stream,
                device_rate,
                failed,
            },
            producer,
        ))
    }

    /// Sample rate the ring buffer is played at.
    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// True once the device has reported an error.
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

fn ring_len(device_rate: u32) -> usize {
    (device_rate as f32 * RING_SECONDS) as usize
}

/// Prefers a 48 kHz config, then f32 samples, then fewer channels. Falls
/// back to the device default (and resampling) if 48 kHz is unsupported.
fn pick_config(
    ranges: impl Iterator<Item = SupportedStreamConfigRange>,
    default: SupportedStreamConfig,
) -> SupportedStreamConfig {
    ranges
        .filter_map(|range| range.try_with_sample_rate(SAMPLE_RATE))
        .filter(|config| {
            matches!(
                config.sample_format(),
                SampleFormat::F32 | SampleFormat::I16 | SampleFormat::I32 | SampleFormat::U16
            )
        })
        .min_by_key(|config| {
            (
                config.sample_format() != SampleFormat::F32,
                config.channels(),
            )
        })
        .unwrap_or(default)
}

fn build_capture<T>(
    device: &Device,
    config: StreamConfig,
    mut producer: Producer<f32>,
    failed: &Arc<AtomicBool>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let scale = 1.0 / channels as f32;
    let failed = Arc::clone(failed);
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &InputCallbackInfo| {
            for frame in data.chunks_exact(channels) {
                let mono = frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() * scale;
                // A full ring means the consumer is behind; drop samples.
                let _ = producer.push(mono);
            }
        },
        move |_| failed.store(true, Ordering::Relaxed),
        None,
    )?;
    Ok(stream)
}

fn build_playback<T>(
    device: &Device,
    config: StreamConfig,
    mut consumer: Consumer<f32>,
    failed: &Arc<AtomicBool>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let failed = Arc::clone(failed);
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &OutputCallbackInfo| {
            for frame in data.chunks_exact_mut(channels) {
                frame.fill(T::from_sample(consumer.pop().unwrap_or(0.0)));
            }
        },
        move |_| failed.store(true, Ordering::Relaxed),
        None,
    )?;
    Ok(stream)
}
