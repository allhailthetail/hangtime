//! Parrot: a headless node that plays each transmission back to its
//! sender, like a DMR parrot talkgroup. It forwards Opus frames without
//! decoding, so it needs no audio devices.

use std::error::Error;
use std::time::Duration;

use bytes::Bytes;
use hangtime_proto::{FRAME_DURATION, VoiceHeader};
use hangtime_transport::{Node, Peer};
use tokio::time::{Instant, sleep, sleep_until};

/// Pause between the sender unkeying and the replay starting.
const REPLAY_DELAY: Duration = Duration::from_millis(500);

/// Longest transmission recorded: 3 minutes of 20 ms frames.
const MAX_FRAMES: usize = 9_000;

pub async fn run(node: &Node) -> Result<(), Box<dyn Error>> {
    println!("parrot listening on {}", node.local_addr()?);
    let parrot_id = node.fingerprint().radio_id();
    while let Some(accepted) = node.accept().await {
        match accepted {
            Ok(peer) => {
                println!(
                    "{} connected [UNTRUSTED] fingerprint {}",
                    peer.remote_address(),
                    peer.fingerprint()
                );
                tokio::spawn(echo(peer, parrot_id));
            }
            Err(e) => eprintln!("incoming connection failed: {e}"),
        }
    }
    Ok(())
}

/// Records each transmission from `peer` and replays it after it ends.
async fn echo(peer: Peer, parrot_id: u32) {
    let mut recording: Vec<(VoiceHeader, Bytes)> = Vec::new();
    let mut current = None;
    let mut ended = None;
    let mut replay_id: u16 = 0;
    while let Ok(datagram) = peer.recv_voice().await {
        let Ok((header, payload)) = VoiceHeader::decode(&datagram) else {
            continue;
        };
        let key = (header.sender, header.transmission_id);
        if ended == Some(key) {
            continue;
        }
        if current != Some(key) {
            recording.clear();
            current = Some(key);
        }
        if !header.end_of_transmission {
            if recording.len() < MAX_FRAMES {
                recording.push((header, datagram.slice_ref(payload)));
            }
            continue;
        }
        ended = current.take();
        replay_id = replay_id.wrapping_add(1);
        tokio::spawn(replay(
            peer.clone(),
            std::mem::take(&mut recording),
            header,
            parrot_id,
            replay_id,
        ));
    }
    println!("{} disconnected", peer.remote_address());
}

/// Sends a recording back with its original frame timing, as a new
/// transmission from the parrot.
async fn replay(
    peer: Peer,
    mut frames: Vec<(VoiceHeader, Bytes)>,
    end: VoiceHeader,
    parrot_id: u32,
    transmission_id: u16,
) {
    sleep(REPLAY_DELAY).await;
    frames.sort_by_key(|(header, _)| header.timestamp);
    let start = Instant::now();
    let mut sequence: u16 = 0;
    for (header, payload) in frames {
        sleep_until(start + FRAME_DURATION * u32::from(header.timestamp)).await;
        let header = VoiceHeader {
            sender: parrot_id,
            transmission_id,
            sequence,
            ..header
        };
        let mut datagram = header.encode().to_vec();
        datagram.extend_from_slice(&payload);
        if peer.send_voice(datagram).is_err() {
            return;
        }
        sequence = sequence.wrapping_add(1);
    }
    sleep_until(start + FRAME_DURATION * u32::from(end.timestamp)).await;
    let end = VoiceHeader {
        sender: parrot_id,
        transmission_id,
        sequence,
        ..end
    };
    let _ = peer.send_voice(end.encode().to_vec());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn datagram(header: VoiceHeader, payload: &[u8]) -> Vec<u8> {
        let mut datagram = header.encode().to_vec();
        datagram.extend_from_slice(payload);
        datagram
    }

    #[tokio::test]
    async fn echoes_transmission_as_its_own() {
        let localhost: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let parrot = Node::bind(localhost).unwrap();
        let client = Node::bind(localhost).unwrap();
        let parrot_addr = parrot.local_addr().unwrap();
        let parrot_id = parrot.fingerprint().radio_id();
        tokio::spawn(async move { run(&parrot).await.unwrap() });
        let peer = client.connect(parrot_addr).await.unwrap();

        let first = VoiceHeader {
            group_id: 9,
            sender: 1,
            transmission_id: 5,
            sequence: 0,
            timestamp: 0,
            end_of_transmission: false,
        };
        let second = VoiceHeader {
            sequence: 1,
            timestamp: 1,
            ..first
        };
        let end = VoiceHeader {
            sequence: 2,
            timestamp: 2,
            end_of_transmission: true,
            ..first
        };
        peer.send_voice(datagram(first, b"one")).unwrap();
        peer.send_voice(datagram(second, b"two")).unwrap();
        peer.send_voice(datagram(end, b"")).unwrap();

        let mut echoed = Vec::new();
        for _ in 0..3 {
            let received = peer.recv_voice().await.unwrap();
            let (header, payload) = VoiceHeader::decode(&received).unwrap();
            echoed.push((header, payload.to_vec()));
        }
        let payloads: Vec<&[u8]> = echoed.iter().map(|(_, p)| p.as_slice()).collect();
        assert_eq!(payloads, [&b"one"[..], b"two", b""]);
        for (header, _) in &echoed {
            assert_eq!(header.sender, parrot_id);
            assert_eq!(header.group_id, 9);
            assert_ne!((header.sender, header.transmission_id), (1, 5));
        }
        assert!(echoed[2].0.end_of_transmission);
    }
}
