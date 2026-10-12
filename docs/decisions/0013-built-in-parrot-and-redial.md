# 0013: Built-in parrot and staying connected

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

Testing your own microphone, VOX, and speaker meant starting a second
instance (`hangtime parrot`) and pointing the first at it. A parrot is a
basic radio feature and should always be there.

Separately, connections dropped after 30 s with nobody talking, because
QUIC's default idle timeout applied and nothing kept the link alive. A
dropped connection to a `--peer` stayed down until the user restarted.

## Options considered

- **Parrot as a local talkgroup:** the radio gets a "Parrot" row;
  transmissions on it are recorded locally and played back after
  unkeying. No network involved, so it tests exactly the user's own
  audio chain.
- **Run the network parrot inside the client on a loopback
  connection:** also exercises QUIC, but adds a second node, a second
  identity, and a connection to itself for no gain to the user.
- **Keep-alive and redial:** QUIC keep-alive pings on every connection;
  the dialing side redials a lost peer with backoff.

## Decision

The last row of the group list is **Parrot**, local only. It uses group
ID `0`, which is reserved on the wire, so no peer can join it and it is
never sent. Voice sent on it goes to a local recorder; 0.5 s after the
transmission ends, the recording replays as a transmission from the
parrot (talker ID `0`) through the same floor, hang time, level meter,
latency report, and receive path as a peer's. Replays queue and never
overlap. The headless `hangtime parrot` stays for testing across the
network.

Every connection pings every 3 s and is dropped after 10 s of silence.
The side that dialed redials a lost peer, after 1 s doubling to 30 s,
with jitter so two nodes that dial each other don't collide every
time. A `--peer` is redialed for as long as Hangtime runs; a LAN peer
found by mDNS is given up after about a minute, since it announces
itself again when it returns. If a redial finds the peer already
connected the other way, it waits for that connection to end.

## Consequences

- One instance is enough to hear yourself; select Parrot and talk.
- An idle link costs a small ping every 3 s per peer, and a dead peer
  is noticed within 10 s.
- A `--peer` that is down costs one connection attempt every 30 s.
