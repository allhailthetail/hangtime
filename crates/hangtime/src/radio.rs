//! The radio: ties floor control to the transmitter, peers, talkgroups,
//! and the status line.
//!
//! Scan list: the radio monitors every configured group, each with its
//! own floor, and transmits on the selected one. Only the selected group
//! and All Call are audible, one transmission at a time; activity on the
//! other groups shows as a level bar and talker name.
//!
//! The last group is the built-in parrot ([`PARROT`]): local only, it
//! plays each of your transmissions back to you.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hangtime_audio::{
    FRAME_SAMPLES, Inbox, Tone, Transmitter, VOX_SENSITIVITY_DEFAULT, VOX_SENSITIVITY_MAX,
    VOX_SENSITIVITY_MIN, VoiceDecoder, VoxEvent, level_db, min_level_db,
};
use hangtime_proto::{ALL_CALL, ControlMessage, VoiceHeader};
use hangtime_session::{Floor, FloorConfig, FloorEvent, FloorState, Group, KeyUpOutcome};
use hangtime_transport::{Fingerprint, Peer};
use tokio::sync::Notify;

use crate::echo::PARROT_TALKER;
use crate::ui::{self, Activity, GroupRow, PeerRow};

/// Group ID of the built-in parrot. `0` is reserved on the wire, so no
/// peer can join it; it never leaves this machine.
pub const PARROT: u32 = 0;

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
    /// Per group: the transmission being metered and its decoder.
    meters: HashMap<u32, ((u32, u16), VoiceDecoder)>,
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
    vox: AtomicBool,
    vox_sensitivity: AtomicU8,
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
        groups.push(Group {
            id: PARROT,
            name: "Parrot".into(),
            key: None,
        });
        let floors = groups
            .iter()
            .map(|group| (group.id, Floor::new(me.radio_id(), FloorConfig::default())))
            .collect();
        Arc::new(Self {
            me,
            groups,
            state: Mutex::new(State {
                selected: 0,
                floors,
                meters: HashMap::new(),
            }),
            members,
            transmitter,
            tones,
            next_transmission: AtomicU16::new(seed),
            vox: AtomicBool::new(false),
            vox_sensitivity: AtomicU8::new(VOX_SENSITIVITY_DEFAULT),
            deadline_changed: Notify::new(),
        })
    }

    /// Registers a connected peer and joins the monitored groups with it.
    /// Returns false for a duplicate connection.
    pub fn add_peer(&self, peer: &Peer) -> bool {
        if !self.members.add(peer.clone()) {
            return false;
        }
        ui::add_peer(PeerRow {
            radio_id: peer.radio_id(),
            name: peer.display_name().to_string(),
            address: peer.remote_address().to_string(),
        });
        let state = self.state.lock().unwrap();
        for group_id in state.floors.keys() {
            if let Some(group) = self.group(*group_id)
                && group.id != PARROT
            {
                join(peer, group, &self.me);
            }
        }
        true
    }

    /// The local PTT key went down or up.
    pub fn ptt(&self, keyed: bool) {
        self.key(keyed, false);
    }

    /// VOX heard speech start or stop.
    pub fn on_vox(&self, event: VoxEvent) {
        if self.vox.load(Ordering::SeqCst) {
            self.key(event == VoxEvent::Start, true);
        }
    }

    /// Turns VOX on or off.
    pub fn set_vox(&self, on: bool) {
        if self.vox.swap(on, Ordering::SeqCst) == on {
            return;
        }
        self.transmitter.set_vox(on);
        if !on {
            // Don't leave a VOX transmission keyed with nothing to end it.
            self.key(false, true);
        }
        ui::notice(if on {
            "VOX on: talking keys the radio"
        } else {
            "VOX off"
        });
        self.show_vox();
    }

    pub fn toggle_vox(&self) {
        self.set_vox(!self.vox.load(Ordering::SeqCst));
    }

    /// Sets VOX sensitivity, clamped to 1 (least) through 5 (most).
    pub fn set_vox_sensitivity(&self, level: u8) {
        let level = level.clamp(VOX_SENSITIVITY_MIN, VOX_SENSITIVITY_MAX);
        self.vox_sensitivity.store(level, Ordering::SeqCst);
        self.transmitter.set_vox_sensitivity(level);
    }

    /// Makes VOX more (positive) or less (negative) sensitive by one step.
    pub fn adjust_vox_sensitivity(&self, step: i8) {
        let level = self
            .vox_sensitivity
            .load(Ordering::SeqCst)
            .saturating_add_signed(step);
        self.set_vox_sensitivity(level);
        let level = self.vox_sensitivity.load(Ordering::SeqCst);
        ui::notice(&format!(
            "VOX sensitivity {level}/{VOX_SENSITIVITY_MAX} (put vox_sensitivity = {level} in hangtime.toml to keep it)"
        ));
        self.show_vox();
    }

    /// Keys or unkeys. VOX key-ups send pre-roll, skip the talk-permit
    /// chirp, and are refused silently when the channel is busy, so speech
    /// on a busy channel doesn't set off a string of bonks.
    fn key(&self, keyed: bool, vox: bool) {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        let group = &self.groups[state.selected];
        if keyed {
            // Half duplex: no talking over a transmission you can hear.
            // The selected group's own floor covers itself; All Call is the
            // only other audible group.
            let receiving_elsewhere = state.floors.iter().any(|(id, floor)| {
                *id != group.id
                    && *id == ALL_CALL
                    && matches!(floor.state(), FloorState::Receiving { .. })
            });
            let id = self.next_transmission.fetch_add(1, Ordering::Relaxed);
            let floor = state
                .floors
                .get_mut(&group.id)
                .expect("selected group has a floor");
            if !receiving_elsewhere && floor.request(id, now).is_ok() {
                self.transmitter.key_up(id, group.id, vox);
                if !vox {
                    self.tones.tone(Tone::TalkPermit);
                }
                self.members.send_to_group(
                    group.id,
                    &ControlMessage::KeyUp {
                        group_id: group.id,
                        transmission_id: id,
                    },
                );
            } else {
                if !vox {
                    self.tones.tone(Tone::Bonk);
                }
                ui::set_status(Activity::Busy, &format!("BUSY  {}", group.name));
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
            ui::notice("can't change talkgroup while transmitting");
            return;
        }
        // Every group is already monitored; selecting only changes which
        // one is heard and transmitted on.
        let new = &self.groups[index];
        state.selected = index;
        ui::set_selected(index);
        ui::notice(&format!("talkgroup {}: {}", index, describe(new)));
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
                // transmit on them. The parrot is local only.
                let Some(group) = self.group(group_id).filter(|group| group.id != PARROT) else {
                    return;
                };
                let joiner = peer.fingerprint();
                if group.accepts_join(proof.as_ref(), joiner.as_bytes(), self.me.as_bytes()) {
                    self.members.set_member(talker, group_id, true);
                } else {
                    ui::notice(&format!(
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
                    ui::notice("lost the floor to a simultaneous key-up");
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

    /// A voice datagram arrived from a peer. Returns true if it should be
    /// played.
    pub fn on_voice(&self, header: &VoiceHeader, payload: &[u8]) -> bool {
        self.members.is_member(header.sender, header.group_id) && self.hear(header, payload)
    }

    /// A frame of a parrot replay. Returns true if it should be played.
    pub fn on_parrot_voice(&self, header: &VoiceHeader, payload: &[u8]) -> bool {
        self.hear(header, payload)
    }

    /// The parrot starts or ends a replay, as a peer's `KEYUP` or
    /// `KEYDOWN` would.
    pub fn parrot_keyed(&self, keyed: bool, transmission_id: u16) {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        let floor = state.floors.get_mut(&PARROT).expect("parrot has a floor");
        if !keyed {
            floor.on_key_down(PARROT_TALKER, transmission_id, now);
        } else if floor.on_key_up(PARROT_TALKER, transmission_id, now) == KeyUpOutcome::Preempted {
            self.transmitter.key_down();
            self.tones.tone(Tone::Bonk);
        }
        self.show(&state);
        self.deadline_changed.notify_one();
    }

    /// Updates the group's floor and meter for a voice frame, and returns
    /// true if it should be played: only the selected group and All Call
    /// are audible, one transmission at a time.
    fn hear(&self, header: &VoiceHeader, payload: &[u8]) -> bool {
        let mut state = self.state.lock().unwrap();
        let selected = self.groups[state.selected].id;
        let audible = |id: u32| id == ALL_CALL || id == selected;
        let busy_elsewhere = state.floors.iter().any(|(&id, floor)| {
            id != header.group_id
                && audible(id)
                && matches!(floor.state(), FloorState::Receiving { .. })
        });
        let Some(floor) = state.floors.get_mut(&header.group_id) else {
            return false;
        };
        let before = floor.state();
        let accepted = floor.on_voice(header.sender, header.transmission_id, Instant::now());
        let changed = floor.state() != before;
        if accepted {
            self.meter(&mut state, header, payload);
        }
        if changed {
            self.show(&state);
            self.deadline_changed.notify_one();
        }
        accepted && audible(header.group_id) && !busy_elsewhere
    }

    /// Decodes a frame just to measure its level for the group's bar.
    fn meter(&self, state: &mut State, header: &VoiceHeader, payload: &[u8]) {
        if payload.is_empty() {
            return;
        }
        let key = (header.sender, header.transmission_id);
        let entry = match state.meters.entry(header.group_id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let Ok(decoder) = VoiceDecoder::new() else {
                    return;
                };
                entry.insert((key, decoder))
            }
        };
        if entry.0 != key {
            // A new transmission: start a fresh decoder.
            if let Ok(decoder) = VoiceDecoder::new() {
                *entry = (key, decoder);
            }
        }
        let mut pcm = [0f32; FRAME_SAMPLES];
        if entry.1.decode(payload, &mut pcm).is_ok()
            && let Some(index) = self.groups.iter().position(|g| g.id == header.group_id)
        {
            ui::meter(index, level_db(&pcm));
        }
    }

    /// A peer disconnected.
    pub fn on_peer_lost(&self, peer: &Peer) {
        self.members.remove(peer.radio_id());
        ui::remove_peer(peer.radio_id());
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        for floor in state.floors.values_mut() {
            floor.on_peer_lost(peer.radio_id(), now);
        }
        self.show(&state);
        self.deadline_changed.notify_one();
        ui::notice(&format!("{} disconnected", peer.display_name()));
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
                    ui::notice("time-out: transmission ended");
                }
                Some(FloorEvent::HangEnded) => self.tones.tone(Tone::HangEnd),
                Some(FloorEvent::TalkerDropped { .. }) | None => {}
            }
        }
        self.show(&state);
    }

    /// A talker's name; the parrot's replays come from "Parrot".
    fn name_of(&self, group_id: u32, talker: u32) -> String {
        if group_id == PARROT && talker == PARROT_TALKER {
            "Parrot".to_string()
        } else {
            self.members.name_of(talker)
        }
    }

    fn group(&self, id: u32) -> Option<&Group> {
        self.groups.iter().find(|group| group.id == id)
    }

    fn show(&self, state: &State) {
        let selected = &self.groups[state.selected];
        let name_of = |id: u32| self.group(id).map_or("?", |group| group.name.as_str());
        let mut shown = None;
        for (&group_id, floor) in &state.floors {
            match floor.state() {
                FloorState::Transmitting { .. } => {
                    shown = Some((Activity::Transmitting, format!("TX  {}", name_of(group_id))));
                }
                FloorState::Receiving { talker, .. }
                    if shown.is_none() && (group_id == ALL_CALL || group_id == selected.id) =>
                {
                    shown = Some((
                        Activity::Receiving,
                        format!(
                            "RX  {} on {}",
                            self.name_of(group_id, talker),
                            name_of(group_id)
                        ),
                    ));
                }
                _ => {}
            }
        }
        let talkers = self
            .groups
            .iter()
            .map(
                |group| match state.floors.get(&group.id).map(Floor::state) {
                    Some(FloorState::Receiving { talker, .. }) => {
                        Some(self.name_of(group.id, talker))
                    }
                    Some(FloorState::Transmitting { .. }) => Some("you".to_string()),
                    _ => None,
                },
            )
            .collect();
        ui::set_talkers(talkers);
        let (activity, text) =
            shown.unwrap_or_else(|| match state.floors.get(&selected.id).map(Floor::state) {
                Some(FloorState::Hang { .. }) => {
                    (Activity::Hang, format!("HANG  {}", selected.name))
                }
                _ => (Activity::Idle, format!("IDLE  {}", selected.name)),
            });
        ui::set_status(activity, &text);
    }

    /// Pushes the whole radio state to the screen, once it is up.
    pub fn publish(&self) {
        let rows = self
            .groups
            .iter()
            .map(|group| GroupRow {
                name: group.name.clone(),
                kind: match (group.id, &group.key) {
                    (ALL_CALL, _) => "",
                    (PARROT, _) => "echo",
                    (_, Some(_)) => "keyed",
                    (_, None) => "open",
                },
            })
            .collect();
        let state = self.state.lock().unwrap();
        ui::set_groups(rows, state.selected);
        self.show_vox();
        self.show(&state);
    }

    fn show_vox(&self) {
        let level = self.vox_sensitivity.load(Ordering::SeqCst);
        ui::set_vox(
            self.vox
                .load(Ordering::SeqCst)
                .then(|| (level, min_level_db(level))),
        );
    }
}

/// A group's name, ID, and whether it is keyed, for notices.
pub fn describe(group: &Group) -> String {
    if group.id == ALL_CALL {
        return group.name.clone();
    }
    if group.id == PARROT {
        return format!("{} (plays you back, local only)", group.name);
    }
    let kind = if group.key.is_some() { "keyed" } else { "open" };
    format!("{} ({}, {kind})", group.name, group.id)
}
