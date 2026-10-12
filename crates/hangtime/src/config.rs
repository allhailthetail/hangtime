//! Settings file: `hangtime.toml` in the user's config folder.
//!
//! Holds the display name and talkgroups, including group keys, so it is
//! a secret like the identity file. A commented starter file is written
//! on first run.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use hangtime_audio::{VOX_SENSITIVITY_MAX, VOX_SENSITIVITY_MIN};
use hangtime_proto::ALL_CALL;
use hangtime_session::Group;
use serde::Deserialize;

const STARTER: &str = r#"# Hangtime settings. Restart Hangtime after editing.

# Name others see. Defaults to the computer name.
# name = "Desk"

# Start with VOX on: talking keys the radio, no SPACE needed. Toggle
# with V while running. A headset avoids the speaker setting off VOX.
# vox = true

# VOX sensitivity, 1 (least) to 5 (most). Lower it if taps or room noise
# key the radio; raise it if speech is missed. Adjust live with + and -.
# vox_sensitivity = 3

# Talkgroups. All Call (everyone) is always available as group 0 in the
# app; groups below are numbered 1, 2, ... in the order listed.
#
# Peers with the same id and key share a group. Leave out key for an open
# group anyone can join. A key is a shared passphrase: give it out
# privately, and use a long one.
#
# [[group]]
# name = "Ops"
# id = 100
# key = "a long shared passphrase"
#
# [[group]]
# name = "Lobby"
# id = 200
"#;

#[derive(Debug, Default)]
pub struct Config {
    pub name: Option<String>,
    pub vox: bool,
    pub vox_sensitivity: Option<u8>,
    /// Configured talkgroups, not including All Call.
    pub groups: Vec<Group>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: Option<String>,
    #[serde(default)]
    vox: bool,
    vox_sensitivity: Option<u8>,
    #[serde(default, rename = "group")]
    groups: Vec<GroupEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupEntry {
    name: String,
    id: u32,
    key: Option<String>,
}

/// Loads the settings file, writing a commented starter if it is missing.
pub fn load_or_create(path: &Path) -> Result<Config, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, STARTER);
            STARTER.to_string()
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse(text: &str) -> Result<Config, String> {
    let file: File = toml::from_str(text).map_err(|e| e.to_string())?;
    if let Some(level) = file.vox_sensitivity
        && !(VOX_SENSITIVITY_MIN..=VOX_SENSITIVITY_MAX).contains(&level)
    {
        return Err(format!(
            "vox_sensitivity {level} is out of range; use {VOX_SENSITIVITY_MIN} to {VOX_SENSITIVITY_MAX}"
        ));
    }
    let mut seen = HashSet::new();
    let mut groups = Vec::new();
    for entry in file.groups {
        if entry.id == 0 || entry.id == ALL_CALL {
            return Err(format!(
                "group {:?}: id {} is reserved",
                entry.name, entry.id
            ));
        }
        if !seen.insert(entry.id) {
            return Err(format!("group id {} is listed twice", entry.id));
        }
        if entry.name.trim().is_empty() {
            return Err(format!("group {} needs a name", entry.id));
        }
        if entry.key.as_deref().is_some_and(str::is_empty) {
            return Err(format!(
                "group {:?}: key is empty; remove it for an open group",
                entry.name
            ));
        }
        groups.push(Group {
            id: entry.id,
            name: entry.name,
            key: entry.key,
        });
    }
    Ok(Config {
        name: file.name,
        vox: file.vox,
        vox_sensitivity: file.vox_sensitivity,
        groups,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_file_parses_to_no_groups() {
        let config = parse(STARTER).unwrap();
        assert!(config.groups.is_empty());
        assert_eq!(config.name, None);
    }

    #[test]
    fn parses_groups_in_order() {
        let config = parse(
            r#"
            name = "Desk"
            vox = true
            vox_sensitivity = 2
            [[group]]
            name = "Ops"
            id = 100
            key = "secret"
            [[group]]
            name = "Lobby"
            id = 200
            "#,
        )
        .unwrap();
        assert_eq!(config.name.as_deref(), Some("Desk"));
        assert!(config.vox);
        assert_eq!(config.vox_sensitivity, Some(2));
        assert_eq!(config.groups.len(), 2);
        assert_eq!(config.groups[0].key.as_deref(), Some("secret"));
        assert_eq!(config.groups[1].key, None);
    }

    #[test]
    fn rejects_bad_groups() {
        let bad = [
            "[[group]]\nname = \"x\"\nid = 0",
            "[[group]]\nname = \"x\"\nid = 4294967295",
            "[[group]]\nname = \"a\"\nid = 5\n[[group]]\nname = \"b\"\nid = 5",
            "[[group]]\nname = \"\"\nid = 5",
            "[[group]]\nname = \"x\"\nid = 5\nkey = \"\"",
            "[[group]]\nname = \"x\"\nid = 5\ncolour = \"red\"",
            "vox_sensitivity = 0",
            "vox_sensitivity = 6",
        ];
        for text in bad {
            assert!(parse(text).is_err(), "accepted: {text}");
        }
    }
}
