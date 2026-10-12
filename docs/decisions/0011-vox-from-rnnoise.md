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
speech probability reaches a threshold, its level is clearly above the
room's noise floor, and its level reaches a minimum. VOX keys after a
run of voice frames and unkeys after 25 quiet frames (500 ms hang).

The loudness check is what rejects taps. Measured on RNNoise, a table
tap is loud for one 20 ms frame, but the speech estimate jumps on it
and stays raised for about 200 ms. Speech stays loud for many frames.
The noise floor drops at once to any quieter frame and rises 2.5 dB/s,
so it follows the room and learns a new steady noise such as a fan.

The minimum level is what rejects other people talking nearby. Their
speech is real speech, so RNNoise rates it as such, but at the mic it
is much quieter than the wearer's own voice. Headsets that gate their
background to digital silence (the WH-1000XM4 reads about -100 dBFS
between sounds) make the noise floor check pass for any sound, so the
minimum carries them.

Sensitivity has five levels:

| Level | Probability | Run | Above floor | Minimum |
| --- | --- | --- | --- | --- |
| 1 (least) | 0.90 | 100 ms | 15 dB | -24 dBFS |
| 2 | 0.85 | 80 ms | 12 dB | -30 dBFS |
| 3 (default) | 0.75 | 60 ms | 10 dB | -36 dBFS |
| 4 | 0.60 | 60 ms | 8 dB | -42 dBFS |
| 5 (most) | 0.50 | 40 ms | 6 dB | -48 dBFS |

Users adjust the level live with + and -, or set `vox_sensitivity` in
`hangtime.toml`. `hangtime mic-test` calibrates: it records 5 s of the
room and 5 s of the user talking, and recommends the level whose
minimum sits closest to halfway between the two.

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
- In office testing with a WH-1000XM4, colleagues talking in another
  room keyed VOX at level 3 before the minimum level existed. On that
  recording, levels 1 to 4 now never key; level 5 still does.
- Speakers can set off VOX; a headset avoids it. The floor refuses VOX
  while receiving, which limits echo loops.
