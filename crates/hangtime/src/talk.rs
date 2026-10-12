//! Talk mode: one peer, PTT from the terminal.

use std::error::Error;
use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use hangtime_audio::{
    Capture, DeviceLatency, PACKETIZATION_DELAY, Playback, ReceiveStats, Receiver, Transmitter,
};
use hangtime_transport::{Node, Peer};
use tokio::runtime::Runtime;

use crate::ptt;

/// Talkgroups arrive in phase 2; until then everything is group 0.
const GROUP_ID: u32 = 0;

pub fn run(runtime: &Runtime, node: &Node, peer: Option<SocketAddr>) -> Result<(), Box<dyn Error>> {
    let peer = match peer {
        Some(addr) => {
            println!("connecting to {addr}...");
            runtime.block_on(node.connect(addr))?
        }
        None => {
            println!("listening on {}; waiting for a peer...", node.local_addr()?);
            runtime.block_on(node.accept()).ok_or("endpoint closed")??
        }
    };
    println!("connected to {} [UNTRUSTED]", peer.remote_address());
    println!("  peer fingerprint {}", peer.fingerprint());

    let (capture, mic) = Capture::open_default()?;
    let (playback, speaker) = Playback::open_default()?;
    let report = latency_reporter(capture.latency(), playback.latency(), peer.clone());
    let receiver = Receiver::spawn(speaker, playback.device_rate(), report)?;
    runtime.spawn(forward_voice(peer.clone(), receiver.inbox()));

    let tx_peer = peer.clone();
    let transmitter = Transmitter::spawn(
        mic,
        capture.device_rate(),
        node.fingerprint().radio_id(),
        GROUP_ID,
        move |datagram| {
            // Voice is unreliable by design; a dropped send is a lost frame.
            let _ = tx_peer.send_voice(datagram);
        },
    )?;
    ptt::run(|keyed| transmitter.set_ptt(keyed))?;
    Ok(())
}

/// Hands received voice datagrams to the receive thread until the peer
/// disconnects.
async fn forward_voice(peer: Peer, inbox: std::sync::mpsc::Sender<Vec<u8>>) {
    while let Ok(datagram) = peer.recv_voice().await {
        if inbox.send(datagram.to_vec()).is_err() {
            return;
        }
    }
    ptt::notice("peer disconnected; press Q to quit");
}

/// Prints a latency estimate after each received transmission.
///
/// The input figure is this machine's capture latency, standing in for the
/// sender's. With the parrot the sender is this machine, so it is exact.
fn latency_reporter(
    input: DeviceLatency,
    output: DeviceLatency,
    peer: Peer,
) -> impl FnMut(ReceiveStats) + Send + 'static {
    move |stats| {
        let estimate = LatencyEstimate {
            input: input.get(),
            packetize: PACKETIZATION_DELAY,
            network: peer.rtt() / 2,
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
