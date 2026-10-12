//! Session layer shared by client and server: floor control, hang time,
//! and the time-out timer. No I/O; callers feed in events and time.

mod floor;
mod groups;

pub use floor::{Busy, Floor, FloorConfig, FloorEvent, FloorState, KeyUpOutcome};
pub use groups::Group;
