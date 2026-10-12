# Architecture decision records

Each file records one decision: the context, the options considered,
what was chosen, and why. Settled decisions should not be reopened
without writing a new ADR that supersedes the old one.

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-rust-single-binary.md) | Rust, single binary for client and server | Accepted |
| [0002](0002-quic-transport.md) | QUIC with datagrams for transport | Accepted |
| [0003](0003-opus-codec.md) | Opus for voice | Accepted |
| [0004](0004-floor-control.md) | One talker per talkgroup | Accepted |
| [0005](0005-slint-ui-toolkit.md) | Slint for UI Toolkit | Proposed |
| [0006](0006-voice-datagram-header.md) | Voice datagram header layout | Accepted |
| [0007](0007-phase-1-connection-setup.md) | Phase 1 connection setup and peer trust | Accepted |
| [0008](0008-persistent-ed25519-identity.md) | Persistent Ed25519 identity | Accepted |
| [0009](0009-group-keys-prove-membership.md) | Group keys prove membership | Accepted |

To add a decision, copy [template.md](template.md),
number it sequentially, and add it to the table above.
