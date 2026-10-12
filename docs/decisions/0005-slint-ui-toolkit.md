# 0005: Slint for UI toolkit

- **Status:** Proposed
- **Date:** 2026-10-12

## Context

Hangtime is tray-first. Its window is opened occasionally, created on
demand, and destroyed when hidden, so idle memory stays low (target:
under about 30 MB with the window closed).

## Options considered

- **Slint:** native Rust UI toolkit with declarative markup, a small
  runtime, and no webview.
- **Tauri 2:** web UI in the system webview with familiar tooling, but
  it pulls in a webview stack and a JavaScript frontend.

## Decision

Proposed: use Slint for the window. Confirm when phase 4 (tray app)
starts.

## Consequences

- No webview dependency.
- Slint's license terms need checking once Hangtime's own license is
  chosen.
