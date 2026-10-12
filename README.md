# Hangtime

DMR-style push-to-talk voice comms between computers.

Hangtime brings the parts of DMR radio that work well to desktop voice
chat: push-to-talk, VOX, squelch, talkgroups, private calls, floor
control, and call hang time. It lives in the system tray, is driven by
global hotkeys, and works peer-to-peer on a LAN with no server at all.
An optional server acts as a repeater for off-net comms.

> **Status:** in development (phase 2 of 5). A terminal build does PTT
> voice between peers on a LAN with floor control, tones, and mDNS
> discovery. The design lives in
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

Native build dependencies:

- **Windows:** Visual Studio Build Tools with the C++ workload and
  CMake (both included in the "Desktop development with C++" workload).
- **Linux:** `build-essential`, `cmake`, `pkg-config`, `libasound2-dev`.
- **macOS:** Xcode Command Line Tools and `cmake`.

On Windows the release exe links the C runtime statically, so
`target\release\hangtime.exe` is a single file that runs on any
64-bit Windows 10 or 11 machine with nothing else installed.

## Usage

```bash
hangtime                          # talk; finds peers on the LAN
hangtime --peer 192.168.1.20:4747 # also dial a peer by address
hangtime --name "Desk"            # name shown to others
hangtime parrot                   # echo node for testing alone
hangtime parrot --lan             # echo node, discoverable on the LAN
hangtime devices                  # list microphones and speakers
hangtime --mic "WH-1000XM4"       # use a specific microphone
hangtime mic-test                 # see what VOX hears from your mic
```

`--mic` and `--speaker` (or `microphone` and `speaker` in
`hangtime.toml`) take a device name or part of one, ignoring case. If
that device isn't available, Hangtime says so and uses the Windows
default, then any other device that works.

Hold SPACE to talk and press Q to quit. Press V for VOX, where talking
keys the radio with no SPACE needed (a headset keeps the speaker from
setting it off); + and - adjust how easily it keys. TAB or a number
key changes the talkgroup you talk on; you always hear All Call too.
Hangtime listens on UDP port 4747 (or any free port if that is taken)
and uses mDNS (UDP 5353) to find peers.

Talkgroups, your display name, and VOX settings (`vox = true`,
`vox_sensitivity = 3`) are set in `hangtime.toml` in your config
folder (`%APPDATA%\hangtime` on Windows). A commented starter file is
written on first run:

```toml
[[group]]
name = "Ops"
id = 100
key = "a long shared passphrase"   # leave out for an open group
```

Machines with the same group `id` and `key` share the group; without
the key a machine never receives that group's voice.

### Trying it on two machines

1. Copy `hangtime.exe` to both machines. Each creates its own identity
   on first run.
1. Run `hangtime` on both. When Windows Firewall asks, allow access on
   the network type you are on (usually Private).
1. Within a few seconds each shows the other as joined. If not (for
   example, different subnets, or Wi-Fi that blocks multicast), run one
   side with `--peer <other machine's IP>:4747`.

Both machines must run the same build; older builds cannot connect.

Planned later: `hangtime --server` as a repeater, and a tray app.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Wire protocol](docs/PROTOCOL.md) (draft)
- [Decision records](docs/decisions/)

## License

To be decided.
