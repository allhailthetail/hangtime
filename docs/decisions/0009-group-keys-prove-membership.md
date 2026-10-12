# 0009: Group keys prove membership

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

The architecture said group keys encrypt a scoped talkgroup's voice so
that peers without the key cannot decrypt it. In peer-to-peer mode a
talker sends voice directly to each member over QUIC, which is already
encrypted end to end. There is no intermediary, so keeping
non-members out reduces to deciding who gets sent the voice.

## Options considered

- **Encrypt voice payloads with the group key now:** needs per-
  transmission keys and nonce management on top of QUIC's encryption,
  for no gain while traffic is direct.
- **Use the key to prove membership when joining:** one HMAC per join,
  no change to voice datagrams.

## Decision

`JOIN` carries a membership proof:
`HMAC-SHA256(group key, "hangtime-join" || group ID || both peers'
fingerprints)`. A peer only sends a group's voice to peers whose proof
verifies. Binding both fingerprints means a captured proof cannot be
replayed to another peer. Open groups and All Call carry no proof.

Encrypting voice payloads with the group key moves to phase 5, when a
relay server sits between peers and must not hear the audio.

## Consequences

- Voice datagrams are unchanged in phase 2.
- A member who has the key can still forward voice to non-members;
  that is true of any shared-key scheme.
- Phase 5 adds payload encryption for relayed traffic.
