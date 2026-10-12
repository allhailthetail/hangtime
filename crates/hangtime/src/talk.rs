//! Talk mode: a mesh node. Accepts any number of peers, dials static
//! peers and peers found by mDNS, and runs floor control on All Call and
//! the selected talkgroup.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hangtime_audio::{
    Capture, DeviceLatency, Inbox, PACKETIZATION_DELAY, Playback, ReceiveStats, Receiver,
    Transmitter,
};
use hangtime_discovery::{Advert, Discovery};
use hangtime_proto::VoiceHeader;
use hangtime_session::Group;
use hangtime_transport::{Node, Peer};
use tokio::runtime::Runtime;

use crate::ptt::{self, Input};
use crate::radio::{Members, Radio, describe};

/// Everything a connection task needs.
struct Shared {
    node: Node,
    radio: Arc<Radio>,
    members: Arc<Members>,
    inbox: Inbox,
    /// Fingerprints (hex) being dialed, so mDNS re-announcements don't
    /// start a second attempt.
    dialing: Mutex<HashSet<String>>,
}

pub fn run(
    runtime: &Runtime,
    node: Node,
    static_peers: Vec<SocketAddr>,
    name: &str,
    groups: Vec<Group>,
    vox: bool,
) -> Result<(), Box<dyn Error>> {
    let me = node.fingerprint().radio_id();
    let port = node.local_addr()?.port();
    println!("listening on port {port}");

    let (capture, mic) = Capture::open_default()?;
    let (playback, speaker) = Playback::open_default()?;
    let members = Arc::new(Members::default());
    let report = latency_reporter(capture.latency(), playback.latency(), Arc::clone(&members));
    let receiver = Receiver::spawn(speaker, playback.device_rate(), report)?;
    let tx_members = Arc::clone(&members);
    // VOX decisions come from the transmit thread; the radio handles them
    // on the runtime.
    let (vox_tx, mut vox_rx) = tokio::sync::mpsc::unbounded_channel();
    let transmitter = Transmitter::spawn(
        mic,
        capture.device_rate(),
        me,
        move |datagram| tx_members.send_voice(datagram),
        move |event| {
            let _ = vox_tx.send(event);
        },
    )?;
    let radio = Radio::new(
        node.fingerprint(),
        groups,
        transmitter,
        Arc::clone(&members),
        receiver.inbox(),
    );
    println!("talkgroups:");
    for (index, group) in radio.groups().iter().enumerate() {
        println!("  {index}: {}", describe(group));
    }
    runtime.spawn(Arc::clone(&radio).run_timers());
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
    });
    runtime.spawn(accept_loop(Arc::clone(&shared)));
    for addr in static_peers {
        runtime.spawn(dial(Arc::clone(&shared), vec![addr], None));
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
        Err(e) => println!("LAN discovery unavailable ({e}); use --peer"),
    }

    ptt::set_status("[  ] All Call");
    radio.set_vox(vox);
    ptt::run(|input| match input {
        Input::Ptt(keyed) => radio.ptt(keyed),
        Input::Select(index) => radio.select(index),
        Input::Next => radio.select_next(),
        Input::ToggleVox => radio.toggle_vox(),
    })?;
    Ok(())
}

async fn accept_loop(shared: Arc<Shared>) {
    while let Some(accepted) = shared.node.accept().await {
        match accepted {
            Ok(peer) => {
                tokio::spawn(serve(Arc::clone(&shared), peer));
            }
            Err(e) => ptt::notice(&format!("incoming connection failed: {e}")),
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
        tokio::spawn(dial(
            Arc::clone(&shared),
            found.addrs,
            Some(found.fingerprint),
        ));
    }
}

/// Connects to whichever address answers first. With `expected`, only
/// keeps a connection to the identity that was advertised.
async fn dial(shared: Arc<Shared>, addrs: Vec<SocketAddr>, expected: Option<String>) {
    let result = shared.node.connect_any(&addrs).await;
    if let Some(fp) = &expected {
        shared.dialing.lock().unwrap().remove(fp);
    }
    match result {
        Ok(peer)
            if expected
                .as_ref()
                .is_none_or(|fp| *fp == peer.fingerprint().hex()) =>
        {
            serve(shared, peer).await;
        }
        Ok(peer) => peer.close("unexpected identity"),
        // LAN nodes come and go; only report peers the user named.
        Err(e) if expected.is_none() => {
            ptt::notice(&format!("could not reach {}: {e}", addrs[0]));
        }
        Err(_) => {}
    }
}

/// Registers a connected peer and handles its control messages and voice
/// until it disconnects. Duplicate connections are closed.
async fn serve(shared: Arc<Shared>, peer: Peer) {
    if !shared.radio.add_peer(&peer) {
        peer.close("duplicate connection");
        return;
    }
    ptt::notice(&format!(
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
            let Ok((header, _)) = VoiceHeader::decode(&datagram) else {
                continue;
            };
            if radio.on_voice(&header) && !shared.inbox.datagram(datagram.to_vec()) {
                return;
            }
        }
    };
    tokio::select! {
        () = control => {}
        () = voice => {}
    }
    radio.on_peer_lost(&peer);
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
        ptt::notice(&format!(
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
