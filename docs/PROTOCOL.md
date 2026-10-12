# Hangtime wire protocol

> **Status:** draft skeleton. Nothing here is final. Fill in each
> section before implementing the code that depends on it.

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

- **ALPN:** `hangtime/1`. The suffix is the protocol major version, so
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

TODO: the `HELLO` exchange (radio ID, capabilities, display name) and
control stream framing, designed together with the other control
messages in phase 2.

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

TODO: encoding and fields for each message. Initial set:

- `HELLO`: identity, version, capabilities
- `PRESENCE`: online, monitored groups, selected group
- `KEYUP`: request the floor for a group
- `KEYDOWN`: release the floor; starts hang time
- `FLOOR_GRANT` / `FLOOR_DENY`: server mode, or collision resolution
- `FLOOR_STATE`: state sync for peers joining mid-transmission
- `JOIN` / `LEAVE`: group membership
- `PING` / `PONG`: keepalive and latency measurement

## Floor arbitration state machine

TODO: states (idle, transmitting, receiving, hang time, denied),
transitions, the collision window, the tiebreak rule, and time-out
timer behavior, for both peer-to-peer and server modes.

## Discovery

TODO: mDNS service type (`_hangtime._udp`), TXT record fields, and the
rendezvous registration messages for server mode.
