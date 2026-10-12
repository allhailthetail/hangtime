# 0008: Persistent Ed25519 identity

- **Status:** Accepted (location changed by [0015](0015-config-folder-and-release-zip.md))
- **Date:** 2026-10-12

## Context

Phase 1 generated a new certificate on every run, so fingerprints and
radio IDs changed each time. Floor arbitration breaks ties by radio ID,
and trusting a peer only means something if its identity is stable.

## Options considered

- **Persistent Ed25519 keypair, certificate built from it:** stable
  fingerprint and radio ID, matches the identity design, small change.
- **Persist the phase 1 certificate as-is:** stable, but leaves the
  Ed25519 identity work for later and ties identity to certificate bytes.

## Decision

Each install has one Ed25519 keypair, created on first run (there is no
installer yet) and stored as PKCS#8 DER in `hangtime-identity.key` in
the user's config folder (`%APPDATA%\hangtime` on Windows). Each run
builds a self-signed certificate from that key.

- **Fingerprint:** SHA-256 of the raw 32-byte Ed25519 public key. It is
  taken from the key, not the certificate, because certificate bytes
  change between runs.
- **Radio ID:** the first four bytes of the fingerprint.
- Peers must present an Ed25519 certificate; others fail the handshake.
- The parrot test node uses a temporary identity so it never shares the
  install's radio ID.

## Consequences

- Fingerprints and radio IDs are stable across runs, which supersedes
  the "regenerated on each run" consequence of
  [0007](0007-phase-1-connection-setup.md).
- Builds before this change cannot connect: they present P-256
  certificates. This is folded into the `hangtime/2` protocol version.
- The key file is a secret. It is never committed and is readable only
  by its owner on Unix.
