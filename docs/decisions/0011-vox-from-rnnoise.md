# 0011: VOX from RNNoise's speech estimate

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

Phase 3 adds noise suppression and voice-operated transmit (VOX). VOX
needs a voice activity detector that ignores keyboard noise, fans, and
breathing, and keys quickly enough that pre-roll can cover the start of
speech.

## Options considered

- **Energy threshold:** trivial, but keys on any loud noise and misses
  quiet speakers. Needs per-user tuning.
- **Separate VAD (WebRTC VAD):** good quality, but a C library and a
  second analysis pass.
- **RNNoise's speech probability:** RNNoise (`nnnoiseless`, pure Rust)
  already runs for noise suppression and reports, per 10 ms frame, how
  likely the frame is speech.

## Decision

Run RNNoise on every transmit frame. A frame counts as voice when its
speech probability reaches a threshold and its level is clearly above
the room's noise floor. VOX keys after a run of voice frames and
unkeys after 25 quiet frames (500 ms hang).

The loudness check is what rejects taps. Measured on RNNoise, a table
tap is loud for one 20 ms frame, but the speech estimate jumps on it
and stays raised for about 200 ms. Speech stays loud for many frames.
The noise floor drops at once to any quieter frame and rises 2.5 dB/s,
so it follows the room and learns a new steady noise such as a fan.

Sensitivity has five levels:

| Level | Probability | Run | Above floor |
| --- | --- | --- | --- |
| 1 (least) | 0.90 | 100 ms | 15 dB |
| 2 | 0.85 | 80 ms | 12 dB |
| 3 (default) | 0.75 | 60 ms | 10 dB |
| 4 | 0.60 | 60 ms | 8 dB |
| 5 (most) | 0.50 | 40 ms | 6 dB |

Users adjust the level live with + and -, or set `vox_sensitivity` in
`hangtime.toml`.

On a VOX key-up, send the last 5 cleaned frames (100 ms of pre-roll)
first. VOX key-ups follow the same floor rules as PTT but skip the
talk-permit chirp and are refused silently on a busy channel.

## Consequences

- One analysis pass serves both noise suppression and VOX; no extra
  dependency or C code.
- RNNoise adds about 10 ms of latency to all transmissions.
- Pre-roll adds about 100 ms to VOX transmissions: the pre-roll frames
  arrive in a burst and play at normal speed, so the receiver's buffer
  stays that much deeper. PTT transmissions have no pre-roll.
- With VOX on, the transmit thread runs continuously (RNNoise on every
  frame) instead of sleeping between transmissions.
- Table taps set off the first version (speech probability only, 40 ms
  at 0.5), and requiring longer runs alone did not help because the
  estimate lingers after a tap. The loudness check addresses that.
- Speakers can set off VOX; a headset avoids it. The floor refuses VOX
  while receiving, which limits echo loops.
