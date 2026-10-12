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

Run RNNoise on every transmit frame and use its speech probability for
VOX: key after a run of frames at or above a threshold, unkey after 25
quiet frames (500 ms hang). Sensitivity has five levels, from 0.95 for
100 ms (least sensitive) to 0.50 for 40 ms (most); the default, level
3, is 0.80 for 60 ms. Requiring a run rejects transients: a tap or
click can briefly score as speech, but a syllable lasts well over
100 ms. Users adjust the level live with + and -, or set
`vox_sensitivity` in `hangtime.toml`.

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
- The first version keyed after 40 ms at 0.5 and was set off by table
  taps in testing; that is now level 5.
- Speakers can set off VOX; a headset avoids it. The floor refuses VOX
  while receiving, which limits echo loops.
