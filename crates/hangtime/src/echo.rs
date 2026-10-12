//! The built-in parrot talkgroup. Transmissions on it never leave this
//! machine: each one is recorded and played back after the talker
//! unkeys, so anyone can check their microphone, VOX, and speaker without
//! a second node. The replay goes through the same floor and receive path
//! as a peer's transmission.

use std::time::Duration;

use hangtime_proto::{FRAME_DURATION, VoiceHeader};
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep, sleep_until};

/// Pause between the talker unkeying and the replay starting.
const REPLAY_DELAY: Duration = Duration::from_millis(500);

/// Longest transmission recorded: 3 minutes of 20 ms frames.
const MAX_FRAMES: usize = 9_000;

/// Talker ID the replays come from.
pub const PARROT_TALKER: u32 = 0;

/// Where replays go: in talk mode, the radio and the receive path.
pub trait Listener: Send + Sync + 'static {
    /// The parrot keys up or down, as a peer's `KEYUP` or `KEYDOWN` would.
    fn keyed(&self, keyed: bool, transmission_id: u16);
    /// One datagram of a replay, already re-headed as the parrot's.
    fn voice(&self, datagram: Vec<u8>);
}

/// A finished transmission: its datagrams, the last one marking the end.
type Recording = Vec<Vec<u8>>;

/// Records the datagrams this node sends on the parrot group and replays
/// each transmission once it ends. Replays queue, so they never overlap.
pub async fn run(listener: impl Listener, mut sent: mpsc::UnboundedReceiver<Vec<u8>>) {
    let (queue, mut recordings) = mpsc::unbounded_channel::<Recording>();
    tokio::spawn(async move {
        let mut transmission_id: u16 = 0;
        while let Some(recording) = recordings.recv().await {
            transmission_id = transmission_id.wrapping_add(1);
            replay(&listener, recording, transmission_id).await;
        }
    });
    let mut recording = Recording::new();
    let mut current = None;
    while let Some(datagram) = sent.recv().await {
        let Ok((header, _)) = VoiceHeader::decode(&datagram) else {
            continue;
        };
        if current != Some(header.transmission_id) {
            recording.clear();
            current = Some(header.transmission_id);
        }
        let end = header.end_of_transmission;
        if end || recording.len() < MAX_FRAMES {
            recording.push(datagram);
        }
        if end {
            current = None;
            let _ = queue.send(std::mem::take(&mut recording));
        }
    }
}

/// Plays a recording back with its original frame timing, as a
/// transmission from the parrot.
async fn replay(listener: &impl Listener, recording: Recording, transmission_id: u16) {
    sleep(REPLAY_DELAY).await;
    listener.keyed(true, transmission_id);
    let start = Instant::now();
    for (sequence, datagram) in recording.into_iter().enumerate() {
        let Ok((header, payload)) = VoiceHeader::decode(&datagram) else {
            continue;
        };
        let header = VoiceHeader {
            sender: PARROT_TALKER,
            transmission_id,
            sequence: sequence as u16,
            ..header
        };
        sleep_until(start + FRAME_DURATION * u32::from(header.timestamp)).await;
        let mut replayed = header.encode().to_vec();
        replayed.extend_from_slice(payload);
        listener.voice(replayed);
    }
    listener.keyed(false, transmission_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, PartialEq)]
    enum Event {
        Keyed(bool, u16),
        Voice(VoiceHeader, Vec<u8>),
    }

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<(Duration, Event)>>>, Option<Instant>);

    impl Listener for Log {
        fn keyed(&self, keyed: bool, transmission_id: u16) {
            self.push(Event::Keyed(keyed, transmission_id));
        }
        fn voice(&self, datagram: Vec<u8>) {
            let (header, payload) = VoiceHeader::decode(&datagram).unwrap();
            self.push(Event::Voice(header, payload.to_vec()));
        }
    }

    impl Log {
        fn push(&self, event: Event) {
            let at = self.1.unwrap().elapsed();
            self.0.lock().unwrap().push((at, event));
        }
    }

    fn sent(transmission_id: u16, timestamp: u16, end: bool, payload: &[u8]) -> Vec<u8> {
        let header = VoiceHeader {
            group_id: 0,
            sender: 42,
            transmission_id,
            sequence: timestamp,
            timestamp,
            end_of_transmission: end,
        };
        let mut datagram = header.encode().to_vec();
        datagram.extend_from_slice(payload);
        datagram
    }

    #[tokio::test(start_paused = true)]
    async fn replays_each_transmission_after_it_ends() {
        let log = Log(Arc::default(), Some(Instant::now()));
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run(log.clone(), rx));

        for (timestamp, payload) in [(0, &b"a"[..]), (1, b"b")] {
            tx.send(sent(7, timestamp, false, payload)).unwrap();
        }
        tx.send(sent(7, 2, true, b"")).unwrap();
        // A second transmission, queued behind the first replay.
        tx.send(sent(8, 0, false, b"c")).unwrap();
        tx.send(sent(8, 1, true, b"")).unwrap();
        sleep(Duration::from_secs(3)).await;

        let events = log.0.lock().unwrap();
        let summary: Vec<_> = events
            .iter()
            .map(|(_, event)| match event {
                Event::Keyed(keyed, id) => format!("{}{id}", if *keyed { "up" } else { "down" }),
                Event::Voice(header, payload) => {
                    assert_eq!(header.sender, PARROT_TALKER);
                    format!(
                        "{}{}",
                        String::from_utf8_lossy(payload),
                        if header.end_of_transmission {
                            "end"
                        } else {
                            ""
                        }
                    )
                }
            })
            .collect();
        assert_eq!(
            summary,
            ["up1", "a", "b", "end", "down1", "up2", "c", "end", "down2"]
        );
        // The replay starts after the delay and keeps the frame timing.
        assert_eq!(events[0].0, REPLAY_DELAY);
        assert_eq!(events[2].0 - events[1].0, FRAME_DURATION);
    }
}
