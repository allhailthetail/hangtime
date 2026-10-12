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
    description: String,
    notice: Option<String>,
    state: Arc<StreamState>,
}

impl Capture {
    /// Opens a microphone and starts capturing. Tries the device named
    /// `preferred` (an exact name, ignoring case, or else any name that
    /// contains it), then the system default, then every other input, and
    /// keeps the first that opens. [`Capture::notice`] explains when the
    /// preferred device was not used.
    pub fn open(preferred: Option<&str>) -> Result<(Self, Consumer<f32>), AudioError> {
        let host = cpal::default_host();
        let devices = host
            .input_devices()
            .map(Iterator::collect)
            .unwrap_or_default();
        let default = host.default_input_device();
        let ((stream, device_rate, description, state, consumer), notice) =
            open_first("microphone", preferred, devices, default, start_capture)?;
        Ok((
            Self {
                _stream: stream,
                device_rate,
                description,
                notice,
                state,
            },
            consumer,
        ))
    }

    /// Sample rate of the samples in the ring buffer.
    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// Device name and format, for display.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Why the preferred or default device was not used, if it wasn't.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
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
    description: String,
    notice: Option<String>,
    state: Arc<StreamState>,
}

impl Playback {
    /// Opens a speaker and starts playing, choosing the device the same
    /// way as [`Capture::open`].
    pub fn open(preferred: Option<&str>) -> Result<(Self, Producer<f32>), AudioError> {
        let host = cpal::default_host();
        let devices = host
            .output_devices()
            .map(Iterator::collect)
            .unwrap_or_default();
        let default = host.default_output_device();
        let ((stream, device_rate, description, state, producer), notice) =
            open_first("speaker", preferred, devices, default, start_playback)?;
        Ok((
            Self {
                _stream: stream,
                device_rate,
                description,
                notice,
                state,
            },
            producer,
        ))
    }

    /// Sample rate the ring buffer is played at.
    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }

    /// Device name and format, for display.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Why the preferred or default device was not used, if it wasn't.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
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

/// An audio device's name, marked when it is the system default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
}

/// Lists microphones and speakers by name.
pub fn list_devices() -> (Vec<DeviceInfo>, Vec<DeviceInfo>) {
    let host = cpal::default_host();
    let info = |devices: Option<Vec<Device>>, default: Option<Device>| {
        let default = default.map(|device| device_name(&device));
        devices
            .unwrap_or_default()
            .iter()
            .map(|device| {
                let name = device_name(device);
                let is_default = default.as_deref() == Some(name.as_str());
                DeviceInfo { name, is_default }
            })
            .collect()
    };
    (
        info(
            host.input_devices().ok().map(Iterator::collect),
            host.default_input_device(),
        ),
        info(
            host.output_devices().ok().map(Iterator::collect),
            host.default_output_device(),
        ),
    )
}

type Started<T> = (cpal::Stream, u32, String, Arc<StreamState>, T);

fn start_capture(device: &Device) -> Result<Started<Consumer<f32>>, AudioError> {
    let config = pick_config(
        device.supported_input_configs()?,
        device.default_input_config()?,
    );
    let device_rate = config.sample_rate();
    let description = describe(device, &config);
    let (producer, consumer) = RingBuffer::new(ring_len(device_rate));
    let state = Arc::new(StreamState::default());
    let stream = match config.sample_format() {
        SampleFormat::F32 => build_capture::<f32>(device, config.config(), producer, &state)?,
        SampleFormat::I16 => build_capture::<i16>(device, config.config(), producer, &state)?,
        SampleFormat::I32 => build_capture::<i32>(device, config.config(), producer, &state)?,
        SampleFormat::U16 => build_capture::<u16>(device, config.config(), producer, &state)?,
        other => return Err(AudioError::UnsupportedFormat(other)),
    };
    stream.play()?;
    Ok((stream, device_rate, description, state, consumer))
}

fn start_playback(device: &Device) -> Result<Started<Producer<f32>>, AudioError> {
    let config = pick_config(
        device.supported_output_configs()?,
        device.default_output_config()?,
    );
    let device_rate = config.sample_rate();
    let description = describe(device, &config);
    let (producer, consumer) = RingBuffer::new(ring_len(device_rate));
    let state = Arc::new(StreamState::default());
    let stream = match config.sample_format() {
        SampleFormat::F32 => build_playback::<f32>(device, config.config(), consumer, &state)?,
        SampleFormat::I16 => build_playback::<i16>(device, config.config(), consumer, &state)?,
        SampleFormat::I32 => build_playback::<i32>(device, config.config(), consumer, &state)?,
        SampleFormat::U16 => build_playback::<u16>(device, config.config(), consumer, &state)?,
        other => return Err(AudioError::UnsupportedFormat(other)),
    };
    stream.play()?;
    Ok((stream, device_rate, description, state, producer))
}

/// Opens the first candidate that works (see [`order`]). Returns it with
/// a notice when the preferred device, or the default when none was
/// preferred, could not be used.
fn open_first<T>(
    kind: &str,
    preferred: Option<&str>,
    devices: Vec<Device>,
    default: Option<Device>,
    start: impl Fn(&Device) -> Result<T, AudioError>,
) -> Result<(T, Option<String>), AudioError> {
    let default_name = default.as_ref().map(device_name);
    let mut named: Vec<(String, Device)> = devices
        .into_iter()
        .map(|device| (device_name(&device), device))
        .collect();
    // Some hosts omit the default from the list.
    if let (Some(device), Some(name)) = (default, &default_name)
        && !named.iter().any(|(candidate, _)| candidate == name)
    {
        named.push((name.clone(), device));
    }
    let mut failures = Vec::new();
    for (name, device) in order(preferred, named, default_name.as_deref()) {
        match start(&device) {
            Ok(started) => {
                let notice =
                    fallback_notice(kind, preferred, default_name.as_deref(), &name, &failures);
                return Ok((started, notice));
            }
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    if failures.is_empty() {
        Err(AudioError::NoDevice)
    } else {
        Err(AudioError::NoUsableDevice(failures.join("; ")))
    }
}

/// Explains a fallback, or `None` if the wanted device opened.
fn fallback_notice(
    kind: &str,
    preferred: Option<&str>,
    default: Option<&str>,
    used: &str,
    failures: &[String],
) -> Option<String> {
    let tried = if failures.is_empty() {
        String::new()
    } else {
        format!(" ({})", failures.join("; "))
    };
    match preferred {
        Some(wanted) if !matches_name(used, wanted) => {
            let reason = if failures.is_empty() {
                format!("no {kind} matching \"{wanted}\"")
            } else {
                format!("couldn't open the {kind} \"{wanted}\"{tried}")
            };
            Some(format!("{reason}; using {used}"))
        }
        None if default.is_some_and(|default| default != used) => Some(format!(
            "couldn't open the default {kind}{tried}; using {used}"
        )),
        _ => None,
    }
}

/// True if `name` is `wanted` or contains it, ignoring case.
fn matches_name(name: &str, wanted: &str) -> bool {
    name.to_lowercase().contains(&wanted.to_lowercase())
}

/// Orders candidates: exact name matches for `preferred` (ignoring case),
/// then names containing it, then the default, then the rest in their
/// original order.
fn order<T>(
    preferred: Option<&str>,
    mut devices: Vec<(String, T)>,
    default: Option<&str>,
) -> Vec<(String, T)> {
    let rank = |name: &str| match preferred {
        Some(wanted) if name.eq_ignore_ascii_case(wanted) => 0,
        Some(wanted) if matches_name(name, wanted) => 1,
        _ if Some(name) == default => 2,
        _ => 3,
    };
    devices.sort_by_key(|(name, _)| rank(name));
    devices
}

fn device_name(device: &Device) -> String {
    device
        .description()
        .map_or_else(|_| "unknown device".to_string(), |d| d.name().to_string())
}

/// "Name (48000 Hz, 2 ch)".
fn describe(device: &Device, config: &SupportedStreamConfig) -> String {
    format!(
        "{} ({} Hz, {} ch)",
        device_name(device),
        config.sample_rate(),
        config.channels()
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn names(ordered: Vec<(String, ())>) -> Vec<String> {
        ordered.into_iter().map(|(name, ())| name).collect()
    }

    fn devices(list: &[&str]) -> Vec<(String, ())> {
        list.iter().map(|name| (name.to_string(), ())).collect()
    }

    const DEVICES: [&str; 4] = [
        "Microphone (1080P Pro Stream)",
        "Headset (WH-1000XM4)",
        "Microphone Array (Realtek)",
        "Headset (wh-1000xm4) 2",
    ];

    #[test]
    fn exact_then_partial_match_then_default_then_rest() {
        let ordered = order(
            Some("headset (wh-1000xm4)"),
            devices(&DEVICES),
            Some("Microphone (1080P Pro Stream)"),
        );
        assert_eq!(
            names(ordered),
            [
                "Headset (WH-1000XM4)",
                "Headset (wh-1000xm4) 2",
                "Microphone (1080P Pro Stream)",
                "Microphone Array (Realtek)",
            ]
        );
    }

    #[test]
    fn without_preference_default_comes_first() {
        let ordered = order(None, devices(&DEVICES), Some("Microphone Array (Realtek)"));
        assert_eq!(names(ordered)[0], "Microphone Array (Realtek)");
    }

    #[test]
    fn no_match_falls_back_to_default() {
        let ordered = order(
            Some("Yeti"),
            devices(&DEVICES),
            Some("Headset (WH-1000XM4)"),
        );
        assert_eq!(names(ordered)[0], "Headset (WH-1000XM4)");
    }

    #[test]
    fn explains_fallbacks() {
        assert_eq!(
            fallback_notice("microphone", Some("Yeti"), Some("A"), "A", &[]).as_deref(),
            Some("no microphone matching \"Yeti\"; using A")
        );
        assert_eq!(
            fallback_notice(
                "speaker",
                Some("XM4"),
                Some("A"),
                "A",
                &["Headset (XM4): busy".into()]
            )
            .as_deref(),
            Some("couldn't open the speaker \"XM4\" (Headset (XM4): busy); using A")
        );
        assert_eq!(
            fallback_notice("microphone", None, Some("A"), "B", &["A: gone".into()]).as_deref(),
            Some("couldn't open the default microphone (A: gone); using B")
        );
        assert_eq!(
            fallback_notice("microphone", Some("xm4"), Some("A"), "Headset (XM4)", &[]),
            None
        );
        assert_eq!(
            fallback_notice("microphone", None, Some("A"), "A", &[]),
            None
        );
    }
}
