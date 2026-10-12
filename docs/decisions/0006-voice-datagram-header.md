# 0006: Voice datagram header layout

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

Each voice datagram carries one 20 ms Opus frame, 50 times a second.
Receivers need the talkgroup, the sender, which key-up the frame
belongs to, its order, and when to play it. Phase 1 only uses ordering,
timing, and end of transmission, but adding fields later would bump
the protocol version.

## Options considered

- **Minimal phase 1 header (flags, sequence, timestamp):** smallest,
  but group and sender fields would force a version bump in phase 2.
- **Full fixed 16-byte header now:** 8 more bytes per packet for group
  and sender (about 3 kbps), and no layout change in phase 2.
- **3-byte radio IDs, as in DMR:** closer to real radios, but an ID
  shortened from a public key is far more likely to collide.

## Decision

A fixed 16-byte big-endian header: version (1), flags (1), group ID
(4), sender radio ID (4), transmission ID (2), sequence (2), timestamp
(2), followed by one Opus frame. The exact layout is in
[PROTOCOL.md](../PROTOCOL.md#voice-datagram).

## Consequences

- About 6.4 kbps of header overhead per stream at 50 packets a second.
- Sequence and timestamp reset at each key-up. As 16-bit counters they
  last about 21 minutes, well past the maximum time-out timer.
- Group key encryption (phase 2) will authenticate the header and add
  a tag to the payload; the header layout does not need to change.
