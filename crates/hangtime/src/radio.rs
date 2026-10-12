//! The radio: ties floor control to the transmitter, peers, and the
//! status line. One talkgroup (All Call) until talkgroups arrive.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use hangtime_audio::{Inbox, Tone, Transmitter};
use hangtime_proto::{ALL_CALL, ControlMessage, VoiceHeader};
use hangtime_session::{Floor, FloorConfig, FloorEvent, FloorState, KeyUpOutcome};
use hangtime_transport::Peer;
use tokio::sync::Notify;

use crate::ptt;

/// A connected peer and the groups it has joined.
struct Member {
    peer: Peer,
    groups: HashSet<u32>,
}

/// Connected peers. Shared by the radio and the transmit thread.
#[derive(Default)]
pub struct Members(Mutex<Vec<Member>>);

impl Members {
    /// Adds a connected peer and joins All Call with it.
    pub fn add(&self, peer: Peer) {
        let _ = peer.send_control(&ControlMessage::Join {
            group_id: ALL_CALL,
            proof: None,
        });
        self.0.lock().unwrap().push(Member {
            peer,
            groups: HashSet::new(),
        });
    }

    /// Sends a voice datagram to every peer in its group.
    pub fn send_voice(&self, datagram: Vec<u8>) {
        let Ok((header, _)) = VoiceHeader::decode(&datagram) else {
            return;
        };
        for member in self.0.lock().unwrap().iter() {
            if member.groups.contains(&header.group_id) {
                // Voice is unreliable by design; a failed send is a lost frame.
                let _ = member.peer.send_voice(datagram.clone());
            }
        }
    }

    fn remove(&self, radio_id: u32) {
        self.0
            .lock()
            .unwrap()
            .retain(|member| member.peer.radio_id() != radio_id);
    }

    fn set_joined(&self, radio_id: u32, group_id: u32, joined: bool) {
        for member in self.0.lock().unwrap().iter_mut() {
            if member.peer.radio_id() == radio_id {
                if joined {
                    member.groups.insert(group_id);
                } else {
                    member.groups.remove(&group_id);
                }
            }
        }
    }

    fn broadcast(&self, message: &ControlMessage) {
        for member in self.0.lock().unwrap().iter() {
            let _ = member.peer.send_control(message);
        }
    }

    fn name_of(&self, radio_id: u32) -> String {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|member| member.peer.radio_id() == radio_id)
            .map_or_else(
                || format!("{radio_id:08x}"),
                |member| member.peer.display_name().to_string(),
            )
    }
}

pub struct Radio {
    floor: Mutex<Floor>,
    members: Arc<Members>,
    transmitter: Transmitter,
    /// Plays tones through the receive thread.
    tones: Inbox,
    next_transmission: AtomicU16,
    /// Wakes the timer task when the floor's next deadline may have moved.
    deadline_changed: Notify,
}

impl Radio {
    pub fn new(
        me: u32,
        transmitter: Transmitter,
        members: Arc<Members>,
        tones: Inbox,
    ) -> Arc<Self> {
        // Start transmission IDs somewhere new each run, so receivers that
        // remember our last transmission don't ignore the first new one.
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos() as u16);
        Arc::new(Self {
            floor: Mutex::new(Floor::new(me, FloorConfig::default())),
            members,
            transmitter,
            tones,
            next_transmission: AtomicU16::new(seed),
            deadline_changed: Notify::new(),
        })
    }

    /// The local PTT key went down or up.
    pub fn ptt(&self, keyed: bool) {
        let now = Instant::now();
        let mut floor = self.floor.lock().unwrap();
        if keyed {
            let id = self.next_transmission.fetch_add(1, Ordering::Relaxed);
            if floor.request(id, now).is_ok() {
                self.transmitter.key_up(id);
                self.tones.tone(Tone::TalkPermit);
                self.members.broadcast(&ControlMessage::KeyUp {
                    group_id: ALL_CALL,
                    transmission_id: id,
                });
            } else {
                self.tones.tone(Tone::Bonk);
                ptt::set_status("[--] busy");
                return;
            }
        } else if let Some(id) = floor.release(now) {
            self.transmitter.key_down();
            self.members.broadcast(&ControlMessage::KeyDown {
                group_id: ALL_CALL,
                transmission_id: id,
            });
        }
        self.show(&floor);
        self.deadline_changed.notify_one();
    }

    /// Handles a control message from `peer`.
    pub fn on_control(&self, peer: &Peer, message: ControlMessage) {
        let now = Instant::now();
        let talker = peer.radio_id();
        let mut floor = self.floor.lock().unwrap();
        match message {
            ControlMessage::KeyUp {
                group_id: ALL_CALL,
                transmission_id,
            } => {
                if floor.on_key_up(talker, transmission_id, now) == KeyUpOutcome::Preempted {
                    self.transmitter.key_down();
                    self.tones.tone(Tone::Bonk);
                    ptt::notice("lost the floor to a simultaneous key-up");
                }
            }
            ControlMessage::KeyDown {
                group_id: ALL_CALL,
                transmission_id,
            } => floor.on_key_down(talker, transmission_id, now),
            ControlMessage::Join { group_id, .. } => {
                self.members.set_joined(talker, group_id, true);
            }
            ControlMessage::Leave { group_id } => self.members.set_joined(talker, group_id, false),
            _ => return,
        }
        self.show(&floor);
        self.deadline_changed.notify_one();
    }

    /// A voice datagram arrived. Returns true if it should be played.
    pub fn on_voice(&self, header: &VoiceHeader) -> bool {
        if header.group_id != ALL_CALL {
            return false;
        }
        let mut floor = self.floor.lock().unwrap();
        let before = floor.state();
        let play = floor.on_voice(header.sender, header.transmission_id, Instant::now());
        if floor.state() != before {
            self.show(&floor);
            self.deadline_changed.notify_one();
        }
        play
    }

    /// A peer disconnected.
    pub fn on_peer_lost(&self, peer: &Peer) {
        self.members.remove(peer.radio_id());
        let mut floor = self.floor.lock().unwrap();
        floor.on_peer_lost(peer.radio_id(), Instant::now());
        self.show(&floor);
        self.deadline_changed.notify_one();
        ptt::notice(&format!("{} disconnected", peer.display_name()));
    }

    /// Runs floor timers (hang time, time-out). Sleeps with no deadline
    /// while the channel is idle.
    pub async fn run_timers(self: Arc<Self>) {
        loop {
            let deadline = self.floor.lock().unwrap().next_deadline();
            match deadline {
                Some(deadline) => {
                    tokio::select! {
                        _ = tokio::time::sleep_until(deadline.into()) => self.tick(),
                        _ = self.deadline_changed.notified() => {}
                    }
                }
                None => self.deadline_changed.notified().await,
            }
        }
    }

    fn tick(&self) {
        let mut floor = self.floor.lock().unwrap();
        match floor.tick(Instant::now()) {
            Some(FloorEvent::TimedOut { transmission_id }) => {
                self.transmitter.key_down();
                self.members.broadcast(&ControlMessage::KeyDown {
                    group_id: ALL_CALL,
                    transmission_id,
                });
                self.tones.tone(Tone::Bonk);
                ptt::notice("time-out: transmission ended");
            }
            Some(FloorEvent::HangEnded) => self.tones.tone(Tone::HangEnd),
            Some(FloorEvent::TalkerDropped { .. }) | None => {}
        }
        self.show(&floor);
    }

    fn show(&self, floor: &Floor) {
        let text = match floor.state() {
            FloorState::Idle => "[  ] idle".to_string(),
            FloorState::Transmitting { .. } => "[TX] transmitting".to_string(),
            FloorState::Receiving { talker, .. } => {
                format!("[RX] {}", self.members.name_of(talker))
            }
            FloorState::Hang { .. } => "[..] hang time".to_string(),
        };
        ptt::set_status(&text);
    }
}
