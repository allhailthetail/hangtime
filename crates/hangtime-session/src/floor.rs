//! Floor control for one talkgroup: one talker at a time (ADR-0004).
//!
//! Pure state machine with time passed in, so the same rules run on every
//! node and can be tested exactly. Rules (ADR-0010):
//!
//! - Keying while someone else holds the floor is refused (bonk).
//! - Two key-ups within the collision window: lower radio ID wins.
//! - After a key-down, hang time reserves the floor for the people in
//!   that exchange.
//! - The talker's own node enforces the time-out timer. Receivers give up
//!   on a stuck talker after the time-out plus a margin.

use std::time::{Duration, Instant};

/// Timing rules for the floor.
#[derive(Debug, Clone, Copy)]
pub struct FloorConfig {
    pub collision_window: Duration,
    pub hang_time: Duration,
    pub time_out: Duration,
}

impl Default for FloorConfig {
    fn default() -> Self {
        Self {
            collision_window: Duration::from_millis(150),
            hang_time: Duration::from_secs(3),
            time_out: Duration::from_secs(120),
        }
    }
}

/// Extra time receivers allow past the time-out before dropping a talker
/// that never sent a key-down.
const RECEIVE_MARGIN: Duration = Duration::from_secs(5);

/// Who holds the floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorState {
    Idle,
    /// This node is transmitting.
    Transmitting {
        transmission_id: u16,
        since: Instant,
    },
    /// Another node is transmitting.
    Receiving {
        talker: u32,
        transmission_id: u16,
        since: Instant,
    },
    /// Reserved for the last exchange's participants until `until`.
    Hang {
        until: Instant,
    },
}

/// The local user can't key: someone else has the floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Busy;

/// Result of a remote key-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyUpOutcome {
    /// The remote talker now holds the floor.
    Accepted,
    /// The remote talker won a collision with this node: stop
    /// transmitting and play the bonk tone.
    Preempted,
    /// The floor stays where it was.
    Ignored,
}

/// Something the caller must act on after [`Floor::tick`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorEvent {
    /// This node hit the time-out timer: stop transmitting, send key-down.
    TimedOut { transmission_id: u16 },
    /// Hang time ended; the floor is free.
    HangEnded,
    /// A remote talker exceeded the time-out without a key-down.
    TalkerDropped { talker: u32 },
}

/// Floor state for one talkgroup on one node.
#[derive(Debug)]
pub struct Floor {
    me: u32,
    config: FloorConfig,
    state: FloorState,
    /// Radio IDs that transmitted in the current exchange.
    participants: Vec<u32>,
}

impl Floor {
    pub fn new(me: u32, config: FloorConfig) -> Self {
        Self {
            me,
            config,
            state: FloorState::Idle,
            participants: Vec::new(),
        }
    }

    pub fn state(&self) -> FloorState {
        self.state
    }

    /// The local user keyed. On `Ok` the caller sends `KEYUP` and starts
    /// transmitting.
    pub fn request(&mut self, transmission_id: u16, now: Instant) -> Result<(), Busy> {
        match self.state {
            FloorState::Receiving { .. } => return Err(Busy),
            FloorState::Hang { .. } if !self.participants.contains(&self.me) => return Err(Busy),
            FloorState::Transmitting { .. } => return Ok(()),
            FloorState::Idle | FloorState::Hang { .. } => {}
        }
        self.take(self.me);
        self.state = FloorState::Transmitting {
            transmission_id,
            since: now,
        };
        Ok(())
    }

    /// The local user unkeyed. Returns the transmission to send `KEYDOWN`
    /// for, if this node was transmitting.
    pub fn release(&mut self, now: Instant) -> Option<u16> {
        let FloorState::Transmitting {
            transmission_id, ..
        } = self.state
        else {
            return None;
        };
        self.start_hang(now);
        Some(transmission_id)
    }

    /// A peer sent `KEYUP`.
    pub fn on_key_up(&mut self, talker: u32, transmission_id: u16, now: Instant) -> KeyUpOutcome {
        let within_window =
            |since: Instant| now.duration_since(since) <= self.config.collision_window;
        match self.state {
            FloorState::Idle | FloorState::Hang { .. } => {}
            FloorState::Receiving {
                talker: current,
                since,
                ..
            } => {
                let collision_won = within_window(since) && talker < current;
                if talker != current && !collision_won {
                    return KeyUpOutcome::Ignored;
                }
            }
            FloorState::Transmitting { since, .. } => {
                if !(within_window(since) && talker < self.me) {
                    return KeyUpOutcome::Ignored;
                }
                self.receive(talker, transmission_id, now);
                return KeyUpOutcome::Preempted;
            }
        }
        self.receive(talker, transmission_id, now);
        KeyUpOutcome::Accepted
    }

    /// A peer sent `KEYDOWN`.
    pub fn on_key_down(&mut self, talker: u32, transmission_id: u16, now: Instant) {
        if let FloorState::Receiving {
            talker: current,
            transmission_id: current_id,
            ..
        } = self.state
            && (current, current_id) == (talker, transmission_id)
        {
            self.start_hang(now);
        }
    }

    /// A voice datagram arrived. Returns true if it should be played.
    /// Voice on a free channel takes the floor, so a node that joins (or
    /// whose `KEYUP` arrives late) still hears the talker.
    pub fn on_voice(&mut self, talker: u32, transmission_id: u16, now: Instant) -> bool {
        match self.state {
            FloorState::Receiving {
                talker: current,
                transmission_id: current_id,
                ..
            } => (current, current_id) == (talker, transmission_id),
            FloorState::Idle | FloorState::Hang { .. } => {
                self.receive(talker, transmission_id, now);
                true
            }
            FloorState::Transmitting { .. } => false,
        }
    }

    /// A peer disconnected. Frees the floor if it was talking.
    pub fn on_peer_lost(&mut self, talker: u32, now: Instant) {
        if let FloorState::Receiving {
            talker: current, ..
        } = self.state
            && current == talker
        {
            self.start_hang(now);
        }
    }

    /// When [`Floor::tick`] next needs to run, if ever.
    pub fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            FloorState::Idle => None,
            FloorState::Hang { until } => Some(until),
            FloorState::Transmitting { since, .. } => Some(since + self.config.time_out),
            FloorState::Receiving { since, .. } => {
                Some(since + self.config.time_out + RECEIVE_MARGIN)
            }
        }
    }

    /// Applies any timers that have expired by `now`.
    pub fn tick(&mut self, now: Instant) -> Option<FloorEvent> {
        if self.next_deadline().is_none_or(|deadline| now < deadline) {
            return None;
        }
        match self.state {
            FloorState::Idle => None,
            FloorState::Hang { .. } => {
                self.state = FloorState::Idle;
                self.participants.clear();
                Some(FloorEvent::HangEnded)
            }
            FloorState::Transmitting {
                transmission_id, ..
            } => {
                self.start_hang(now);
                Some(FloorEvent::TimedOut { transmission_id })
            }
            FloorState::Receiving { talker, .. } => {
                self.start_hang(now);
                Some(FloorEvent::TalkerDropped { talker })
            }
        }
    }

    fn receive(&mut self, talker: u32, transmission_id: u16, now: Instant) {
        self.take(talker);
        self.state = FloorState::Receiving {
            talker,
            transmission_id,
            since: now,
        };
    }

    fn take(&mut self, talker: u32) {
        if !self.participants.contains(&talker) {
            self.participants.push(talker);
        }
    }

    fn start_hang(&mut self, now: Instant) {
        self.state = FloorState::Hang {
            until: now + self.config.hang_time,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 50;
    const LOW: u32 = 10;
    const HIGH: u32 = 90;

    fn floor() -> (Floor, Instant) {
        (Floor::new(ME, FloorConfig::default()), Instant::now())
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn keys_on_idle_channel_and_hangs_after_release() {
        let (mut floor, t) = floor();
        assert_eq!(floor.request(1, t), Ok(()));
        assert_eq!(floor.release(t + ms(500)), Some(1));
        assert!(matches!(floor.state(), FloorState::Hang { .. }));
        assert_eq!(
            floor.tick(t + ms(500) + Duration::from_secs(3)),
            Some(FloorEvent::HangEnded)
        );
        assert_eq!(floor.state(), FloorState::Idle);
    }

    #[test]
    fn refuses_key_while_receiving() {
        let (mut floor, t) = floor();
        assert_eq!(floor.on_key_up(HIGH, 1, t), KeyUpOutcome::Accepted);
        assert_eq!(floor.request(1, t + ms(10)), Err(Busy));
    }

    #[test]
    fn collision_lower_radio_id_wins() {
        let (mut floor, t) = floor();
        floor.request(1, t).unwrap();
        assert_eq!(floor.on_key_up(HIGH, 1, t + ms(50)), KeyUpOutcome::Ignored);
        assert_eq!(floor.on_key_up(LOW, 1, t + ms(60)), KeyUpOutcome::Preempted);
        assert!(matches!(
            floor.state(),
            FloorState::Receiving { talker: LOW, .. }
        ));
    }

    #[test]
    fn late_key_up_does_not_preempt() {
        let (mut floor, t) = floor();
        floor.request(1, t).unwrap();
        assert_eq!(floor.on_key_up(LOW, 1, t + ms(500)), KeyUpOutcome::Ignored);
        assert!(matches!(floor.state(), FloorState::Transmitting { .. }));
    }

    #[test]
    fn receivers_pick_the_same_collision_winner() {
        let (mut floor, t) = floor();
        floor.on_key_up(HIGH, 1, t);
        assert_eq!(floor.on_key_up(LOW, 1, t + ms(100)), KeyUpOutcome::Accepted);
        assert_eq!(floor.on_key_up(HIGH, 2, t + ms(120)), KeyUpOutcome::Ignored);
        assert!(matches!(
            floor.state(),
            FloorState::Receiving { talker: LOW, .. }
        ));
    }

    #[test]
    fn hang_time_reserves_floor_for_participants() {
        let (mut floor, t) = floor();
        floor.on_key_up(HIGH, 1, t);
        floor.on_key_down(HIGH, 1, t + ms(500));
        // This node was not part of the exchange.
        assert_eq!(floor.request(1, t + ms(600)), Err(Busy));
        // The previous talker can reply into hang time.
        assert_eq!(
            floor.on_key_up(HIGH, 2, t + ms(700)),
            KeyUpOutcome::Accepted
        );
    }

    #[test]
    fn participant_can_reply_during_hang_time() {
        let (mut floor, t) = floor();
        floor.request(1, t).unwrap();
        floor.release(t + ms(500));
        floor.on_key_up(HIGH, 1, t + ms(800));
        floor.on_key_down(HIGH, 1, t + ms(1500));
        assert_eq!(floor.request(2, t + ms(1600)), Ok(()));
    }

    #[test]
    fn voice_on_free_channel_takes_floor() {
        let (mut floor, t) = floor();
        assert!(floor.on_voice(HIGH, 3, t));
        assert!(!floor.on_voice(LOW, 1, t + ms(10)));
        assert!(floor.on_voice(HIGH, 3, t + ms(20)));
    }

    #[test]
    fn does_not_play_voice_while_transmitting() {
        let (mut floor, t) = floor();
        floor.request(1, t).unwrap();
        assert!(!floor.on_voice(HIGH, 1, t + ms(10)));
    }

    #[test]
    fn time_out_forces_release() {
        let (mut floor, t) = floor();
        floor.request(7, t).unwrap();
        assert_eq!(floor.tick(t + Duration::from_secs(119)), None);
        assert_eq!(
            floor.tick(t + Duration::from_secs(120)),
            Some(FloorEvent::TimedOut { transmission_id: 7 })
        );
        assert_eq!(floor.release(t + Duration::from_secs(121)), None);
    }

    #[test]
    fn drops_talker_that_never_keys_down() {
        let (mut floor, t) = floor();
        floor.on_key_up(HIGH, 1, t);
        assert_eq!(
            floor.tick(t + Duration::from_secs(125)),
            Some(FloorEvent::TalkerDropped { talker: HIGH })
        );
    }

    #[test]
    fn lost_peer_frees_floor() {
        let (mut floor, t) = floor();
        floor.on_key_up(HIGH, 1, t);
        floor.on_peer_lost(HIGH, t + ms(10));
        assert!(matches!(floor.state(), FloorState::Hang { .. }));
    }

    #[test]
    fn idle_floor_has_no_deadline() {
        let (floor, _) = floor();
        assert_eq!(floor.next_deadline(), None);
    }
}
