//! Hangtime binary entry point.
//!
//! Modes:
//!
//! - `hangtime [--port N] [--peer HOST:PORT]... [--name NAME]`: talk.
//!   Finds peers on the LAN with mDNS and also dials each `--peer`.
//! - `hangtime parrot [--port N] [--lan]`: headless echo node that plays
//!   each transmission back to its sender, for testing with one machine.
//!   `--lan` also makes it discoverable over mDNS.

use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::ExitCode;

use hangtime_identity::Identity;
use hangtime_transport::{DEFAULT_PORT, Node};

/// One identity per install, in the user's config folder.
const IDENTITY_FILE: &str = "hangtime-identity.key";

/// Settings and talkgroups, next to the identity.
const CONFIG_FILE: &str = "hangtime.toml";

mod config;
mod parrot;
mod ptt;
mod radio;
mod talk;

const USAGE: &str = "\
usage:
  hangtime [--port N] [--peer HOST:PORT]... [--name NAME]
      talk; finds peers on the LAN, and dials each --peer
  hangtime parrot [--port N] [--lan]
      echo each transmission back to its sender (--lan: discoverable)";

enum Mode {
    Talk { peers: Vec<SocketAddr> },
    Parrot { lan: bool },
}

struct Args {
    mode: Mode,
    port: Option<u16>,
    name: Option<String>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parrot = false;
    let mut lan = false;
    let mut port = None;
    let mut peers = Vec::new();
    let mut name = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "parrot" => parrot = true,
            "--lan" => lan = true,
            "--port" => {
                let value = args.next().ok_or("--port needs a value")?;
                port = Some(value.parse().map_err(|_| format!("bad port: {value}"))?);
            }
            "--peer" => {
                let value = args.next().ok_or("--peer needs HOST:PORT")?;
                let addr = value
                    .to_socket_addrs()
                    .map_err(|e| format!("bad peer {value}: {e}"))?
                    .next()
                    .ok_or_else(|| format!("peer {value} did not resolve"))?;
                peers.push(addr);
            }
            "--name" => name = Some(args.next().ok_or("--name needs a value")?),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let mode = match (parrot, peers.is_empty()) {
        (true, false) => return Err("parrot does not take --peer".into()),
        (true, true) => Mode::Parrot { lan },
        (false, _) if lan => {
            return Err("--lan is for parrot; talk mode always uses the LAN".into());
        }
        (false, _) => Mode::Talk { peers },
    };
    Ok(Args { mode, port, name })
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
    let runtime = tokio::runtime::Runtime::new()?;
    println!("hangtime {}", env!("CARGO_PKG_VERSION"));
    let identity = match args.mode {
        Mode::Talk { .. } => {
            let path =
                Identity::default_path(IDENTITY_FILE).ok_or("no user config folder found")?;
            let identity = Identity::load_or_create(&path)?;
            println!("identity {}", path.display());
            identity
        }
        // The parrot is a test tool: a temporary identity keeps it from
        // sharing this install's radio ID.
        Mode::Parrot { .. } => Identity::generate()?,
    };
    let config = match args.mode {
        Mode::Talk { .. } => {
            let path = Identity::default_path(CONFIG_FILE).ok_or("no user config folder found")?;
            println!("settings {}", path.display());
            config::load_or_create(&path)?
        }
        Mode::Parrot { .. } => config::Config::default(),
    };
    let name = match (&args.mode, args.name, config.name) {
        (_, Some(name), _) | (Mode::Talk { .. }, None, Some(name)) => name,
        (Mode::Talk { .. }, None, None) => default_name(),
        (Mode::Parrot { .. }, None, _) => "parrot".into(),
    };
    println!("name {name}");
    let node = {
        let _guard = runtime.enter();
        let bind = |port| Node::bind(SocketAddr::from(([0, 0, 0, 0], port)), &identity, &name);
        match (&args.mode, args.port) {
            (_, Some(port)) => bind(port)?,
            (Mode::Parrot { .. }, None) => bind(DEFAULT_PORT)?,
            // Talk mode prefers the default port but takes any free one
            // (mDNS advertises whichever it gets), so it can run beside a
            // parrot on the same machine.
            (Mode::Talk { .. }, None) => bind(DEFAULT_PORT).or_else(|_| bind(0))?,
        }
    };
    println!("this node's fingerprint {}", node.fingerprint());
    match args.mode {
        Mode::Talk { peers } => talk::run(
            &runtime,
            node,
            peers,
            &name,
            config.groups,
            config.vox,
            config.vox_sensitivity,
        ),
        Mode::Parrot { lan } => runtime.block_on(parrot::run(node, lan)),
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
    fn parses_talk_with_peers() {
        let args = parse(&["--peer", "127.0.0.1:4747", "--peer", "127.0.0.1:4748"]).unwrap();
        let Mode::Talk { peers } = args.mode else {
            panic!("not talk mode");
        };
        assert_eq!(peers.len(), 2);
        assert_eq!(args.port, None);
    }

    #[test]
    fn parses_parrot_with_port() {
        let args = parse(&["parrot", "--port", "5000", "--lan"]).unwrap();
        assert!(matches!(args.mode, Mode::Parrot { lan: true }));
        assert_eq!(args.port, Some(5000));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&["--port", "x"]).is_err());
        assert!(parse(&["parrot", "--peer", "127.0.0.1:1"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--lan"]).is_err());
    }
}
