# 0002: QUIC with datagrams for transport

- **Status:** Accepted
- **Date:** 2026-10-12

## Context

Voice needs low latency and must never wait on retransmission, since a
late frame is useless. Control messages (key-up, floor grants,
presence) must arrive reliably and in order. Both should share one
encrypted connection and one port, to keep firewall rules and NAT
traversal simple.

## Options considered

- **QUIC (`quinn`) with RFC 9221 datagrams:** unreliable datagrams for
  voice and reliable streams for control on one connection, TLS 1.3
  built in, one UDP port.
- **Raw UDP with a custom reliability layer:** least overhead, but
  reimplements encryption, congestion control, and reliable delivery.
- **RTP/SRTP with a separate signaling channel:** the VoIP standard,
  but two transports and extra key exchange machinery.
- **WebRTC:** handles NAT traversal, but is a large dependency aimed at
  browsers and full-duplex conferencing.

## Decision

Use QUIC via `quinn`. Voice frames go in QUIC datagrams; control and
signaling go on QUIC streams. Certificates are self-signed and pinned
to the node's Ed25519 key.

## Consequences

- Voice is never sent on a reliable stream.
- Each node listens on one UDP port.
- No PKI to run, but key pinning and trust-on-first-use need their own
  design.
- IP, UDP, and QUIC headers roughly double the wire bitrate relative to
  the Opus bitrate.
