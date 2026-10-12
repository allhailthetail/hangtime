# 0012: Scan list monitors all groups, plays the selected one

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

The architecture planned a scan list that mixes audio from every
monitored talkgroup. In practice two conversations at once are hard to
follow, and the user wanted to see activity on other groups without
hearing it.

## Options considered

- **Mix all monitored groups:** never miss a word, but overlapping
  conversations are hard to follow and need a mixer with several jitter
  buffers.
- **Play only the selected group (and All Call), show the rest:** one
  stream to play, as today, plus a cheap level meter per group.

## Decision

The radio joins every configured group with each peer and keeps a floor
per group, so it always knows who is talking where. Only the selected
group and All Call are audible, one transmission at a time. Every
group's row shows its current talker and a level bar, measured by
decoding the incoming Opus frames. Selecting a group is a local switch:
no `JOIN` or `LEAVE`.

Activity on an inaudible group does not block transmitting on the
selected one; activity on an audible group does.

While VOX is on, a mic bar shows the user's level with a marker at the
VOX minimum for the current sensitivity.

## Consequences

- Peers send each node voice for every group it has joined, even ones
  it isn't playing. Voice is small, so this is cheap on a LAN.
- No mixer: the receive path still plays one stream.
- Bars redraw about 15 times a second only while one is moving, so an
  idle radio still does no work.
- All Call stays audible regardless of selection, as the channel for
  everyone. Making it follow the selection would be a small change.
