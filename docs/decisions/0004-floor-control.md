# 0004: One talker per talkgroup

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

Full-duplex conferencing needs mixing servers, aggressive echo
cancellation, and selective forwarding. DMR avoids that with floor
control: one transmitter per talkgroup at a time, and a second key-up
is refused.

## Options considered

- **Floor control (half duplex per talkgroup):** predictable bandwidth,
  no mixing within a group, a simple repeater, and the DMR feel the
  project is built around.
- **Full-duplex conferencing:** familiar from other voice apps, but
  brings back the complexity Hangtime is designed to avoid.

## Decision

A talkgroup has exactly one talker at a time. Peer-to-peer mode
resolves collisions with a deterministic tiebreak; in server mode the
server arbitrates, as a repeater does. Both modes use the same
messages.

## Consequences

- Receivers handle at most one stream per talkgroup. Mixing happens
  only across monitored groups in the scan list.
- The server forwards packets without decoding or mixing.
- Private calls may optionally be full duplex, since they have no
  shared floor.
