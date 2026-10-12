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

TODO: QUIC ALPN identifier, certificate pinning to the Ed25519 key,
version negotiation, and the hello exchange (radio ID, capabilities,
display name).

## Voice datagram

TODO: final field sizes. Proposed fields:

| Field | Purpose |
| --- | --- |
| Version / flags | Format version; flags such as end-of-transmission |
| Group ID | Talkgroup or private-call identifier |
| Sender radio ID | Who is talking |
| Transmission ID | Distinguishes successive key-ups |
| Sequence number | Loss detection and ordering |
| Timestamp | Jitter buffer timing, in 20 ms frame units |
| Payload | One Opus frame, encrypted with the group key if scoped |

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