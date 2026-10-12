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

To add a decision, copy [template.md](template.md), 
number it sequentially, and add it to the table above.