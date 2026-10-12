//! Receive-side jitter buffer for one voice stream.
//!
//! Frames are indexed by the datagram timestamp, so DTX gaps and lost
//! frames both show up as missing slots. The caller pops one entry every
//! 20 ms and turns it into audio with the Opus decoder.

use hangtime_proto::VoiceHeader;

/// Frames the buffer can hold ahead of the playout point (1.28 s).
const CAPACITY: usize = 64;

/// Frames to play past the newest one received before treating the
/// sender as gone. Longer than the 400 ms Opus DTX update interval, so a
/// pause in speech does not end the stream.
const STALL_FRAMES: u16 = 30;

/// What to play for the next 20 ms frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Playout {
    /// Still filling to the target depth; play nothing yet.
    Waiting,
    /// Decode this Opus frame.
    Frame(Vec<u8>),
    /// This frame is missing but the next one is here: decode its
    /// in-band FEC to recover this one.
    Recover(Vec<u8>),
    /// This frame is missing: run packet loss concealment.
    Conceal,
    /// The transmission is over, or no transmission is active.
    End,
}

struct Stream {
    sender: u32,
    transmission_id: u16,
    slots: Vec<Option<Vec<u8>>>,
    next_ts: u16,
    newest_ts: u16,
    end_ts: Option<u16>,
    playing: bool,
    waited: u16,
}

/// Jitter buffer holding at most one transmission at a time.
pub struct JitterBuffer {
    target: u16,
    stream: Option<Stream>,
    last_ended: Option<(u32, u16)>,
}

/// True if `a` is at or after `b`, allowing for wraparound.
fn at_or_after(a: u16, b: u16) -> bool {
    a.wrapping_sub(b) < 0x8000
}

impl JitterBuffer {
    /// Creates a buffer that starts playing once `target` frames are
    /// queued.
    pub fn new(target: u16) -> Self {
        Self {
            target: target.max(1),
            stream: None,
            last_ended: None,
        }
    }

    /// True while a transmission is buffered or playing.
    pub fn is_active(&self) -> bool {
        self.stream.is_some()
    }

    /// Adds a received datagram. Late, duplicate, and too-far-ahead
    /// frames are dropped, as are frames from a transmission that has
    /// already ended.
    pub fn push(&mut self, header: &VoiceHeader, payload: &[u8]) {
        let key = (header.sender, header.transmission_id);
        if self.last_ended == Some(key) {
            return;
        }
        let stream = match &mut self.stream {
            Some(s) if (s.sender, s.transmission_id) == key => s,
            slot => slot.insert(Stream {
                sender: header.sender,
                transmission_id: header.transmission_id,
                slots: vec![None; CAPACITY],
                next_ts: header.timestamp,
                newest_ts: header.timestamp,
                end_ts: None,
                playing: false,
                waited: 0,
            }),
        };
        let ts = header.timestamp;
        if !at_or_after(ts, stream.next_ts)
            || usize::from(ts.wrapping_sub(stream.next_ts)) >= CAPACITY
        {
            return;
        }
        if header.end_of_transmission {
            stream.end_ts = Some(ts);
        } else if at_or_after(ts, stream.newest_ts) {
            stream.newest_ts = ts;
        }
        if !payload.is_empty() {
            stream.slots[usize::from(ts) % CAPACITY] = Some(payload.to_vec());
        }
    }

    /// Returns what to play for the next 20 ms frame.
    pub fn pop(&mut self) -> Playout {
        let Some(stream) = &mut self.stream else {
            return Playout::End;
        };
        if !stream.playing {
            let queued = stream
                .newest_ts
                .wrapping_sub(stream.next_ts)
                .wrapping_add(1);
            if queued < self.target && stream.end_ts.is_none() {
                stream.waited += 1;
                if stream.waited <= STALL_FRAMES {
                    return Playout::Waiting;
                }
            }
            stream.playing = true;
        }
        let past_newest = stream.next_ts.wrapping_sub(stream.newest_ts);
        if stream.end_ts == Some(stream.next_ts)
            || (past_newest < 0x8000 && past_newest > STALL_FRAMES)
        {
            self.last_ended = Some((stream.sender, stream.transmission_id));
            self.stream = None;
            return Playout::End;
        }
        let index = usize::from(stream.next_ts) % CAPACITY;
        let playout = match stream.slots[index].take() {
            Some(frame) => Playout::Frame(frame),
            None => match &stream.slots[(index + 1) % CAPACITY] {
                Some(next) => Playout::Recover(next.clone()),
                None => Playout::Conceal,
            },
        };
        stream.next_ts = stream.next_ts.wrapping_add(1);
        playout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(transmission_id: u16, timestamp: u16) -> VoiceHeader {
        VoiceHeader {
            group_id: 0,
            sender: 7,
            transmission_id,
            sequence: timestamp,
            timestamp,
            end_of_transmission: false,
        }
    }

    fn eot(transmission_id: u16, timestamp: u16) -> VoiceHeader {
        VoiceHeader {
            end_of_transmission: true,
            ..header(transmission_id, timestamp)
        }
    }

    fn frame(ts: u16) -> Playout {
        Playout::Frame(vec![ts as u8])
    }

    #[test]
    fn waits_for_target_then_plays_in_order() {
        let mut jb = JitterBuffer::new(3);
        jb.push(&header(1, 0), &[0]);
        jb.push(&header(1, 1), &[1]);
        assert_eq!(jb.pop(), Playout::Waiting);
        jb.push(&header(1, 2), &[2]);
        assert_eq!(jb.pop(), frame(0));
        assert_eq!(jb.pop(), frame(1));
        assert_eq!(jb.pop(), frame(2));
    }

    #[test]
    fn reorders_frames() {
        let mut jb = JitterBuffer::new(1);
        jb.push(&header(1, 0), &[0]);
        jb.push(&header(1, 2), &[2]);
        jb.push(&header(1, 1), &[1]);
        assert_eq!(jb.pop(), frame(0));
        assert_eq!(jb.pop(), frame(1));
        assert_eq!(jb.pop(), frame(2));
    }

    #[test]
    fn recovers_from_next_frame_or_conceals() {
        let mut jb = JitterBuffer::new(1);
        jb.push(&header(1, 0), &[0]);
        jb.push(&header(1, 2), &[2]);
        jb.push(&header(1, 5), &[5]);
        assert_eq!(jb.pop(), frame(0));
        assert_eq!(jb.pop(), Playout::Recover(vec![2]));
        assert_eq!(jb.pop(), frame(2));
        assert_eq!(jb.pop(), Playout::Conceal);
    }

    #[test]
    fn ends_at_end_of_transmission() {
        let mut jb = JitterBuffer::new(3);
        jb.push(&header(1, 0), &[0]);
        jb.push(&eot(1, 1), &[]);
        assert_eq!(jb.pop(), frame(0));
        assert_eq!(jb.pop(), Playout::End);
        assert!(!jb.is_active());
    }

    #[test]
    fn ignores_frames_after_transmission_ended() {
        let mut jb = JitterBuffer::new(1);
        jb.push(&eot(1, 0), &[]);
        assert_eq!(jb.pop(), Playout::End);
        jb.push(&header(1, 1), &[1]);
        assert!(!jb.is_active());
    }

    #[test]
    fn drops_late_frames() {
        let mut jb = JitterBuffer::new(1);
        jb.push(&header(1, 5), &[5]);
        assert_eq!(jb.pop(), frame(5));
        jb.push(&header(1, 4), &[4]);
        jb.push(&header(1, 6), &[6]);
        assert_eq!(jb.pop(), frame(6));
    }

    #[test]
    fn new_transmission_replaces_old() {
        let mut jb = JitterBuffer::new(1);
        jb.push(&header(1, 0), &[0]);
        jb.push(&header(2, 0), &[9]);
        assert_eq!(jb.pop(), Playout::Frame(vec![9]));
    }

    #[test]
    fn ends_when_sender_goes_silent() {
        let mut jb = JitterBuffer::new(1);
        jb.push(&header(1, 0), &[0]);
        assert_eq!(jb.pop(), frame(0));
        for _ in 0..STALL_FRAMES {
            assert_eq!(jb.pop(), Playout::Conceal);
        }
        assert_eq!(jb.pop(), Playout::End);
    }

    #[test]
    fn handles_timestamp_wraparound() {
        let mut jb = JitterBuffer::new(1);
        jb.push(&header(1, u16::MAX), &[1]);
        jb.push(&header(1, 0), &[2]);
        assert_eq!(jb.pop(), Playout::Frame(vec![1]));
        assert_eq!(jb.pop(), Playout::Frame(vec![2]));
    }
}
