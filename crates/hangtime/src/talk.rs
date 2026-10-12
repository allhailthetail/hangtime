//! Talk mode: one peer, PTT from the terminal.

use std::error::Error;
use std::net::SocketAddr;

use hangtime_audio::{Capture, Playback, Receiver, Transmitter};
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
    let receiver = Receiver::spawn(speaker, playback.device_rate())?;
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
