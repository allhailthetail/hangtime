# Hangtime wire protocol

> **Status:** draft. Protocol version `hangtime/2`. Sections marked
> TODO are not designed yet; fill them in before implementing the code
> that depends on them.

This document is the single source of truth for everything that goes
on the wire. Client and server code must match it.

## Principles

- Every connection negotiates a **protocol version** at handshake.
  Incompatible changes bump the major version.
- Voice frames travel in QUIC **datagrams**. Control messages travel on
  QUIC **streams**.
- Messages are versioned and length-prefixed. Unknown message types are
  ignored, not treated as errors, so minor additions stay compatible.
- All multi-byte integers are big-endian.

## Connection setup

Rationale is in
[ADR-0007](decisions/0007-phase-1-connection-setup.md).

- **ALPN:** `hangtime/2`. The suffix is the protocol major version, so
  version negotiation is ALPN negotiation: a node that speaks several
  major versions offers each, and peers with no version in common fail
  the handshake.
- **TLS:** TLS 1.3 (required by QUIC). Both sides present a self-signed
  certificate for their Ed25519 identity key (mutual TLS). Handshake
  signatures are verified, so a peer must hold that key. Certificates
  with any other key type fail the handshake.
- **Identity:** a node's fingerprint is the SHA-256 of its raw 32-byte
  Ed25519 public key, and its radio ID is the fingerprint's first four
  bytes, big-endian ([ADR-0008](decisions/0008-persistent-ed25519-identity.md)).
- **Trust:** any identity is accepted. Nodes show a peer as untrusted
  until the user confirms its fingerprint.
- **Datagrams:** both sides enable QUIC datagrams (RFC 9221).
- **Control stream:** the dialing side opens one bidirectional stream
  and sends `HELLO`; the accepting side replies with `HELLO`. Both
  sides send all later control messages on this stream. A peer whose
  first message is not a `HELLO`, or whose `HELLO` radio ID does not
  match its identity key, is disconnected. `HELLO` must complete within
  5 seconds.

## Voice datagram

Each QUIC datagram carries one voice frame: a fixed 16-byte header
followed by the payload. Rationale is in
[ADR-0006](decisions/0006-voice-datagram-header.md).

| Offset | Size | Field | Meaning |
| --- | --- | --- | --- |
| 0 | 1 | Version | Datagram format version. Currently `1`. |
| 1 | 1 | Flags | Bit 0: end of transmission. Bits 1-7 reserved. |
| 2 | 4 | Group ID | Talkgroup or private-call identifier |
| 6 | 4 | Sender radio ID | Radio ID of the talker |
| 10 | 2 | Transmission ID | Incremented on each key-up; wraps |
| 12 | 2 | Sequence | Starts at 0 at key-up; +1 per datagram sent |
| 14 | 2 | Timestamp | Starts at 0 at key-up; counts 20 ms frames |
| 16 | rest | Payload | One Opus frame, encrypted with the group key if scoped |

Rules:

- Senders set reserved flag bits to 0. Receivers ignore them.
- Receivers drop datagrams shorter than 16 bytes, and datagrams whose
  version they do not support.
- With DTX, frames are not sent during silence, so the timestamp can
  jump ahead while the sequence stays contiguous. Sequence gaps mean
  loss; timestamp gaps alone do not.
- A datagram with the end-of-transmission flag set may have an empty
  payload.
- How radio IDs derive from the Ed25519 key and how group IDs are
  assigned are defined with identity and talkgroups (see Connection
  setup and Control messages).

## Control messages

Rationale is in [ADR-0010](decisions/0010-control-stream-and-floor.md).
Each message is a frame:

| Size | Field |
| --- | --- |
| 2 | Message type |
| 2 | Body length in bytes |
| length | Body |

Receivers skip frames with unknown types, and ignore body bytes after
the fields they know, so minor versions can add messages and append
fields. Text is a 1-byte length followed by UTF-8.

| Type | Message | Body |
| --- | --- | --- |
| `0x0001` | `HELLO` | minor version (2), radio ID (4), capabilities (4), name |
| `0x0010` | `JOIN` | group ID (4), proof length (1: 0 or 32), proof |
| `0x0011` | `LEAVE` | group ID (4) |
| `0x0020` | `KEYUP` | group ID (4), transmission ID (2) |
| `0x0021` | `KEYDOWN` | group ID (4), transmission ID (2) |
| `0x0023` | `FLOOR_GRANT` | reserved for server mode |
| `0x0024` | `FLOOR_DENY` | reserved for server mode |

- **`HELLO`:** minor version is `0`; no capability bits are defined.
  The name is text of at most 32 bytes.
  The display name is not verified.
- **`JOIN` / `LEAVE`:** a node only sends a group's voice to peers that
  have joined it. Scoped groups carry the membership proof from
  [ADR-0009](decisions/0009-group-keys-prove-membership.md); open groups
  and All Call send proof length 0.
- **`KEYUP` / `KEYDOWN`:** bracket each transmission. The transmission
  ID matches the voice datagrams'.
- **Group IDs:** `0xFFFFFFFF` is All Call. `0` is reserved.

## Floor arbitration

Every node runs the same rules for each group, so peer-to-peer nodes
agree without a coordinator. Server mode (phase 5) makes the server the
arbiter using the reserved grant and deny messages.

States: **idle**, **transmitting** (this node), **receiving** (a peer),
**hang time**.

- **Local key-up:** allowed when idle, or during hang time if this node
  took part in the exchange. Otherwise refused with the bonk tone and
  nothing is sent. When allowed, send `KEYUP`, then voice.
- **Remote `KEYUP`:** taken when idle or in hang time. If the floor was
  taken less than **150 ms** ago (measured at this node), the lower
  radio ID wins; a node that loses while transmitting stops and plays
  the bonk tone. Later key-ups are ignored.
- **Voice without `KEYUP`:** voice on a free channel takes the floor,
  so a node that joins mid-transmission still hears it. Voice from
  anyone but the floor holder is dropped, as is voice from a
  transmission that already ended (its last datagrams can arrive after
  `KEYDOWN`, which travels on the stream).
- **`KEYDOWN`:** starts **hang time (3 s)**. During hang time only
  nodes that transmitted in the current exchange may key. An exchange
  is a run of transmissions less than the hang time apart.
- **Time-out timer (120 s):** the talker's node ends the transmission
  and sends `KEYDOWN`. Receivers drop a talker that exceeds the
  time-out by 5 s without a `KEYDOWN`.
- **Disconnect:** a peer that disconnects while holding the floor
  releases it.

## Discovery

TODO: mDNS service type (`_hangtime._udp`), TXT record fields, and the
rendezvous registration messages for server mode.
