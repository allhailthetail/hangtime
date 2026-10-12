//! `hangtime mic-test`: calibrates VOX. Records 5 s of the room and 5 s
//! of the user talking, shows what VOX hears, recommends a sensitivity
//! that keys on the user's voice but not the room, and saves the
//! recording as a WAV file for tuning.

use std::error::Error;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use hangtime_audio::{
    Capture, FrameReport, Resampler, SAMPLE_RATE, VOX_SENSITIVITY_DEFAULT, VOX_SENSITIVITY_MAX,
    VOX_SENSITIVITY_MIN, VoxEvent, VoxProbe, min_level_db,
};

const SECONDS: usize = 10;

/// When the user should start talking.
const TALK_AT: f32 = 5.0;

/// Frames per summary line (0.5 s).
const SUMMARY_FRAMES: usize = 25;

/// Speech probability above which a frame counts as the user's voice in
/// the talking half.
const VOICED: f32 = 0.5;

/// How far a level's minimum must sit from the room and from the voice.
const MARGIN_DB: f32 = 3.0;

pub fn run(wav_path: &Path, microphone: Option<&str>) -> Result<(), Box<dyn Error>> {
    let (capture, mut mic) = Capture::open(microphone)?;
    let rate = capture.device_rate();
    println!("microphone: {}", capture.description());
    if let Some(notice) = capture.notice() {
        println!("note: {notice}");
    }
    println!();
    println!("Recording {SECONDS} s to calibrate VOX.");
    println!("  First 5 s: stay quiet. Room noise and people talking nearby are");
    println!("             fine; that's what this measures.");
    println!("  Last 5 s:  talk normally, as you would on the radio.");
    println!();
    println!("  time   peak level   peak speech   VOX (level {VOX_SENSITIVITY_DEFAULT})");

    let total = rate as usize * SECONDS;
    let mut recorded = Vec::with_capacity(total);
    let mut resampler = Resampler::new(rate, SAMPLE_RATE);
    let mut probe = VoxProbe::new(VOX_SENSITIVITY_DEFAULT);
    let mut chunk = Vec::new();
    let mut at_48k = Vec::new();
    let mut reports: Vec<FrameReport> = Vec::new();
    let mut prompted = false;
    while recorded.len() < total {
        chunk.clear();
        while let Ok(sample) = mic.pop() {
            chunk.push(sample);
        }
        recorded.extend_from_slice(&chunk);
        at_48k.clear();
        resampler.process(&chunk, &mut at_48k);
        for report in probe.push(&at_48k) {
            if !prompted && report.time >= TALK_AT {
                println!("  --- now talk normally ---");
                prompted = true;
            }
            reports.push(report);
            if reports.len() % SUMMARY_FRAMES == 0 {
                print_summary(&reports[reports.len() - SUMMARY_FRAMES..]);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    recorded.truncate(total);
    write_wav(wav_path, rate, &recorded)?;

    let (room, voice) = measure(&reports);
    println!();
    match room {
        Some(room) => println!("room (quiet half):  loudest sounds around {room:.0} dB"),
        None => println!("room (quiet half):  nothing recorded"),
    }
    match voice {
        Some(voice) => println!("your voice:         around {voice:.0} dB"),
        None => println!("your voice:         not heard"),
    }
    println!("{}", recommend(room, voice).message());
    println!();
    println!("recording saved to {}", wav_path.display());
    Ok(())
}

/// The room's loudest sounds (95th percentile of quiet-half frame levels)
/// and the user's voice (median level of voiced talking-half frames).
fn measure(reports: &[FrameReport]) -> (Option<f32>, Option<f32>) {
    let (quiet, talking): (Vec<&FrameReport>, Vec<&FrameReport>) =
        reports.iter().partition(|r| r.time < TALK_AT);
    let room = percentile(quiet.iter().map(|r| r.level_db).collect(), 0.95);
    let voice = percentile(
        talking
            .iter()
            .filter(|r| r.speech >= VOICED)
            .map(|r| r.level_db)
            .collect(),
        0.5,
    );
    (room, voice)
}

fn percentile(mut values: Vec<f32>, fraction: f32) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    let index = ((values.len() - 1) as f32 * fraction).round() as usize;
    Some(values[index])
}

/// What mic-test advises.
#[derive(Debug, PartialEq)]
enum Advice {
    Level(u8),
    NoVoice,
    /// The voice isn't clearly louder than the room.
    TooClose,
}

impl Advice {
    fn message(&self) -> String {
        match self {
            Self::Level(level) => format!(
                "recommended: vox_sensitivity = {level} (keys on voice above {:.0} dB)\n\
                 Put that in hangtime.toml, or press + / - while talking.",
                min_level_db(*level)
            ),
            Self::NoVoice => "Didn't hear you talk in the last 5 s. Run mic-test again and talk \
                              after the prompt."
                .into(),
            Self::TooClose => "Your voice isn't clearly louder than the room at this mic, so VOX \
                               can't tell them apart. Use PTT, or move the mic closer to your \
                               mouth (a headset mic works best)."
                .into(),
        }
    }
}

/// Picks the level whose minimum sits closest to halfway between the
/// room and the voice, keeping a margin from each.
fn recommend(room: Option<f32>, voice: Option<f32>) -> Advice {
    let Some(voice) = voice else {
        return Advice::NoVoice;
    };
    let room = room.unwrap_or(f32::MIN);
    let middle = if room == f32::MIN {
        voice - 12.0
    } else {
        (room + voice) / 2.0
    };
    (VOX_SENSITIVITY_MIN..=VOX_SENSITIVITY_MAX)
        .filter(|&level| {
            let min = min_level_db(level);
            min >= room + MARGIN_DB && min <= voice - MARGIN_DB
        })
        .min_by(|&a, &b| {
            (min_level_db(a) - middle)
                .abs()
                .total_cmp(&(min_level_db(b) - middle).abs())
        })
        .map_or(Advice::TooClose, Advice::Level)
}

fn print_summary(window: &[FrameReport]) {
    let peak_level = window.iter().map(|r| r.level_db).fold(f32::MIN, f32::max);
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
        "  {:4.1} s  {:7.1} dB      {:4.2}        {}",
        window[0].time,
        peak_level,
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
    fn recommends_level_between_room_and_voice() {
        // The measured office: colleagues around -45 dB, a headset voice
        // around -20 dB. Halfway is -32.5; level 2 (-30) is closest.
        assert_eq!(recommend(Some(-45.0), Some(-20.0)), Advice::Level(2));
        // Quieter room, quieter voice: a more sensitive level.
        assert_eq!(recommend(Some(-60.0), Some(-36.0)), Advice::Level(5));
        // Loud room: least sensitive.
        assert_eq!(recommend(Some(-30.0), Some(-10.0)), Advice::Level(1));
    }

    #[test]
    fn says_when_vox_cant_work() {
        assert_eq!(recommend(Some(-45.0), None), Advice::NoVoice);
        assert_eq!(recommend(Some(-40.0), Some(-36.0)), Advice::TooClose);
    }

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
