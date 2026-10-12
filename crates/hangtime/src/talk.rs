//! Talk mode: a mesh node. Accepts any number of peers, dials static
//! peers and peers found by mDNS (redialing when a connection drops), and
//! runs the radio: floor control on every group, plus the local parrot.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hangtime_audio::{
    Capture, DeviceLatency, Inbox, PACKETIZATION_DELAY, Playback, ReceiveStats, Receiver,
    Transmitter,
};
use hangtime_discovery::{Advert, Discovery};
use hangtime_proto::VoiceHeader;
use hangtime_session::Group;
use hangtime_transport::{Node, Peer};
use tokio::runtime::Runtime;
use tokio::sync::Notify;

use crate::echo;
use crate::ptt::{self, Input};
use crate::radio::{Members, PARROT, Radio};
use crate::ui;

/// First wait before redialing a lost peer; doubles up to the longest.
const REDIAL_MIN: Duration = Duration::from_secs(1);
const REDIAL_MAX: Duration = Duration::from_secs(30);

/// Everything a connection task needs.
struct Shared {
    node: Node,
    radio: Arc<Radio>,
    members: Arc<Members>,
    inbox: Inbox,
    /// Fingerprints (hex) being dialed, so mDNS re-announcements don't
    /// start a second attempt.
    dialing: Mutex<HashSet<String>>,
    /// Signalled whenever a peer disconnects.
    peer_lost: Notify,
}

/// Talk-mode settings, from the command line and `hangtime.toml`.
pub struct Options {
    pub peers: Vec<SocketAddr>,
    pub name: String,
    pub groups: Vec<Group>,
    /// Names of muted groups.
    pub muted: Vec<String>,
    pub vox: bool,
    pub vox_sensitivity: Option<u8>,
    /// Preferred microphone and speaker names; `None` for the default.
    pub microphone: Option<String>,
    pub speaker: Option<String>,
}

pub fn run(runtime: &Runtime, node: Node, options: Options) -> Result<(), Box<dyn Error>> {
    let Options {
        peers: static_peers,
        name,
        groups,
        muted,
        vox,
        vox_sensitivity,
        microphone,
        speaker: speaker_name,
    } = options;
    let name = name.as_str();
    let me = node.fingerprint().radio_id();
    let port = node.local_addr()?.port();

    let (capture, mic) = Capture::open(microphone.as_deref())?;
    let (playback, speaker) = Playback::open(speaker_name.as_deref())?;
    let members = Arc::new(Members::default());
    let report = latency_reporter(capture.latency(), playback.latency(), Arc::clone(&members));
    let receiver = Receiver::spawn(speaker, playback.device_rate(), report)?;
    let tx_members = Arc::clone(&members);
    // VOX decisions come from the transmit thread; the radio handles them
    // on the runtime.
    let (vox_tx, mut vox_rx) = tokio::sync::mpsc::unbounded_channel();
    // Voice on the parrot group goes to the local echo, not the network.
    let (parrot_tx, parrot_rx) = tokio::sync::mpsc::unbounded_channel();
    let transmitter = Transmitter::spawn(
        mic,
        capture.device_rate(),
        me,
        move |datagram: Vec<u8>| match VoiceHeader::decode(&datagram) {
            Ok((header, _)) if header.group_id == PARROT => {
                let _ = parrot_tx.send(datagram);
            }
            _ => tx_members.send_voice(datagram),
        },
        move |event| {
            let _ = vox_tx.send(event);
        },
        ui::mic_level,
    )?;
    let radio = Radio::new(
        node.fingerprint(),
        groups,
        &muted,
        transmitter,
        Arc::clone(&members),
        receiver.inbox(),
    );
    if let Some(level) = vox_sensitivity {
        radio.set_vox_sensitivity(level);
    }

    // The screen goes up before any peer can connect, so none is missed.
    let fingerprint = node.fingerprint().to_string();
    let screen = Screen::start(format!(
        "Hangtime · {name} · port {port} · {}",
        &fingerprint[..9]
    ))?;
    radio.publish();
    ui::notice(&format!("microphone {}", capture.description()));
    ui::notice(&format!("speaker {}", playback.description()));
    for notice in [capture.notice(), playback.notice()].into_iter().flatten() {
        ui::notice(&format!("note: {notice}"));
    }

    runtime.spawn(Arc::clone(&radio).run_timers());
    runtime.spawn(echo::run(
        ParrotOut {
            radio: Arc::clone(&radio),
            inbox: receiver.inbox(),
        },
        parrot_rx,
    ));
    let vox_radio = Arc::clone(&radio);
    runtime.spawn(async move {
        while let Some(event) = vox_rx.recv().await {
            vox_radio.on_vox(event);
        }
    });

    let fingerprint = node.fingerprint().hex();
    let shared = Arc::new(Shared {
        node,
        radio: Arc::clone(&radio),
        members,
        inbox: receiver.inbox(),
        dialing: Mutex::new(HashSet::new()),
        peer_lost: Notify::new(),
    });
    runtime.spawn(accept_loop(Arc::clone(&shared)));
    for addr in static_peers {
        runtime.spawn(keep_connected(Arc::clone(&shared), vec![addr], None));
    }
    let discovery = Discovery::start(&Advert {
        radio_id: me,
        fingerprint: &fingerprint,
        name,
        port,
    });
    match discovery {
        Ok(discovery) => {
            runtime.spawn(discover_loop(Arc::clone(&shared), discovery, fingerprint));
        }
        Err(e) => ui::notice(&format!("LAN discovery unavailable ({e}); use --peer")),
    }

    radio.set_vox(vox);
    ptt::run(|input| match input {
        Input::Ptt(keyed) => radio.ptt(keyed),
        Input::Select(index) => radio.select(index),
        Input::Next => radio.select_next(),
        Input::ToggleVox => radio.toggle_vox(),
        Input::VoxSensitivity(step) => radio.adjust_vox_sensitivity(step),
        Input::ToggleMute => radio.toggle_mute(),
    })?;

    // Close the audio devices first, so a Bluetooth headset goes back to
    // its listening mode as soon as possible, then give back the terminal.
    drop(capture);
    drop(playback);
    drop(screen);
    Ok(())
}

/// Parrot replays go through the radio's floor, then to the speaker.
struct ParrotOut {
    radio: Arc<Radio>,
    inbox: Inbox,
}

impl echo::Listener for ParrotOut {
    fn keyed(&self, keyed: bool, transmission_id: u16) {
        self.radio.parrot_keyed(keyed, transmission_id);
    }

    fn voice(&self, datagram: Vec<u8>) {
        if let Ok((header, payload)) = VoiceHeader::decode(&datagram)
            && self.radio.on_parrot_voice(&header, payload)
        {
            self.inbox.datagram(datagram);
        }
    }
}

/// The full-screen UI for the life of talk mode. Dropping it, including
/// on an early error return, gives the terminal back.
struct Screen;

impl Screen {
    fn start(title: String) -> std::io::Result<Self> {
        ui::start(title, true)?;
        Ok(Self)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        ui::stop();
    }
}

async fn accept_loop(shared: Arc<Shared>) {
    while let Some(accepted) = shared.node.accept().await {
        match accepted {
            Ok(peer) => {
                tokio::spawn(serve(Arc::clone(&shared), peer));
            }
            Err(e) => ui::notice(&format!("incoming connection failed: {e}")),
        }
    }
}

/// Dials nodes found on the LAN. Only the side with the lower fingerprint
/// dials, so two nodes that find each other open one connection.
async fn discover_loop(shared: Arc<Shared>, discovery: Discovery, own_fingerprint: String) {
    while let Some(found) = discovery.next().await {
        if own_fingerprint >= found.fingerprint || shared.members.has(&found.fingerprint) {
            continue;
        }
        if !shared
            .dialing
            .lock()
            .unwrap()
            .insert(found.fingerprint.clone())
        {
            continue;
        }
        tokio::spawn(keep_connected(
            Arc::clone(&shared),
            found.addrs,
            Some(found.fingerprint),
        ));
    }
}

/// Keeps a peer connected: dials whichever address answers first, serves
/// the connection until it drops, then redials with backoff.
///
/// A peer the user named (`expected` is `None`) is redialed for as long as
/// Hangtime runs. A LAN peer (`expected` is its advertised fingerprint) is
/// given up once the backoff reaches its limit; if it comes back, it
/// announces itself again.
async fn keep_connected(shared: Arc<Shared>, addrs: Vec<SocketAddr>, expected: Option<String>) {
    let mut backoff = REDIAL_MIN;
    let mut reported = false;
    loop {
        match shared.node.connect_any(&addrs).await {
            Ok(peer)
                if expected
                    .as_ref()
                    .is_some_and(|fp| *fp != peer.fingerprint().hex()) =>
            {
                peer.close("unexpected identity");
                break;
            }
            Ok(peer) => {
                let fingerprint = peer.fingerprint().hex();
                if serve(Arc::clone(&shared), peer).await {
                    backoff = REDIAL_MIN;
                    reported = false;
                } else {
                    // Already connected the other way: wait for that
                    // connection to end before dialing again.
                    wait_until_gone(&shared, &fingerprint).await;
                    continue;
                }
            }
            Err(e) => {
                // LAN nodes come and go; only report peers the user named.
                if expected.is_none() && !reported {
                    ui::notice(&format!(
                        "could not reach {}: {e}; will keep trying",
                        addrs[0]
                    ));
                    reported = true;
                }
                if expected.is_some() && backoff >= REDIAL_MAX {
                    break;
                }
            }
        }
        tokio::time::sleep(jitter(backoff)).await;
        backoff = (backoff * 2).min(REDIAL_MAX);
    }
    if let Some(fp) = &expected {
        shared.dialing.lock().unwrap().remove(fp);
    }
}

/// Waits until no peer with this fingerprint is connected.
async fn wait_until_gone(shared: &Shared, fingerprint: &str) {
    loop {
        let lost = shared.peer_lost.notified();
        tokio::pin!(lost);
        lost.as_mut().enable();
        if !shared.members.has(fingerprint) {
            return;
        }
        lost.await;
    }
}

/// Scales `base` by a factor between 0.5 and 1.5, so two nodes that dial
/// each other don't collide on every retry.
fn jitter(base: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    base.mul_f64(0.5 + f64::from(nanos % 1000) / 1000.0)
}

/// Registers a connected peer and handles its control messages and voice
/// until it disconnects. Returns false, closing the connection, if the
/// peer was already connected.
async fn serve(shared: Arc<Shared>, peer: Peer) -> bool {
    if !shared.radio.add_peer(&peer) {
        peer.close("duplicate connection");
        return false;
    }
    ui::notice(&format!(
        "{} joined from {} [UNTRUSTED] {}",
        peer.display_name(),
        peer.remote_address(),
        peer.fingerprint()
    ));
    let radio = &shared.radio;
    let control = async {
        while let Ok(message) = peer.recv_control().await {
            radio.on_control(&peer, message);
        }
    };
    let voice = async {
        while let Ok(datagram) = peer.recv_voice().await {
            let Ok((header, payload)) = VoiceHeader::decode(&datagram) else {
                continue;
            };
            if radio.on_voice(&header, payload) && !shared.inbox.datagram(datagram.to_vec()) {
                return;
            }
        }
    };
    tokio::select! {
        () = control => {}
        () = voice => {}
    }
    radio.on_peer_lost(&peer);
    shared.peer_lost.notify_waiters();
    true
}

/// Prints a latency estimate after each received transmission.
///
/// The input figure is this machine's capture latency, standing in for the
/// sender's. With the parrot the sender is this machine, so it is exact.
fn latency_reporter(
    input: DeviceLatency,
    output: DeviceLatency,
    members: Arc<Members>,
) -> impl FnMut(ReceiveStats) + Send + 'static {
    move |stats| {
        let estimate = LatencyEstimate {
            input: input.get(),
            packetize: PACKETIZATION_DELAY,
            network: members.rtt_of(stats.sender).unwrap_or_default() / 2,
            jitter: stats.jitter,
            output: output.get() + stats.playout_queue,
        };
        ui::notice(&format!(
            "{estimate}; lost {}/{} frames",
            stats.lost, stats.sent
        ));
    }
}

/// Estimated one-way mouth-to-ear latency, by stage.
struct LatencyEstimate {
    input: Duration,
    packetize: Duration,
    network: Duration,
    jitter: Duration,
    output: Duration,
}

impl LatencyEstimate {
    fn total(&self) -> Duration {
        self.input + self.packetize + self.network + self.jitter + self.output
    }
}

impl fmt::Display for LatencyEstimate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |d: Duration| d.as_millis();
        write!(
            f,
            "latency ~{} ms = in {} + frame {} + net {} + jitter {} + out {} ms",
            ms(self.total()),
            ms(self.input),
            ms(self.packetize),
            ms(self.network),
            ms(self.jitter),
            ms(self.output),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_estimate_with_total() {
        let ms = Duration::from_millis;
        let estimate = LatencyEstimate {
            input: ms(10),
            packetize: ms(22),
            network: ms(1),
            jitter: ms(50),
            output: ms(15),
        };
        assert_eq!(
            estimate.to_string(),
            "latency ~98 ms = in 10 + frame 22 + net 1 + jitter 50 + out 15 ms"
        );
    }
}
