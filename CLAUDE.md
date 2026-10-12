# Hangtime

DMR-style push-to-talk voice comms between computers. Rust, single
binary: tray client by default, headless repeater with
`hangtime --server`.

Read `docs/ARCHITECTURE.md` before changing audio, transport,
session/floor control, or discovery code. Read `docs/PROTOCOL.md`
before touching anything that goes on the wire. Check `docs/decisions/`
before proposing a different library, protocol, or toolkit; those
questions are settled unless an ADR is superseded.

Build in the roadmap order in `docs/ARCHITECTURE.md`. Don't start a
later phase's features while working on an earlier one.

## Commands

- Build: `cargo build` / release: `cargo build --release`
- Test: `cargo test --workspace` (run before finishing any change)
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Format: `cargo fmt --all`
- Markdown lint: `npx markdownlint-cli "**/*.md"`

## Layout

Cargo workspace (edition 2024); every crate lives in `crates/`.

- `hangtime`: the binary. Wires the library crates together; talk
  mode (mesh, talkgroups), `parrot` echo node, terminal PTT,
  `hangtime.toml` settings.
- `hangtime-proto`: wire format shared by client and server
  (mirrors `docs/PROTOCOL.md`).
- `hangtime-transport`: QUIC connections, voice datagrams, control
  streams.
- `hangtime-audio`: device I/O, ring buffer handoff, Opus, noise
  suppression, VOX, tones.
- `hangtime-identity`: persistent Ed25519 key, fingerprint, radio ID.
- `hangtime-session`: floor control and group membership proofs (no I/O).
- `hangtime-discovery`: mDNS advertise and browse.

Later phases add crates for UI and server-only logic (`hangtime-server`)
as they are needed, not before.

<!-- Fill in once the workspace exists. Planned crates:
crates/core (session, floor control, groups), crates/audio,
crates/transport, crates/discovery, crates/app (tray + UI),
crates/server. -->

## Invariants (do not violate without asking)

- Audio callbacks never allocate, lock, block, or log. Hand frames off
  through the lock-free ring buffer only.
- One talker per talkgroup. Floor control is the core model; do not add
  multi-talker mixing within a group. Mixing exists only across
  monitored groups (scan list).
- Voice frames go in QUIC datagrams; control and signaling go on QUIC
  streams. Never send voice on a reliable stream.
- Client and server share the same protocol and session code.
  Server-only logic lives in the server crate, not in forks of shared
  code.
- Peer-to-peer must keep working with no server present.
- Identity is the Ed25519 keypair. Never commit keys, group keys, or
  test credentials that look real.

## Workflow

- Ask before changing the wire protocol (message types, datagram
  header, version field). Any change bumps the protocol version and
  updates `docs/PROTOCOL.md`.
- Ask before adding a dependency that pulls in a GUI or webview stack,
  or a native library that needs a C toolchain.
- When a design decision is made, add an ADR in `docs/decisions/`
  using the existing format, and update `ARCHITECTURE.md` if it changes
  the design.
- Keep idle memory low: no background polling loops; the UI window is
  created on demand and destroyed when hidden.
- Keep this file short. Detail belongs in `docs/`.

## Known pitfalls

- Windows `RegisterHotKey` gives no key-up event; PTT needs a
  low-level keyboard hook (`WH_KEYBOARD_LL`).
- mDNS does not cross VLANs, routed subnets, or Tailscale; discovery
  must fall back to static peers or server rendezvous.
