//! Wire format shared by client and server.
//!
//! `docs/PROTOCOL.md` is the source of truth for everything in this
//! crate. Change that document first, and bump the protocol version for
//! any incompatible change.

mod control;
mod voice;

pub use control::{
    ControlError, ControlMessage, FRAME_HEADER_LEN, MAX_DISPLAY_NAME, MINOR_VERSION, PROOF_LEN,
};
pub use voice::{DecodeError, FRAME_DURATION, VOICE_HEADER_LEN, VOICE_VERSION, VoiceHeader};

/// Group ID for All Call: every reachable peer.
pub const ALL_CALL: u32 = 0xFFFF_FFFF;
