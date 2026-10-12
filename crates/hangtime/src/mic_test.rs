//! `hangtime mic-test`: records the microphone for a few seconds, shows
//! what VOX hears, and saves the recording as a WAV file for tuning.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use hangtime_audio::{
    Capture, FrameReport, Resampler, SAMPLE_RATE, VOX_SENSITIVITY_DEFAULT, VoxEvent, VoxProbe,
};

const SECONDS: usize = 10;

/// Frames per summary line (0.5 s).
const SUMMARY_FRAMES: usize = 25;

pub fn run(wav_path: &Path, microphone: Option<&str>) -> Result<(), Box<dyn Error>> {
    let (capture, mut mic) = Capture::open(microphone)?;
    let rate = capture.device_rate();
    println!("microphone: {}", capture.description());
    if let Some(notice) = capture.notice() {
        println!("note: {notice}");
    }
    println!();
    println!("Recording {SECONDS} s.");
    println!("  First 5 s: tap the desk like before, without talking.");
    println!("  Last 5 s:  talk normally.");
    println!();
    println!(
        "  time   peak level   noise floor   peak speech   VOX (level {VOX_SENSITIVITY_DEFAULT})"
    );

    let total = rate as usize * SECONDS;
    let mut recorded = Vec::with_capacity(total);
    let mut resampler = Resampler::new(rate, SAMPLE_RATE);
    let mut probe = VoxProbe::new(VOX_SENSITIVITY_DEFAULT);
    let mut chunk = Vec::new();
    let mut at_48k = Vec::new();
    let mut window: Vec<FrameReport> = Vec::new();
    while recorded.len() < total {
        chunk.clear();
        while let Ok(sample) = mic.pop() {
            chunk.push(sample);
        }
        recorded.extend_from_slice(&chunk);
        at_48k.clear();
        resampler.process(&chunk, &mut at_48k);
        for report in probe.push(&at_48k) {
            window.push(report);
            if window.len() == SUMMARY_FRAMES {
                print_summary(&window);
                window.clear();
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    recorded.truncate(total);
    write_wav(wav_path, rate, &recorded)?;
    println!();
    println!("saved {}", wav_path.display());
    Ok(())
}

fn print_summary(window: &[FrameReport]) {
    let peak_level = window.iter().map(|r| r.level_db).fold(f32::MIN, f32::max);
    let floor = window.last().map_or(0.0, |r| r.floor_db);
    let peak_speech = window.iter().map(|r| r.speech).fold(0.0, f32::max);
    let events: Vec<&str> = window
        .iter()
        .filter_map(|r| match r.event {
            Some(VoxEvent::Start) => Some("KEYS"),
            Some(VoxEvent::Stop) => Some("unkeys"),
            None => None,
        })
        .collect();
    println!(
        "  {:4.1} s  {:7.1} dB   {:7.1} dB      {:4.2}        {}",
        window[0].time,
        peak_level,
        floor,
        peak_speech,
        events.join(", ")
    );
}

/// Writes 16-bit mono PCM.
fn write_wav(path: &Path, rate: u32, samples: &[f32]) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_valid_wav_header() {
        let path =
            std::env::temp_dir().join(format!("hangtime-wav-test-{}.wav", std::process::id()));
        write_wav(&path, 16_000, &[0.0, 0.5, -0.5]).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            16_000
        );
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
        assert_eq!(bytes.len(), 44 + 6);
        assert_eq!(i16::from_le_bytes([bytes[46], bytes[47]]), 16383);
    }
}
