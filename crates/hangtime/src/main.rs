//! Hangtime binary entry point.
//!
//! Modes:
//!
//! - `hangtime [--port N] [--peer HOST:PORT]... [--name NAME] [--mic NAME]
//!   [--speaker NAME]`: talk. Finds peers on the LAN with mDNS and also
//!   dials each `--peer`.
//! - `hangtime parrot [--port N] [--lan]`: headless echo node that plays
//!   each transmission back to its sender, for testing with one machine.
//!   `--lan` also makes it discoverable over mDNS.
//! - `hangtime mic-test [--mic NAME]`: record the microphone and show what
//!   VOX hears.
//! - `hangtime devices`: list microphones and speakers.

use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::ExitCode;

use hangtime_audio::list_devices;
use hangtime_identity::Identity;
use hangtime_transport::{DEFAULT_PORT, Node};
use tokio::runtime::Runtime;

/// One identity per install, in the user's config folder.
const IDENTITY_FILE: &str = "hangtime-identity.key";

/// Settings and talkgroups, next to the identity.
const CONFIG_FILE: &str = "hangtime.toml";

/// Where mic-test saves its recording, next to the identity.
const MIC_TEST_FILE: &str = "hangtime-mic-test.wav";

mod config;
mod echo;
mod mic_test;
mod parrot;
mod ptt;
mod radio;
mod talk;
mod ui;

const USAGE: &str = "\
usage:
  hangtime [--port N] [--peer HOST:PORT]... [--name NAME] [--mic NAME] [--speaker NAME]
      talk; finds peers on the LAN, and dials each --peer
  hangtime parrot [--port N] [--lan]
      echo each transmission back to its sender (--lan: discoverable)
  hangtime mic-test [--mic NAME]
      record the microphone for 10 s and show what VOX hears
  hangtime devices
      list microphones and speakers (names for --mic and --speaker)

--mic and --speaker match a device name, or any name containing the text,
ignoring case. If it isn't available, Hangtime uses the default device.";

#[derive(Debug, PartialEq)]
enum Mode {
    Talk { peers: Vec<SocketAddr> },
    Parrot { lan: bool },
    MicTest,
    Devices,
}

#[derive(Debug)]
struct Args {
    mode: Mode,
    port: Option<u16>,
    name: Option<String>,
    mic: Option<String>,
    speaker: Option<String>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut command = None;
    let mut lan = false;
    let mut port = None;
    let mut peers = Vec::new();
    let mut name = None;
    let mut mic = None;
    let mut speaker = None;
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "parrot" | "mic-test" | "devices" if command.is_none() => command = Some(arg),
            "--lan" => lan = true,
            "--port" => {
                let text = value("--port")?;
                port = Some(text.parse().map_err(|_| format!("bad port: {text}"))?);
            }
            "--peer" => {
                let text = value("--peer")?;
                let addr = text
                    .to_socket_addrs()
                    .map_err(|e| format!("bad peer {text}: {e}"))?
                    .next()
                    .ok_or_else(|| format!("peer {text} did not resolve"))?;
                peers.push(addr);
            }
            "--name" => name = Some(value("--name")?),
            "--mic" => mic = Some(value("--mic")?),
            "--speaker" => speaker = Some(value("--speaker")?),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    // Reject flags that mean nothing in the chosen mode, so typos show up.
    let unused = |flags: &[(&str, bool)]| match flags.iter().find(|(_, given)| *given) {
        Some((flag, _)) => Err(format!("{flag} doesn't apply here")),
        None => Ok(()),
    };
    let mode = match command.as_deref() {
        None => {
            unused(&[("--lan (it's for parrot)", lan)])?;
            Mode::Talk { peers }
        }
        Some("parrot") => {
            unused(&[
                ("--peer", !peers.is_empty()),
                ("--mic", mic.is_some()),
                ("--speaker", speaker.is_some()),
            ])?;
            Mode::Parrot { lan }
        }
        Some("mic-test") => {
            unused(&[
                ("--peer", !peers.is_empty()),
                ("--lan", lan),
                ("--port", port.is_some()),
                ("--name", name.is_some()),
                ("--speaker", speaker.is_some()),
            ])?;
            Mode::MicTest
        }
        _ => {
            unused(&[
                ("--peer", !peers.is_empty()),
                ("--lan", lan),
                ("--port", port.is_some()),
                ("--name", name.is_some()),
                ("--mic", mic.is_some()),
                ("--speaker", speaker.is_some()),
            ])?;
            Mode::Devices
        }
    };
    Ok(Args {
        mode,
        port,
        name,
        mic,
        speaker,
    })
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), Box<dyn Error>> {
    match args.mode {
        Mode::Devices => {
            print_devices();
            Ok(())
        }
        Mode::MicTest => {
            let config = load_config()?;
            let path = config_path(MIC_TEST_FILE)?;
            mic_test::run(&path, args.mic.or(config.microphone).as_deref())
        }
        Mode::Parrot { lan } => {
            println!("hangtime {}", env!("CARGO_PKG_VERSION"));
            // The parrot is a test tool: a temporary identity keeps it from
            // sharing this install's radio ID.
            let identity = Identity::generate()?;
            let name = args.name.unwrap_or_else(|| "parrot".into());
            let runtime = Runtime::new()?;
            let node = bind(
                &runtime,
                &identity,
                &name,
                args.port.unwrap_or(DEFAULT_PORT),
                None,
            )?;
            runtime.block_on(parrot::run(node, lan))
        }
        Mode::Talk { peers } => {
            println!("hangtime {}", env!("CARGO_PKG_VERSION"));
            let path = config_path(IDENTITY_FILE)?;
            let identity = Identity::load_or_create(&path)?;
            println!("identity {}", path.display());
            let config = load_config()?;
            let name = args.name.or(config.name).unwrap_or_else(default_name);
            let runtime = Runtime::new()?;
            // Talk mode prefers the default port but takes any free one
            // (mDNS advertises whichever it gets), so it can run beside a
            // parrot on the same machine.
            let fallback_port = args.port.is_none().then_some(0);
            let port = args.port.unwrap_or(DEFAULT_PORT);
            let node = bind(&runtime, &identity, &name, port, fallback_port)?;
            talk::run(
                &runtime,
                node,
                talk::Options {
                    peers,
                    name,
                    groups: config.groups,
                    muted: config.muted,
                    vox: config.vox,
                    vox_sensitivity: config.vox_sensitivity,
                    microphone: args.mic.or(config.microphone),
                    speaker: args.speaker.or(config.speaker),
                },
            )
        }
    }
}

/// Binds the node on `port`, or on `fallback` if that fails.
fn bind(
    runtime: &Runtime,
    identity: &Identity,
    name: &str,
    port: u16,
    fallback: Option<u16>,
) -> Result<Node, Box<dyn Error>> {
    let _guard = runtime.enter();
    let at = |port: u16| Node::bind(SocketAddr::from(([0, 0, 0, 0], port)), identity, name);
    let node = match (at(port), fallback) {
        (Ok(node), _) => node,
        (Err(_), Some(fallback)) => at(fallback)?,
        (Err(e), None) => return Err(e.into()),
    };
    println!("name {name}");
    println!("this node's fingerprint {}", node.fingerprint());
    Ok(node)
}

fn config_path(file: &str) -> Result<std::path::PathBuf, Box<dyn Error>> {
    Ok(Identity::default_path(file).ok_or("no user config folder found")?)
}

fn load_config() -> Result<config::Config, Box<dyn Error>> {
    let path = config_path(CONFIG_FILE)?;
    println!("settings {}", path.display());
    Ok(config::load_or_create(&path)?)
}

fn print_devices() {
    let (inputs, outputs) = list_devices();
    for (heading, devices) in [("microphones", inputs), ("speakers", outputs)] {
        println!("{heading}:");
        if devices.is_empty() {
            println!("  (none found)");
        }
        for device in devices {
            let marker = if device.is_default { "  (default)" } else { "" };
            println!("  {}{marker}", device.name);
        }
    }
}

/// The computer's name, used when `--name` is not given.
fn default_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|var| std::env::var(var).ok())
        .unwrap_or_else(|| "hangtime".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_talk_with_peers_and_devices() {
        let args = parse(&[
            "--peer",
            "127.0.0.1:4747",
            "--peer",
            "127.0.0.1:4748",
            "--mic",
            "WH-1000XM4",
            "--speaker",
            "Headset",
        ])
        .unwrap();
        let Mode::Talk { peers } = args.mode else {
            panic!("not talk mode");
        };
        assert_eq!(peers.len(), 2);
        assert_eq!(args.mic.as_deref(), Some("WH-1000XM4"));
        assert_eq!(args.speaker.as_deref(), Some("Headset"));
        assert_eq!(args.port, None);
    }

    #[test]
    fn parses_parrot_with_port() {
        let args = parse(&["parrot", "--port", "5000", "--lan"]).unwrap();
        assert_eq!(args.mode, Mode::Parrot { lan: true });
        assert_eq!(args.port, Some(5000));
    }

    #[test]
    fn parses_mic_test_and_devices() {
        let args = parse(&["mic-test", "--mic", "xm4"]).unwrap();
        assert_eq!(args.mode, Mode::MicTest);
        assert_eq!(args.mic.as_deref(), Some("xm4"));
        assert_eq!(parse(&["devices"]).unwrap().mode, Mode::Devices);
    }

    #[test]
    fn rejects_bad_input() {
        let bad: &[&[&str]] = &[
            &["--port", "x"],
            &["parrot", "--peer", "127.0.0.1:1"],
            &["parrot", "--mic", "x"],
            &["--bogus"],
            &["--lan"],
            &["mic-test", "--speaker", "x"],
            &["devices", "--mic", "x"],
            &["--mic"],
            &["parrot", "devices"],
        ];
        for args in bad {
            assert!(parse(args).is_err(), "accepted {args:?}");
        }
    }
}
