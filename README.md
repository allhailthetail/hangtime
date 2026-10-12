# Hangtime

DMR-style push-to-talk voice comms between computers.

Hangtime brings the parts of DMR radio that work well to desktop voice
chat: push-to-talk, VOX, squelch, talkgroups, private calls, floor
control, and call hang time. It lives in the system tray, is driven by
global hotkeys, and works peer-to-peer on a LAN with no server at all.
An optional server acts as a repeater for off-net comms.

> **Status:** early design. Nothing is built yet. The design lives in
> [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Planned features

- **PTT and VOX:** global push-to-talk hotkey, or voice-operated
  transmit with pre-roll so the first syllable isn't clipped.
- **Squelch and noise suppression:** receive-side gating and RNNoise on
  the transmit side.
- **Talkgroups:** All Call (everyone reachable), scoped groups protected
  by a group key, and private one-to-one calls.
- **Floor control:** one talker per talkgroup, with talk-permit and
  channel-busy tones, as on a DMR repeater.
- **Hang time:** after a talker unkeys, the channel stays reserved
  briefly so the people in that exchange can reply.
- **Scan list:** monitor several talkgroups, transmit on one.
- **Serverless on a LAN:** peers find each other with mDNS and talk
  directly.
- **Off-net:** run over a Tailscale tailnet, or through a Hangtime
  server that relays like a repeater.
- **Lightweight:** a single Rust binary that idles in the tray with a
  small memory footprint.

## How it works

Each install has an Ed25519 keypair that acts as its radio ID. Audio is
captured, cleaned up, encoded with Opus, and sent as QUIC datagrams;
control messages (key-up, key-down, floor, presence) travel on QUIC
streams over the same connection. Because only one person talks per
talkgroup at a time, receivers never need to mix voices within a group,
which keeps both the client and the server simple.

The same binary runs as the tray client or, with `--server`, as a
headless repeater, rendezvous point, and relay.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full design
and [docs/decisions/](docs/decisions/) for the reasoning behind each
major choice.

## Building

Requires a stable Rust toolchain, installed with
[rustup](https://rustup.rs).

```bash
cargo build --release
cargo test --workspace
```

Platform-specific native dependencies will be listed here as they are
added.

## Usage (planned)

```bash
hangtime                 # start the tray client
hangtime --server        # run as a headless repeater
hangtime status          # show peers, talkgroups, and floor state
```

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Wire protocol](docs/PROTOCOL.md) (draft)
- [Decision records](docs/decisions/)

## License

To be decided.