# Hangtime architecture

This document describes the design of Hangtime. Decisions with
meaningful alternatives are recorded separately in
[decisions/](decisions/); this file describes how the pieces fit.

## Goals

- DMR-style voice comms between computers: PTT, VOX, squelch,
  talkgroups, private calls, floor control, and hang time.
- Serverless operation when peers share a network.
- Optional server for off-net comms, acting like a repeater.
- Simple to build and reason about; fast; small memory footprint.
- Tray-first desktop app driven by global hotkeys, with a pleasant but
  minimal UI.

## Non-goals (for now)

- Full-duplex conferencing within a group. Hangtime is half-duplex per
  talkgroup by design.
- Video, screen sharing, or text chat.
- Interoperability with real DMR radios or networks.
- Browser clients.

## Core model: one talker per talkgroup

Most voice chat apps are full-duplex conference systems: everyone can
talk at once, so they need mixing servers, aggressive echo
cancellation, and selective forwarding infrastructure. DMR avoids that
with floor control. A talkgroup has one transmitter at a time, everyone
else receives, and a second key-up is refused.

Hangtime keeps that rule ([ADR-0004](decisions/0004-floor-control.md)).
Consequences:

- Receivers handle at most one stream per talkgroup. Mixing happens
  only across monitored groups (the scan list).
- Bandwidth is predictable. The talker sends N-1 copies in
  peer-to-peer mode, or one copy to a server; listeners send nothing.
- Echo cancellation matters less, since nobody transmits while a group
  is receiving. Full-duplex private calls are an optional exception.
- The server can forward packets without decoding or mixing.

## System overview

One binary with a core engine split into layers:

```text
+---------------------------------------------------------+
| UI (window created on demand) + Tray + Global hotkeys   |
+---------------------------------------------------------+
| Session: talkgroups, private calls, floor control,      |
|   hang timer, time-out timer, scan list, tones          |
+----------------------------+----------------------------+
| Audio pipeline             | Identity / directory       |
| capture -> NS -> VOX/PTT   | Ed25519 keypair = radio ID |
|   -> Opus encode           | group keys / server ACLs   |
| Opus decode -> jitter buf  |                            |
|   -> squelch -> mix -> out |                            |
+----------------------------+----------------------------+
| Transport: QUIC (quinn)                                 |
|   datagrams = voice frames, streams = control           |
+---------------------------------------------------------+
| Discovery: mDNS | static peers | Tailscale | server     |
+---------------------------------------------------------+
```

The binary runs in two roles:

- **Client (default):** tray app with audio, UI, and hotkeys.
- **Server (`--server`):** headless repeater, rendezvous, relay, and
  directory. It reuses the transport and session crates and has no
  audio or UI dependencies at runtime.

## Audio pipeline

### Capture and playback

- `cpal` provides cross-platform audio I/O (WASAPI, CoreAudio,
  ALSA/PipeWire).
- The audio callback never allocates, locks, blocks, or logs. It moves
  samples into or out of a lock-free ring buffer (`rtrb` or
  `ringbuf`). All processing runs on a dedicated audio thread.
- Internal format: 48 kHz mono f32. Resample at the device boundary if
  the device uses another rate.

### Transmit chain

1. **Noise suppression:** RNNoise via the `nnnoiseless` crate.
1. **Transmit gate:** PTT state, or VOX.
1. **VOX:** voice activity detection plus a VOX hang timer so
   transmission doesn't chop between words. A pre-roll buffer of about
   100-200 ms is sent when VOX triggers, so the first syllable isn't
   lost.
1. **Encode:** Opus, 20 ms frames, mono, voice mode, roughly
   16-24 kbps, in-band FEC on, DTX on
   ([ADR-0003](decisions/0003-opus-codec.md)).

### Receive chain

1. **Jitter buffer:** adaptive, targeting 40-80 ms. Uses sequence
   numbers and timestamps from the datagram header; Opus FEC and packet
   loss concealment fill gaps.
1. **Decode:** Opus.
1. **Squelch:** receive-side gate. Suppresses streams below a level
   threshold and applies per-group mute and monitor settings.
1. **Mix:** sums at most one stream per monitored talkgroup.
1. **Tones:** talk-permit, channel busy (bonk), courtesy tone at end of
   transmission, hang-time end.

## Transport

QUIC via `quinn` ([ADR-0002](decisions/0002-quic-transport.md)):

- **Voice** travels in unreliable QUIC datagrams (RFC 9221). A late
  voice frame is useless, so it is never retransmitted.
- **Control** travels on reliable QUIC streams: key-up, key-down,
  floor grants and denials, presence, group membership, and state sync.
- One UDP port per node, which keeps firewall rules simple.
- TLS 1.3 from QUIC provides encryption. Certificates are self-signed
  and pinned to the node's Ed25519 key, so there is no PKI to run.

The wire format is specified in [PROTOCOL.md](PROTOCOL.md).

## Identity and security

- Each install generates an **Ed25519 keypair** on first run. A short
  numeric **radio ID** is derived from the public key for display and
  addressing.
- Peers authenticate by pinning public keys. The first contact with an
  unknown key follows a trust-on-first-use policy (details are an open
  question).
- **Group keys** protect scoped talkgroups in peer-to-peer mode.
  Possession of the key is membership. Peers without it cannot decrypt
  the group's voice and ignore it.
- In server mode, the server holds authoritative ACLs and can
  distribute group keys.

## Session layer

### Talkgroups and calls

- **All Call:** every reachable peer, or every user on the server.
- **Talkgroups:** a group ID plus an optional group key.
- **Private call:** a direct connection between two radio IDs.
  Full duplex may be allowed here as an option, since there is no
  shared floor.
- **Scan list:** monitor several talkgroups and transmit on the
  selected one.

### Floor control

Peer-to-peer mode has no repeater to grant the floor, so arbitration is
distributed:

1. The talker sends `KEYUP {group, radio_id, timestamp}` to all peers
   in the group on the control stream.
1. If two key-ups collide within the propagation window (tens of ms),
   a deterministic tiebreak decides; for example, the lower radio ID
   wins. The loser hears the bonk tone and stops transmitting.
1. `KEYDOWN` ends the transmission and starts the hang timer.
1. Peers joining a group mid-transmission learn the current floor
   state through a state sync message.

In server mode, the server is the arbiter, exactly as a repeater is.
The message types are the same in both modes.

### Hang time

After a talker unkeys, the floor stays reserved for a configurable
period (a few seconds). During hang time, only participants in the
previous exchange can key without contention. This keeps
back-and-forth conversations from being interrupted.

### Time-out timer

A maximum transmit duration (configurable, for example 60-180 s)
prevents a stuck PTT or VOX from holding a talkgroup indefinitely, as
the TOT does on a radio.

## Discovery

Discovery sources run together; any of them can produce a peer:

- **mDNS/DNS-SD** on the LAN, advertising `_hangtime._udp` with the
  `mdns-sd` crate. Zero-config, serverless.
- **Static peers** from configuration.
- **Tailscale peers:** list tailnet peers (from
  `tailscale status --json` or the Tailscale API) and probe them on the
  Hangtime port.
- **Server rendezvous:** a configured server introduces peers to each
  other.

Limits to keep in mind:

- mDNS is link-local. It does not cross VLANs or routed subnets without
  an mDNS reflector or gateway, and it can be unreliable on enterprise
  Wi-Fi.
- Multicast does not traverse Tailscale, so mDNS cannot find tailnet
  peers.
- IP multicast for talkgroup voice is an attractive fit but fragile in
  practice (IGMP snooping, querier configuration, wireless multicast
  handling). Hangtime uses unicast fan-out by default; multicast may be
  added later as an optimization.

Once peers have found each other, voice flows directly between them
regardless of which source found them.

## Server mode

`hangtime --server` adds:

- **Repeater:** receives a talker's stream once and forwards it to
  listeners. No decoding or mixing is needed because each talkgroup has
  one talker.
- **Floor arbiter:** grants and denies the floor for its talkgroups.
- **Rendezvous and NAT traversal:** peers register their observed
  public address; the server coordinates UDP hole punching and relays
  traffic itself when punching fails (for example, behind symmetric
  NAT).
- **Directory:** users, groups, ACLs, and group key distribution.
- **Bridging (later):** links between servers work like linked
  repeaters, treated as another peer connection.

## Desktop integration

- **Tray icon as status display:** idle, receiving (with talker ID),
  transmitting, channel busy. Most use should not require opening the
  window.
- **Global hotkeys:** PTT, next and previous talkgroup, toggle VOX,
  monitor. PTT needs key-down and key-up events. Windows
  `RegisterHotKey` only reports key-down, so PTT on Windows needs a
  low-level keyboard hook (`WH_KEYBOARD_LL`). Mouse side buttons and USB
  foot pedals (which usually appear as HID keyboards) should work too.
- **UI:** created on demand and destroyed when hidden, so idle memory
  stays low ([ADR-0005](decisions/0005-ui-toolkit.md)).
- **Audio cues are part of the UX.** Tones tell the user what is
  happening without looking at the screen.

## Performance targets

These are targets to measure against, not guarantees.

| Metric | Target |
| --- | --- |
| Mouth-to-ear latency, LAN | Under 150 ms |
| Idle memory, window closed | Under about 30 MB |
| Idle CPU, nobody talking | Near zero; no polling loops |
| Wire bandwidth per voice stream | About 40-50 kbps incl. overhead |
| Peer-to-peer group size | About 15-20 before a server is better |

At 20 ms frames there are 50 packets per second. IP, UDP, and QUIC
headers add roughly 20-25 kbps on top of the Opus bitrate, which is
why the wire figure is about double the codec bitrate.

## Roadmap

1. **Two peers, one LAN:** cpal to Opus to QUIC datagrams to playback.
   PTT only, manual peer address. Measure mouth-to-ear latency.
1. **Groups and floor control:** mDNS discovery, talkgroups with group
   keys, floor arbitration, hang timer, time-out timer, tones.
1. **VOX and squelch:** noise suppression, VAD with pre-roll and VOX
   hang, receive gating, scan list.
1. **Tray app:** tray states, global hotkeys, UI, settings.
1. **Off-net:** Tailscale peer discovery, then `--server` as repeater,
   rendezvous, relay, and directory.

## Open questions

- Target platforms and order (Windows first?).
- Trust-on-first-use policy for unknown peer keys.
- How group keys are shared in peer-to-peer mode (manual, QR code,
  invite string).
- Whether private calls default to full duplex.
- Priority and emergency calls that can preempt the floor.
- Optional local recording or transmission logging.
- Multicast as an optional LAN optimization.

## References

- [cpal](https://docs.rs/cpal)
- [quinn](https://docs.rs/quinn)
- [RFC 9221: QUIC datagrams](https://www.rfc-editor.org/rfc/rfc9221)
- [RFC 6716: Opus](https://www.rfc-editor.org/rfc/rfc6716)
- [Opus codec](https://opus-codec.org)
- [mdns-sd](https://docs.rs/mdns-sd)
- [tray-icon](https://docs.rs/tray-icon)
- [global-hotkey](https://docs.rs/global-hotkey)
- [Slint](https://slint.dev)
- [Tauri 2](https://v2.tauri.app)
- [Tailscale CLI](https://tailscale.com/kb/1080/cli)