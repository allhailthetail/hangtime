//! First-run setup: asks for the user's name and headset, then writes
//! `hangtime.toml` with the default groups. Talk mode runs it when there
//! is no settings file; `hangtime setup` runs it any time.
//!
//! Setup belongs to the app, not the installer: an MSI deployed by Intune
//! runs silently as SYSTEM, so it can neither ask the user anything nor
//! write their settings (ADR-0016). Without a terminal, setup writes the
//! defaults without asking.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

use hangtime_audio::{DeviceInfo, list_devices};

use crate::config::{self, Defaults};

/// Runs setup and writes the settings file at `path`, keeping any old one
/// as `hangtime.toml.bak`.
pub fn run(path: &Path) -> io::Result<()> {
    let name = first_name();
    let defaults = if io::stdin().is_terminal() && io::stdout().is_terminal() {
        ask(
            &mut io::stdin().lock(),
            &mut io::stdout(),
            &name,
            list_devices,
        )?
    } else {
        Defaults {
            name: Some(name),
            ..Defaults::default()
        }
    };
    if path.exists() {
        let backup = path.with_extension("toml.bak");
        std::fs::rename(path, &backup)?;
        println!("old settings kept as {}", backup.display());
    }
    config::write_starter(path, &defaults)?;
    println!("settings written to {}", path.display());
    Ok(())
}

/// Asks the setup questions. `devices` lists microphones and speakers;
/// it is called before and after the user connects their headset, so the
/// headset can be recognised as the device that appeared.
fn ask(
    input: &mut impl BufRead,
    out: &mut impl Write,
    suggested_name: &str,
    mut devices: impl FnMut() -> (Vec<DeviceInfo>, Vec<DeviceInfo>),
) -> io::Result<Defaults> {
    writeln!(out, "\nHangtime setup\n")?;
    let name = line(
        input,
        out,
        &format!("Your name, as others will see it [{suggested_name}]: "),
    )?
    .map(|answer| answer.trim().to_string())
    .filter(|answer| !answer.is_empty())
    .unwrap_or_else(|| suggested_name.to_string());

    let (mics_before, speakers_before) = devices();
    writeln!(
        out,
        "\nConnect the headset you'll use with Hangtime: plug it in, or turn it\n\
         on so it pairs. A headset keeps the speaker out of your microphone."
    )?;
    line(
        input,
        out,
        "Press Enter once it's connected (or if it already is): ",
    )?;
    let (mics, speakers) = devices();
    let microphone = pick(input, out, "microphone", &mics, &mics_before)?;
    let speaker = pick(input, out, "speaker", &speakers, &speakers_before)?;
    writeln!(out)?;
    Ok(Defaults {
        name: Some(name),
        microphone,
        speaker,
    })
}

/// Picks the device that appeared since the headset prompt, or asks the
/// user to choose. `None` means the system default.
fn pick(
    input: &mut impl BufRead,
    out: &mut impl Write,
    kind: &str,
    now: &[DeviceInfo],
    before: &[DeviceInfo],
) -> io::Result<Option<String>> {
    let new: Vec<&DeviceInfo> = now
        .iter()
        .filter(|device| before.iter().all(|old| old.name != device.name))
        .collect();
    if let [device] = new.as_slice() {
        writeln!(out, "Found your {kind}: {}", device.name)?;
        return Ok(Some(device.name.clone()));
    }
    if now.is_empty() {
        writeln!(
            out,
            "No {kind} found; Hangtime will use the system default."
        )?;
        return Ok(None);
    }
    writeln!(out, "\nWhich {kind} should Hangtime use?")?;
    for (number, device) in now.iter().enumerate() {
        let default = if device.is_default {
            "  (system default)"
        } else {
            ""
        };
        writeln!(out, "  {}  {}{default}", number + 1, device.name)?;
    }
    loop {
        let Some(answer) = line(input, out, "Number, or Enter for the system default: ")? else {
            return Ok(None);
        };
        let answer = answer.trim();
        if answer.is_empty() {
            return Ok(None);
        }
        if let Ok(number) = answer.parse::<usize>()
            && (1..=now.len()).contains(&number)
        {
            return Ok(Some(now[number - 1].name.clone()));
        }
        writeln!(out, "Pick a number from 1 to {}.", now.len())?;
    }
}

/// Prints `prompt` and reads one line. `None` at end of input.
fn line(
    input: &mut impl BufRead,
    out: &mut impl Write,
    prompt: &str,
) -> io::Result<Option<String>> {
    write!(out, "{prompt}")?;
    out.flush()?;
    let mut text = String::new();
    Ok((input.read_line(&mut text)? > 0).then_some(text))
}

/// The user's first name: from their account's display name if Windows
/// has one, otherwise guessed from the login name.
pub fn first_name() -> String {
    display_name()
        .as_deref()
        .and_then(first_word)
        .or_else(|| {
            ["USERNAME", "USER"]
                .iter()
                .find_map(|var| std::env::var(var).ok())
                .as_deref()
                .and_then(from_login)
        })
        .unwrap_or_else(|| "Hangtime user".to_string())
}

/// First name from a display name: "Matthew Younger" or "Younger, Matthew".
fn first_word(display: &str) -> Option<String> {
    let given = match display.split_once(',') {
        Some((_, given)) => given,
        None => display,
    };
    given.split_whitespace().next().map(str::to_string)
}

/// First name guessed from a login: "DOMAIN\matthew.younger" or
/// "matthew.younger@example.com" gives "Matthew".
fn from_login(login: &str) -> Option<String> {
    let login = login.rsplit('\\').next()?;
    let login = login.split('@').next()?;
    let first = login
        .split(['.', '_', '-', ' '])
        .find(|part| !part.is_empty())?;
    let mut chars = first.chars();
    let initial = chars.next()?;
    Some(initial.to_uppercase().chain(chars).collect())
}

/// The account's display name, such as "Matthew Younger", if the account
/// has one (domain and Entra ID accounts usually do).
#[cfg(windows)]
fn display_name() -> Option<String> {
    use windows_sys::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};

    let mut buffer = [0u16; 256];
    let mut len = buffer.len() as u32;
    // SAFETY: the buffer and its length are valid for the call; on success
    // `len` holds the number of characters written, without the NUL.
    let ok = unsafe { GetUserNameExW(NameDisplay, buffer.as_mut_ptr(), &mut len) };
    if !ok || len == 0 {
        return None;
    }
    let name = String::from_utf16_lossy(&buffer[..len as usize]);
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(not(windows))]
fn display_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, is_default: bool) -> DeviceInfo {
        DeviceInfo {
            name: name.into(),
            is_default,
        }
    }

    #[test]
    fn first_names() {
        assert_eq!(first_word("Matthew Younger").as_deref(), Some("Matthew"));
        assert_eq!(first_word("Younger, Matthew").as_deref(), Some("Matthew"));
        assert_eq!(first_word("  ").as_deref(), None);
        assert_eq!(from_login("matthew.younger").as_deref(), Some("Matthew"));
        assert_eq!(from_login("CORP\\jane_doe").as_deref(), Some("Jane"));
        assert_eq!(from_login("sam@example.com").as_deref(), Some("Sam"));
        assert!(!first_name().is_empty());
    }

    #[test]
    fn finds_the_headset_that_appears() {
        let mut calls = 0;
        let devices = || {
            calls += 1;
            let mut mics = vec![device("Laptop Mic", true)];
            let mut speakers = vec![device("Laptop Speakers", true)];
            if calls > 1 {
                mics.push(device("Headset (WH-1000XM4)", false));
                speakers.push(device("Headphones (WH-1000XM4)", false));
            }
            (mics, speakers)
        };
        let mut input = "\n\n".as_bytes();
        let mut out = Vec::new();
        let defaults = ask(&mut input, &mut out, "Matthew", devices).unwrap();
        assert_eq!(defaults.name.as_deref(), Some("Matthew"));
        assert_eq!(defaults.microphone.as_deref(), Some("Headset (WH-1000XM4)"));
        assert_eq!(defaults.speaker.as_deref(), Some("Headphones (WH-1000XM4)"));
    }

    #[test]
    fn asks_when_nothing_new_appears() {
        let devices = || {
            (
                vec![device("Laptop Mic", true), device("USB Mic", false)],
                vec![device("Laptop Speakers", true)],
            )
        };
        // Name, Enter after connecting, a bad pick, mic 2, default speaker.
        let mut input = "Matt\n\n9\n2\n\n".as_bytes();
        let mut out = Vec::new();
        let defaults = ask(&mut input, &mut out, "Matthew", devices).unwrap();
        assert_eq!(defaults.name.as_deref(), Some("Matt"));
        assert_eq!(defaults.microphone.as_deref(), Some("USB Mic"));
        assert_eq!(defaults.speaker, None);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Pick a number from 1 to 2.")
        );
    }
}
