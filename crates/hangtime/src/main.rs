//! Hangtime binary entry point.
//!
//! Phase 1 modes:
//!
//! - `hangtime [--port N] [--peer HOST:PORT]`: talk to one peer. Without
//!   `--peer`, waits for a peer to connect.
//! - `hangtime parrot [--port N]`: headless echo node that plays each
//!   transmission back to its sender, for testing with one machine.

use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs};
use std::process::ExitCode;

use hangtime_identity::Identity;
use hangtime_transport::{DEFAULT_PORT, Node};

/// One identity per install, in the user's config folder.
const IDENTITY_FILE: &str = "hangtime-identity.key";

mod parrot;
mod ptt;
mod talk;

const USAGE: &str = "\
usage:
  hangtime [--port N] [--peer HOST:PORT]   talk to one peer (waits for one without --peer)
  hangtime parrot [--port N]               echo each transmission back to its sender";

enum Mode {
    Talk { peer: Option<SocketAddr> },
    Parrot,
}

struct Args {
    mode: Mode,
    port: Option<u16>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parrot = false;
    let mut port = None;
    let mut peer = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "parrot" => parrot = true,
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
                peer = Some(addr);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let mode = match (parrot, peer) {
        (true, Some(_)) => return Err("parrot does not take --peer".into()),
        (true, None) => Mode::Parrot,
        (false, peer) => Mode::Talk { peer },
    };
    Ok(Args { mode, port })
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
    // A dialing client doesn't need a fixed port; listeners do.
    let default_port = match args.mode {
        Mode::Talk { peer: Some(_) } => 0,
        _ => DEFAULT_PORT,
    };
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
        Mode::Parrot => Identity::generate()?,
    };
    let bind = SocketAddr::from(([0, 0, 0, 0], args.port.unwrap_or(default_port)));
    let node = {
        let _guard = runtime.enter();
        Node::bind(bind, &identity)?
    };
    println!("this node's fingerprint {}", node.fingerprint());
    match args.mode {
        Mode::Talk { peer } => talk::run(&runtime, &node, peer),
        Mode::Parrot => runtime.block_on(parrot::run(&node)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_talk_with_peer() {
        let args = parse(&["--peer", "127.0.0.1:4747"]).unwrap();
        assert!(matches!(args.mode, Mode::Talk { peer: Some(addr) } if addr.port() == 4747));
        assert_eq!(args.port, None);
    }

    #[test]
    fn parses_parrot_with_port() {
        let args = parse(&["parrot", "--port", "5000"]).unwrap();
        assert!(matches!(args.mode, Mode::Parrot));
        assert_eq!(args.port, Some(5000));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(&["--port", "x"]).is_err());
        assert!(parse(&["parrot", "--peer", "127.0.0.1:1"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }
}
