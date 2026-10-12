//! Wire format shared by client and server.
//!
//! `docs/PROTOCOL.md` is the source of truth for everything in this
//! crate. Change that document first, and bump the protocol version for
//! any incompatible change.

mod voice;

pub use voice::{DecodeError, FRAME_DURATION, VOICE_HEADER_LEN, VOICE_VERSION, VoiceHeader};
