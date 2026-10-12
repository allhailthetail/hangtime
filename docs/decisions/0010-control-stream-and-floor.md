# 0010: Control stream and floor rules

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

Phase 2 needs floor control between peers: who may talk, collisions,
hang time, and the time-out timer. That needs a reliable control
channel and a message format. The draft protocol listed more messages
than the first version needs.

## Options considered

- **Full draft set:** `HELLO`, `PRESENCE`, `JOIN`, `LEAVE`, `KEYUP`,
  `KEYDOWN`, `FLOOR_STATE`, `FLOOR_GRANT`, `FLOOR_DENY`, `PING`, `PONG`.
- **Minimal set:** `HELLO`, `JOIN`, `LEAVE`, `KEYUP`, `KEYDOWN`, with
  grant and deny reserved for server mode.
- **Serialization library (CBOR, postcard)** versus hand-written fixed
  fields.

## Decision

Use the minimal set with hand-written fixed binary fields, framed as
`type (u16) | length (u16) | body` on one bidirectional QUIC stream per
connection. Bump the protocol to `hangtime/2`.

- `PING`/`PONG` are dropped: QUIC already does keepalive and RTT.
- `PRESENCE` is dropped: `HELLO` and `JOIN` cover it.
- `FLOOR_STATE` is dropped: a node that joins mid-transmission takes the
  floor holder from the voice datagrams.

Floor rules: lower radio ID wins key-ups within 150 ms; 3 s hang time
reserved for the exchange's participants; 120 s time-out enforced by
the talker, with receivers dropping a stuck talker 5 s later.

## Consequences

- Five messages to implement and test; fewer states to get wrong.
- Fixed fields keep encoding allocation-light and the format readable
  in `PROTOCOL.md` without a schema language.
- Forward compatibility relies on skipping unknown types and ignoring
  trailing bytes.
- Hang time and time-out values are fixed for now; they become
  settings with the tray app.
