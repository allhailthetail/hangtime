//! Audio device capture and playback.
//!
//! The data callbacks only convert samples and move them through a
//! lock-free ring buffer: no allocation, locking, blocking, or logging.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    Device, FromSample, InputCallbackInfo, OutputCallbackInfo, SampleFormat, SizedSample,
    StreamConfig, SupportedStreamConfig, SupportedStreamConfigRange,
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::{AudioError, SAMPLE_RATE};

/// Ring buffer length in seconds of audio at the device rate.
const RING_SECONDS: f32 = 0.5;

/// State a stream's callbacks report to other threads through atomics.
#[derive(Default)]
struct StreamState {
    failed: AtomicBool,
    /// Device latency from the last callback, in microseconds.
    latency_us: AtomicU32,
}

/// Latest device latency reported by a stream's callback. Cheap to clone
/// and safe to read from any thread.
#[derive(Clone)]
pub struct DeviceLatency(Arc<StreamState>);

impl DeviceLatency {
    /// Time between sound at the device and the callback (capture), or
    /// between the callback and sound at the device (playback).
    pub fn get(&self) -> Duration {
        Duration::from_micros(u64::from(self.0.latency_us.load(Ordering::Relaxed)))
    }
}

fn store_latency(state: &StreamState, latency: Duration) {
    let micros = u32::try_from(latency.as_micros()).unwrap_or(u32::MAX);
    state.latency_us.store(micros, Ordering::Relaxed);
}

/// Microphone input. Samples arrive as mono f32 at [`Capture::device_rate`].
pub struct Capture {
    _stream: cpal::Stream,
    device_rate: u32,
    state: Arc<StreamState>,
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
        let state = Arc::new(StreamState::default());
        let stream = match config.sample_format() {
            SampleFormat::F32 => build_capture::<f32>(&device, config.config(), producer, &state)?,
            SampleFormat::I16 => build_capture::<i16>(&device, config.config(), producer, &state)?,
            SampleFormat::I32 => build_capture::<i32>(&device, config.config(), producer, &state)?,
            SampleFormat::U16 => build_capture::<u16>(&device, config.config(), producer, &state)?,
            other => return Err(AudioError::UnsupportedFormat(other)),
        };
        stream.play()?;
        Ok((
            Self {
                _stream: stream,
                device_rate,
                state,
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
        self.state.failed.load(Ordering::Relaxed)
    }

    /// Handle to the device latency, readable from other threads.
    pub fn latency(&self) -> DeviceLatency {
        DeviceLatency(Arc::clone(&self.state))
    }
}

/// Speaker output. Expects mono f32 at [`Playback::device_rate`]; plays
/// silence when the ring buffer is empty.
pub struct Playback {
    _stream: cpal::Stream,
    device_rate: u32,
    state: Arc<StreamState>,
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
        let state = Arc::new(StreamState::default());
        let stream = match config.sample_format() {
            SampleFormat::F32 => build_playback::<f32>(&device, config.config(), consumer, &state)?,
            SampleFormat::I16 => build_playback::<i16>(&device, config.config(), consumer, &state)?,
            SampleFormat::I32 => build_playback::<i32>(&device, config.config(), consumer, &state)?,
            SampleFormat::U16 => build_playback::<u16>(&device, config.config(), consumer, &state)?,
            other => return Err(AudioError::UnsupportedFormat(other)),
        };
        stream.play()?;
        Ok((
            Self {
                _stream: stream,
                device_rate,
                state,
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
        self.state.failed.load(Ordering::Relaxed)
    }

    /// Handle to the device latency, readable from other threads.
    pub fn latency(&self) -> DeviceLatency {
        DeviceLatency(Arc::clone(&self.state))
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
    state: &Arc<StreamState>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let scale = 1.0 / channels as f32;
    let rate = u64::from(config.sample_rate);
    let state = Arc::clone(state);
    let error_state = Arc::clone(&state);
    let stream = device.build_input_stream(
        config,
        move |data: &[T], info: &InputCallbackInfo| {
            // Some drivers (WASAPI) report capture and callback as the same
            // instant. The first sample of a buffer has waited at least the
            // buffer's length, so use that as a floor.
            let timestamp = info.timestamp();
            let reported = timestamp.callback.duration_since(timestamp.capture);
            let buffered =
                Duration::from_nanos((data.len() / channels) as u64 * 1_000_000_000 / rate);
            store_latency(&state, reported.max(buffered));
            for frame in data.chunks_exact(channels) {
                let mono = frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() * scale;
                // A full ring means the consumer is behind; drop samples.
                let _ = producer.push(mono);
            }
        },
        move |_| error_state.failed.store(true, Ordering::Relaxed),
        None,
    )?;
    Ok(stream)
}

fn build_playback<T>(
    device: &Device,
    config: StreamConfig,
    mut consumer: Consumer<f32>,
    state: &Arc<StreamState>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let state = Arc::clone(state);
    let error_state = Arc::clone(&state);
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], info: &OutputCallbackInfo| {
            let timestamp = info.timestamp();
            store_latency(
                &state,
                timestamp.playback.duration_since(timestamp.callback),
            );
            for frame in data.chunks_exact_mut(channels) {
                frame.fill(T::from_sample(consumer.pop().unwrap_or(0.0)));
            }
        },
        move |_| error_state.failed.store(true, Ordering::Relaxed),
        None,
    )?;
    Ok(stream)
}
