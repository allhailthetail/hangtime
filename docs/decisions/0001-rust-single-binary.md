# 0001: Rust, single binary for client and server

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

Hangtime needs real-time audio with no allocation or locking in the
audio path, a small idle footprint for a tray app, and a headless
server that speaks exactly the same protocol as the client.

## Options considered

- **Rust:** no garbage collector pauses in the audio path, mature
  cross-platform audio (`cpal`) and QUIC (`quinn`) crates, and a Cargo
  workspace makes sharing code between client and server natural.
- **Go:** simple concurrency and deployment, but GC pauses and cgo for
  audio and codecs complicate the real-time path.
- **C++:** full control, but harder to keep memory-safe and no standard
  package manager.
- **Separate client and server programs:** each could be smaller, but
  protocol and session code would drift or need a shared library
  anyway.

## Decision

Write Hangtime in Rust as one binary. It runs as the tray client by
default and as a headless repeater with `hangtime --server`.

## Consequences

- Protocol and session code live in shared crates used by both roles.
  Server-only logic lives in its own crate, not in forks of shared
  code.
- The server role must not need audio or UI at runtime.
- Native dependencies (libopus, TLS crypto) need a C toolchain at build
  time.
