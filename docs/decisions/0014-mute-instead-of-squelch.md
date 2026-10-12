# 0014: Per-group mute instead of a squelch level

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

The architecture planned a receive-side squelch: suppress streams below
a level threshold, and apply per-group mute and monitor settings. On an
analog radio, squelch hides the hiss of an empty channel. Hangtime has
no empty-channel hiss: a peer only sends while keyed (PTT or VOX), and
its audio is already noise-suppressed. The scan list (ADR-0012) already
covers monitoring: every group shows activity, and only the selected
group and All Call play.

## Options considered

- **Level-threshold squelch:** gates received audio below a level. With
  nothing to remove, it would mostly clip quiet talkers.
- **Per-group mute:** a muted group shows activity but never plays. It
  covers the remaining gap: silencing All Call, or a busy group you
  still want to see.

## Decision

No level threshold. Press M to mute or unmute the selected group,
including All Call and Parrot. A muted group keeps its row, talker
name, and (greyed) level bar, but is never played, even when selected,
and doesn't stop you transmitting on another group. You can still
transmit on a muted group.

Mutes start from `muted = ["All Call", ...]` in `hangtime.toml` (group
names, ignoring case). Pressing M changes them for the current run and
prints the line to put in the file to keep them, as VOX sensitivity
does.

## Consequences

- Phase 3 is complete without a squelch stage in the receive path.
- All Call stays audible by default and can now be muted, which settles
  the question left open in ADR-0012.
- Hangtime doesn't rewrite `hangtime.toml`, so the user's comments and
  layout are never lost; the price is copying the printed line by hand.
