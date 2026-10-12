# 0003: Opus for voice

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

Hangtime needs a low-latency, low-bitrate voice codec that copes with
packet loss on Wi-Fi and the internet.

## Options considered

- **Opus:** open, royalty-free, built for interactive speech, with
  in-band FEC, packet loss concealment, and DTX.
- **Codec2 or AMBE-style vocoders:** closer to real DMR audio, but
  lower quality, and AMBE is patent-encumbered.
- **Uncompressed PCM:** simplest, but far too much bandwidth for
  peer-to-peer fan-out.

## Decision

Use Opus: 48 kHz mono, 20 ms frames, voice mode, roughly 16-24 kbps,
in-band FEC on, DTX on.

## Consequences

- The jitter buffer relies on Opus FEC and loss concealment to fill
  gaps.
- With DTX, silent frames are not sent, so the voice datagram header
  carries both a sequence number and a timestamp
  ([0006](0006-voice-datagram-header.md)).
- The `opus` crate builds libopus from C, so builds need a C compiler
  and CMake. Its bundled CMake script needs
  `CMAKE_POLICY_VERSION_MINIMUM=3.5` under CMake 4, set in
  `.cargo/config.toml`.
