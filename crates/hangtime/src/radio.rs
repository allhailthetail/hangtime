//! The radio: ties floor control to the transmitter, peers, talkgroups,
//! and the status line.
//!
//! The radio monitors All Call plus the selected talkgroup, and transmits
//! on the selected one. Each monitored group has its own floor. Until the
//! scan list arrives, one received transmission plays at a time.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hangtime_audio::{Inbox, Tone, Transmitter};
use hangtime_proto::{ALL_CALL, ControlMessage, VoiceHeader};
use hangtime_session::{Floor, FloorConfig, FloorEvent, FloorState, Group, KeyUpOutcome};
use hangtime_transport::{Fingerprint, Peer};
use tokio::sync::Notify;

use crate::ptt;

/// A connected peer and the groups it has proven membership of.
struct Member {
    peer: Peer,
    groups: HashSet<u32>,
}

/// Connected peers. Shared by the radio and the transmit thread.
#[derive(Default)]
pub struct Members(Mutex<Vec<Member>>);

impl Members {
    /// True if a peer with this fingerprint (lowercase hex) is connected.
    pub fn has(&self, fingerprint: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|member| member.peer.fingerprint().hex() == fingerprint)
    }

    /// Round-trip time to a connected peer.
    pub fn rtt_of(&self, radio_id: u32) -> Option<Duration> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|member| member.peer.radio_id() == radio_id)
            .map(|member| member.peer.rtt())
    }

    /// Sends a voice datagram to every member of its group.
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

    /// Adds a peer. Returns false if one with the same identity exists.
    fn add(&self, peer: Peer) -> bool {
        let mut members = self.0.lock().unwrap();
        if members
            .iter()
            .any(|member| member.peer.radio_id() == peer.radio_id())
        {
            return false;
        }
        members.push(Member {
            peer,
            groups: HashSet::new(),
        });
        true
    }

    fn remove(&self, radio_id: u32) {
        self.0
            .lock()
            .unwrap()
            .retain(|member| member.peer.radio_id() != radio_id);
    }

    fn set_member(&self, radio_id: u32, group_id: u32, member_of: bool) {
        for member in self.0.lock().unwrap().iter_mut() {
            if member.peer.radio_id() == radio_id {
                if member_of {
                    member.groups.insert(group_id);
                } else {
                    member.groups.remove(&group_id);
                }
            }
        }
    }

    fn is_member(&self, radio_id: u32, group_id: u32) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|member| member.peer.radio_id() == radio_id && member.groups.contains(&group_id))
    }

    /// Sends a control message to every member of `group_id`.
    fn send_to_group(&self, group_id: u32, message: &ControlMessage) {
        for member in self.0.lock().unwrap().iter() {
            if member.groups.contains(&group_id) {
                let _ = member.peer.send_control(message);
            }
        }
    }

    /// Sends a control message to every peer.
    fn broadcast(&self, message: &ControlMessage) {
        for member in self.0.lock().unwrap().iter() {
            let _ = member.peer.send_control(message);
        }
    }

    /// Sends every peer a `JOIN` for `group`, with a proof made for that
    /// peer if the group has a key.
    fn join_all(&self, group: &Group, me: &Fingerprint) {
        for member in self.0.lock().unwrap().iter() {
            join(&member.peer, group, me);
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

fn join(peer: &Peer, group: &Group, me: &Fingerprint) {
    let proof = group.join_proof(me.as_bytes(), peer.fingerprint().as_bytes());
    let _ = peer.send_control(&ControlMessage::Join {
        group_id: group.id,
        proof,
    });
}

/// Floor state for the monitored groups.
struct State {
    /// Index into `Radio::groups` of the group to transmit on.
    selected: usize,
    floors: HashMap<u32, Floor>,
}

pub struct Radio {
    me: Fingerprint,
    /// All Call first, then the configured groups.
    groups: Vec<Group>,
    state: Mutex<State>,
    members: Arc<Members>,
    transmitter: Transmitter,
    /// Plays tones through the receive thread.
    tones: Inbox,
    next_transmission: AtomicU16,
    /// Wakes the timer task when a floor's next deadline may have moved.
    deadline_changed: Notify,
}

impl Radio {
    pub fn new(
        me: Fingerprint,
        configured: Vec<Group>,
        transmitter: Transmitter,
        members: Arc<Members>,
        tones: Inbox,
    ) -> Arc<Self> {
        // Start transmission IDs somewhere new each run, so receivers that
        // remember our last transmission don't ignore the first new one.
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos() as u16);
        let mut groups = vec![Group {
            id: ALL_CALL,
            name: "All Call".into(),
            key: None,
        }];
        groups.extend(configured);
        let floors = HashMap::from([(ALL_CALL, Floor::new(me.radio_id(), FloorConfig::default()))]);
        Arc::new(Self {
            me,
            groups,
            state: Mutex::new(State {
                selected: 0,
                floors,
            }),
            members,
            transmitter,
            tones,
            next_transmission: AtomicU16::new(seed),
            deadline_changed: Notify::new(),
        })
    }

    /// The groups in selection order: All Call, then configured groups.
    pub fn groups(&self) -> &[Group] {
        &self.groups
    }

    /// Registers a connected peer and joins the monitored groups with it.
    /// Returns false for a duplicate connection.
    pub fn add_peer(&self, peer: &Peer) -> bool {
        if !self.members.add(peer.clone()) {
            return false;
        }
        let state = self.state.lock().unwrap();
        for group_id in state.floors.keys() {
            if let Some(group) = self.group(*group_id) {
                join(peer, group, &self.me);
            }
        }
        true
    }

    /// The local PTT key went down or up.
    pub fn ptt(&self, keyed: bool) {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        let group = &self.groups[state.selected];
        if keyed {
            // Half duplex: no talking over a transmission on any group.
            let receiving_elsewhere = state.floors.iter().any(|(id, floor)| {
                *id != group.id && matches!(floor.state(), FloorState::Receiving { .. })
            });
            let id = self.next_transmission.fetch_add(1, Ordering::Relaxed);
            let floor = state
                .floors
                .get_mut(&group.id)
                .expect("selected group has a floor");
            if !receiving_elsewhere && floor.request(id, now).is_ok() {
                self.transmitter.key_up(id, group.id);
                self.tones.tone(Tone::TalkPermit);
                self.members.send_to_group(
                    group.id,
                    &ControlMessage::KeyUp {
                        group_id: group.id,
                        transmission_id: id,
                    },
                );
            } else {
                self.tones.tone(Tone::Bonk);
                ptt::set_status(&format!("[--] {} busy", group.name));
                return;
            }
        } else if let Some(floor) = state.floors.get_mut(&group.id)
            && let Some(id) = floor.release(now)
        {
            self.transmitter.key_down();
            self.members.send_to_group(
                group.id,
                &ControlMessage::KeyDown {
                    group_id: group.id,
                    transmission_id: id,
                },
            );
        }
        self.show(&state);
        self.deadline_changed.notify_one();
    }

    /// Selects the group at `index` in [`Radio::groups`] for transmitting.
    pub fn select(&self, index: usize) {
        let mut state = self.state.lock().unwrap();
        if index >= self.groups.len() || index == state.selected {
            return;
        }
        if state
            .floors
            .values()
            .any(|floor| matches!(floor.state(), FloorState::Transmitting { .. }))
        {
            ptt::notice("can't change talkgroup while transmitting");
            return;
        }
        let old = &self.groups[state.selected];
        if old.id != ALL_CALL {
            self.members
                .broadcast(&ControlMessage::Leave { group_id: old.id });
            state.floors.remove(&old.id);
        }
        let new = &self.groups[index];
        if new.id != ALL_CALL {
            self.members.join_all(new, &self.me);
            state.floors.insert(
                new.id,
                Floor::new(self.me.radio_id(), FloorConfig::default()),
            );
        }
        state.selected = index;
        ptt::notice(&format!("talkgroup {}: {}", index, describe(new)));
        self.show(&state);
        self.deadline_changed.notify_one();
    }

    /// Selects the next group, wrapping around.
    pub fn select_next(&self) {
        let next = (self.state.lock().unwrap().selected + 1) % self.groups.len();
        self.select(next);
    }

    /// Handles a control message from `peer`.
    pub fn on_control(&self, peer: &Peer, message: ControlMessage) {
        let now = Instant::now();
        let talker = peer.radio_id();
        let mut state = self.state.lock().unwrap();
        match message {
            ControlMessage::Join { group_id, proof } => {
                // Joins for groups we don't know are ignored: we never
                // transmit on them.
                let Some(group) = self.group(group_id) else {
                    return;
                };
                let joiner = peer.fingerprint();
                if group.accepts_join(proof.as_ref(), joiner.as_bytes(), self.me.as_bytes()) {
                    self.members.set_member(talker, group_id, true);
                } else {
                    ptt::notice(&format!(
                        "{} tried to join {} without the right key",
                        peer.display_name(),
                        group.name
                    ));
                }
                return;
            }
            ControlMessage::Leave { group_id } => {
                self.members.set_member(talker, group_id, false);
                return;
            }
            ControlMessage::KeyUp {
                group_id,
                transmission_id,
            } => {
                if !self.members.is_member(talker, group_id) {
                    return;
                }
                let Some(floor) = state.floors.get_mut(&group_id) else {
                    return;
                };
                if floor.on_key_up(talker, transmission_id, now) == KeyUpOutcome::Preempted {
                    self.transmitter.key_down();
                    self.tones.tone(Tone::Bonk);
                    ptt::notice("lost the floor to a simultaneous key-up");
                }
            }
            ControlMessage::KeyDown {
                group_id,
                transmission_id,
            } => {
                let Some(floor) = state.floors.get_mut(&group_id) else {
                    return;
                };
                floor.on_key_down(talker, transmission_id, now);
            }
            ControlMessage::Hello { .. } => return,
        }
        self.show(&state);
        self.deadline_changed.notify_one();
    }

    /// A voice datagram arrived. Returns true if it should be played.
    pub fn on_voice(&self, header: &VoiceHeader) -> bool {
        if !self.members.is_member(header.sender, header.group_id) {
            return false;
        }
        let mut state = self.state.lock().unwrap();
        // One received stream at a time across groups.
        let busy_elsewhere = state.floors.iter().any(|(id, floor)| {
            *id != header.group_id && matches!(floor.state(), FloorState::Receiving { .. })
        });
        let Some(floor) = state.floors.get_mut(&header.group_id) else {
            return false;
        };
        if busy_elsewhere {
            return false;
        }
        let before = floor.state();
        let play = floor.on_voice(header.sender, header.transmission_id, Instant::now());
        if floor.state() != before {
            self.show(&state);
            self.deadline_changed.notify_one();
        }
        play
    }

    /// A peer disconnected.
    pub fn on_peer_lost(&self, peer: &Peer) {
        self.members.remove(peer.radio_id());
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        for floor in state.floors.values_mut() {
            floor.on_peer_lost(peer.radio_id(), now);
        }
        self.show(&state);
        self.deadline_changed.notify_one();
        ptt::notice(&format!("{} disconnected", peer.display_name()));
    }

    /// Runs floor timers (hang time, time-out). Sleeps with no deadline
    /// while every channel is idle.
    pub async fn run_timers(self: Arc<Self>) {
        loop {
            let deadline = self
                .state
                .lock()
                .unwrap()
                .floors
                .values()
                .filter_map(Floor::next_deadline)
                .min();
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
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        for (&group_id, floor) in &mut state.floors {
            match floor.tick(now) {
                Some(FloorEvent::TimedOut { transmission_id }) => {
                    self.transmitter.key_down();
                    self.members.send_to_group(
                        group_id,
                        &ControlMessage::KeyDown {
                            group_id,
                            transmission_id,
                        },
                    );
                    self.tones.tone(Tone::Bonk);
                    ptt::notice("time-out: transmission ended");
                }
                Some(FloorEvent::HangEnded) => self.tones.tone(Tone::HangEnd),
                Some(FloorEvent::TalkerDropped { .. }) | None => {}
            }
        }
        self.show(&state);
    }

    fn group(&self, id: u32) -> Option<&Group> {
        self.groups.iter().find(|group| group.id == id)
    }

    fn show(&self, state: &State) {
        let selected = &self.groups[state.selected];
        let name_of = |id: u32| self.group(id).map_or("?", |group| group.name.as_str());
        let mut text = None;
        for (&group_id, floor) in &state.floors {
            match floor.state() {
                FloorState::Transmitting { .. } => {
                    text = Some(format!("[TX] {}", name_of(group_id)));
                }
                FloorState::Receiving { talker, .. } if text.is_none() => {
                    text = Some(format!(
                        "[RX] {} on {}",
                        self.members.name_of(talker),
                        name_of(group_id)
                    ));
                }
                _ => {}
            }
        }
        let text = text.unwrap_or_else(|| match state.floors.get(&selected.id).map(Floor::state) {
            Some(FloorState::Hang { .. }) => format!("[..] {} hang time", selected.name),
            _ => format!("[  ] {}", selected.name),
        });
        ptt::set_status(&text);
    }
}

/// A group's name, ID, and whether it is keyed, for notices.
pub fn describe(group: &Group) -> String {
    if group.id == ALL_CALL {
        return group.name.clone();
    }
    let kind = if group.key.is_some() { "keyed" } else { "open" };
    format!("{} ({}, {kind})", group.name, group.id)
}
