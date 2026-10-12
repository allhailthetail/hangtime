# 0007: Phase 1 connection setup and peer trust

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

Phase 1 needs two nodes to open a QUIC connection and exchange voice
datagrams. The design pins certificates to each node's Ed25519 identity
key, but identities are not built yet. Phase 1 has no control messages:
the voice datagram header already carries the sender and an
end-of-transmission flag.

## Options considered

- **Minimal setup:** ALPN `hangtime/1`, mutual TLS with self-signed
  certificates accepted as untrusted, QUIC datagrams enabled. Defer
  `HELLO` and control stream framing to phase 2.
- **Full setup now:** also define `HELLO` and control framing. Designs
  framing around one message before the phase 2 control messages are
  known.
- **Minimal Ed25519 pinning now:** closer to the final design, but
  pulls identity work forward from a later phase.

## Decision

Use the minimal setup. Accept any certificate, identify peers by the
SHA-256 fingerprint of their certificate, and label them untrusted
until the user confirms them. Confirming is a deliberate step (the user
checks the fingerprint with the other person, then accepts a
confirmation prompt); in phase 1 the terminal only shows the label and
fingerprint. Until Ed25519 identities exist, a node's radio ID is the
first four bytes of its certificate fingerprint.

## Consequences

- Phase 1 connections are encrypted but not authenticated: anyone
  reachable can connect. Acceptable on a LAN for testing only.
- Certificates are regenerated on each run, so fingerprints change
  between runs until identities persist.
- The ALPN suffix carries the major version, so version negotiation
  needs no extra message.
- `HELLO` and control framing are designed in phase 2 with the other
  control messages.
