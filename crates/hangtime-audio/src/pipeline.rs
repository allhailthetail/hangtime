//! Transmit and receive threads between the device ring buffers and the
//! network. Both run off the audio callbacks, so they may allocate.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use hangtime_proto::VoiceHeader;
use rtrb::{Consumer, Producer};

use crate::codec::{MAX_PACKET, VoiceDecoder, VoiceEncoder};
use crate::jitter::{JitterBuffer, Playout};
use crate::resample::Resampler;
use crate::tones::Tone;
use crate::{AudioError, FRAME_DURATION, FRAME_SAMPLES, SAMPLE_RATE};

/// How often the transmit thread drains the capture ring while keyed.
const CAPTURE_POLL: Duration = Duration::from_millis(5);

/// Average time from a sample leaving the capture ring to its frame
/// being sent: a full 20 ms frame, plus half the capture poll interval.
pub const PACKETIZATION_DELAY: Duration =
    FRAME_DURATION.saturating_add(Duration::from_micros(CAPTURE_POLL.as_micros() as u64 / 2));

/// Jitter buffer depth before playout starts: 3 frames, 60 ms.
const JITTER_TARGET_FRAMES: u16 = 3;

/// If the receive thread falls this far behind, it skips ahead instead
/// of bursting frames to catch up.
const MAX_LATE: Duration = Duration::from_millis(100);

struct TxShared {
    ptt: AtomicBool,
    transmission_id: AtomicU16,
    shutdown: AtomicBool,
}

/// Encodes microphone audio into voice datagrams while keyed. Sleeps
/// (parked) while unkeyed.
pub struct Transmitter {
    shared: Arc<TxShared>,
    thread: Option<JoinHandle<()>>,
}

impl Transmitter {
    /// Starts the transmit thread. `send` receives each encoded datagram.
    pub fn spawn(
        capture: Consumer<f32>,
        device_rate: u32,
        sender: u32,
        group_id: u32,
        send: impl FnMut(Vec<u8>) + Send + 'static,
    ) -> Result<Self, AudioError> {
        let encoder = VoiceEncoder::new()?;
        let shared = Arc::new(TxShared {
            ptt: AtomicBool::new(false),
            transmission_id: AtomicU16::new(0),
            shutdown: AtomicBool::new(false),
        });
        let mut worker = TxWorker {
            shared: Arc::clone(&shared),
            capture,
            resampler: Resampler::new(device_rate, SAMPLE_RATE),
            encoder,
            send: Box::new(send),
            sender,
            group_id,
        };
        let thread = thread::Builder::new()
            .name("hangtime-tx".into())
            .spawn(move || worker.run())
            .map_err(AudioError::Thread)?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Starts a transmission. The floor layer picks `transmission_id` so
    /// it matches the `KEYUP` it sent. Keying again with a new ID ends the
    /// current transmission and starts the new one.
    pub fn key_up(&self, transmission_id: u16) {
        self.shared
            .transmission_id
            .store(transmission_id, Ordering::SeqCst);
        self.shared.ptt.store(true, Ordering::SeqCst);
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }

    /// Ends the current transmission.
    pub fn key_down(&self) {
        self.shared.ptt.store(false, Ordering::SeqCst);
    }
}

impl Drop for Transmitter {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

struct TxWorker {
    shared: Arc<TxShared>,
    capture: Consumer<f32>,
    resampler: Resampler,
    encoder: VoiceEncoder,
    send: Box<dyn FnMut(Vec<u8>) + Send>,
    sender: u32,
    group_id: u32,
}

impl TxWorker {
    fn run(&mut self) {
        let mut pending = Vec::with_capacity(FRAME_SAMPLES * 2);
        let mut chunk = Vec::new();
        let mut packet = [0u8; MAX_PACKET];
        loop {
            while !self.shared.ptt.load(Ordering::SeqCst) {
                if self.shared.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                thread::park();
            }
            // Key-up: audio captured while unkeyed is stale.
            self.discard_capture();
            self.resampler.reset();
            pending.clear();
            let transmission_id = self.shared.transmission_id.load(Ordering::SeqCst);
            let mut header = VoiceHeader {
                group_id: self.group_id,
                sender: self.sender,
                transmission_id,
                sequence: 0,
                timestamp: 0,
                end_of_transmission: false,
            };
            while self.shared.ptt.load(Ordering::SeqCst)
                && self.shared.transmission_id.load(Ordering::SeqCst) == transmission_id
                && !self.shared.shutdown.load(Ordering::SeqCst)
            {
                chunk.clear();
                while let Ok(sample) = self.capture.pop() {
                    chunk.push(sample);
                }
                self.resampler.process(&chunk, &mut pending);
                let mut frames = pending.chunks_exact(FRAME_SAMPLES);
                for frame in &mut frames {
                    let frame: &[f32; FRAME_SAMPLES] = frame.try_into().expect("exact chunk");
                    if let Ok(Some(encoded)) = self.encoder.encode(frame, &mut packet) {
                        let mut datagram = header.encode().to_vec();
                        datagram.extend_from_slice(encoded);
                        (self.send)(datagram);
                        header.sequence = header.sequence.wrapping_add(1);
                    }
                    header.timestamp = header.timestamp.wrapping_add(1);
                }
                let used = pending.len() - frames.remainder().len();
                pending.drain(..used);
                thread::sleep(CAPTURE_POLL);
            }
            header.end_of_transmission = true;
            (self.send)(header.encode().to_vec());
        }
    }

    fn discard_capture(&mut self) {
        let stale = self.capture.slots();
        if let Ok(chunk) = self.capture.read_chunk(stale) {
            chunk.commit_all();
        }
    }
}

/// Receive-side measurements for one transmission, reported when it ends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReceiveStats {
    /// Radio ID of the talker.
    pub sender: u32,
    /// Frames the sender sent, from sequence numbers.
    pub sent: u32,
    /// Sent frames that never arrived.
    pub lost: u32,
    /// Average time a frame waited in the jitter buffer.
    pub jitter: Duration,
    /// Average audio already queued for playback when a frame was added.
    pub playout_queue: Duration,
}

/// Running totals for [`ReceiveStats`].
#[derive(Default)]
struct StatsAccumulator {
    key: Option<(u32, u16)>,
    received: u32,
    sent: u32,
    played: u32,
    jitter: Duration,
    playout_queue: Duration,
}

impl StatsAccumulator {
    fn on_datagram(&mut self, header: &VoiceHeader) {
        let key = (header.sender, header.transmission_id);
        if self.key != Some(key) {
            *self = Self {
                key: Some(key),
                ..Self::default()
            };
        }
        // The sequence counts sent frames; an end-of-transmission datagram
        // carries the count of frames before it.
        let sent_through = u32::from(header.sequence) + u32::from(!header.end_of_transmission);
        self.sent = self.sent.max(sent_through);
        if !header.end_of_transmission {
            self.received += 1;
        }
    }

    fn on_played(&mut self, jitter: Duration, playout_queue: Duration) {
        self.played += 1;
        self.jitter += jitter;
        self.playout_queue += playout_queue;
    }

    /// Finishes the transmission. `None` if nothing was played.
    fn finish(&mut self) -> Option<ReceiveStats> {
        let finished = std::mem::take(self);
        (finished.played > 0).then(|| ReceiveStats {
            sender: finished.key.map_or(0, |(sender, _)| sender),
            sent: finished.sent,
            lost: finished.sent.saturating_sub(finished.received),
            jitter: finished.jitter / finished.played,
            playout_queue: finished.playout_queue / finished.played,
        })
    }
}

/// Plays received voice datagrams. Blocks while nothing is being
/// received; ticks every 20 ms only during a transmission.
pub struct Receiver {
    inbox: Inbox,
    thread: Option<JoinHandle<()>>,
}

/// Input to the receive thread.
enum RxInput {
    Datagram(Vec<u8>),
    Tone(Tone),
}

/// Hands voice datagrams and tones to the receive thread. Cheap to clone.
#[derive(Clone)]
pub struct Inbox(mpsc::Sender<RxInput>);

impl Inbox {
    /// Queues a received voice datagram. Returns false once the receive
    /// thread has stopped.
    pub fn datagram(&self, datagram: Vec<u8>) -> bool {
        self.0.send(RxInput::Datagram(datagram)).is_ok()
    }

    /// Plays a tone, mixed over any voice that is playing.
    pub fn tone(&self, tone: Tone) {
        let _ = self.0.send(RxInput::Tone(tone));
    }
}

impl Receiver {
    /// Starts the receive thread, playing into `playback` at `device_rate`.
    /// `on_end` gets the stats for each transmission once it ends.
    pub fn spawn(
        playback: Producer<f32>,
        device_rate: u32,
        on_end: impl FnMut(ReceiveStats) + Send + 'static,
    ) -> Result<Self, AudioError> {
        let (inbox, inputs) = mpsc::channel();
        let mut worker = RxWorker {
            inputs,
            playback,
            resampler: Resampler::new(SAMPLE_RATE, device_rate),
            jitter: JitterBuffer::new(JITTER_TARGET_FRAMES),
            decoder: VoiceDecoder::new()?,
            device_rate,
            stats: StatsAccumulator::default(),
            on_end: Box::new(on_end),
            pending_tone: Vec::new(),
            out: Vec::new(),
        };
        let thread = thread::Builder::new()
            .name("hangtime-rx".into())
            .spawn(move || worker.run())
            .map_err(AudioError::Thread)?;
        Ok(Self {
            inbox: Inbox(inbox),
            thread: Some(thread),
        })
    }

    /// Handle for passing datagrams and tones to the receive thread.
    pub fn inbox(&self) -> Inbox {
        self.inbox.clone()
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        // The thread exits once every inbox sender is gone; don't wait
        // on clones still held elsewhere.
        drop(self.thread.take());
    }
}

struct RxWorker {
    inputs: mpsc::Receiver<RxInput>,
    playback: Producer<f32>,
    resampler: Resampler,
    jitter: JitterBuffer,
    decoder: VoiceDecoder,
    device_rate: u32,
    stats: StatsAccumulator,
    on_end: Box<dyn FnMut(ReceiveStats) + Send>,
    /// Tone samples (48 kHz) waiting to be mixed into playing voice.
    pending_tone: Vec<f32>,
    /// Scratch buffer for resampled output.
    out: Vec<f32>,
}

impl RxWorker {
    fn run(&mut self) {
        let mut next_tick: Option<Instant> = None;
        let mut pcm = [0f32; FRAME_SAMPLES];
        loop {
            // None means the tick is due with no new input.
            let received = match next_tick {
                None => match self.inputs.recv() {
                    Ok(input) => Some(input),
                    Err(_) => return,
                },
                Some(tick) => match self
                    .inputs
                    .recv_timeout(tick.saturating_duration_since(Instant::now()))
                {
                    Ok(input) => Some(input),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
            };
            let datagram = match received {
                Some(RxInput::Datagram(datagram)) => Some(datagram),
                Some(RxInput::Tone(tone)) => {
                    self.pending_tone.extend(tone.render());
                    if next_tick.is_none() {
                        self.flush_tone();
                    }
                    None
                }
                None => None,
            };
            if let Some(Ok((header, payload))) = datagram.as_deref().map(VoiceHeader::decode) {
                self.jitter.push(&header, payload);
                if self.jitter.is_current(&header) {
                    self.stats.on_datagram(&header);
                }
                if next_tick.is_none() && self.jitter.is_active() {
                    next_tick = Some(Instant::now());
                }
            }
            while let Some(tick) = next_tick.filter(|tick| *tick <= Instant::now()) {
                // On average a frame has waited half a tick less than the
                // queue ahead of it.
                let depth = u32::from(self.jitter.depth());
                let jitter_wait = (FRAME_DURATION * depth).saturating_sub(FRAME_DURATION / 2);
                let decoded = match self.jitter.pop() {
                    Playout::Waiting => None,
                    Playout::Frame(frame) => Some(self.decoder.decode(&frame, &mut pcm)),
                    Playout::Recover(next) => Some(self.decoder.recover(&next, &mut pcm)),
                    Playout::Conceal => Some(self.decoder.conceal(&mut pcm)),
                    Playout::End => {
                        next_tick = None;
                        if let Some(stats) = self.stats.finish() {
                            (self.on_end)(stats);
                            self.pending_tone.extend(Tone::Courtesy.render());
                        }
                        self.resampler.reset();
                        self.flush_tone();
                        if let Ok(decoder) = VoiceDecoder::new() {
                            self.decoder = decoder;
                        }
                        break;
                    }
                };
                if let Some(Ok(())) = decoded {
                    let queued = self.playback.buffer().capacity() - self.playback.slots();
                    let queued =
                        Duration::from_secs_f64(queued as f64 / f64::from(self.device_rate));
                    self.stats.on_played(jitter_wait, queued);
                    let mixed = self.pending_tone.len().min(FRAME_SAMPLES);
                    for (sample, tone) in pcm.iter_mut().zip(self.pending_tone.drain(..mixed)) {
                        *sample += tone;
                    }
                    self.play(&pcm);
                }
                let now = Instant::now();
                next_tick = Some(if now.duration_since(tick) > MAX_LATE {
                    now + FRAME_DURATION
                } else {
                    tick + FRAME_DURATION
                });
            }
        }
    }

    /// Resamples 48 kHz audio and queues it for playback.
    fn play(&mut self, samples: &[f32]) {
        self.out.clear();
        self.resampler.process(samples, &mut self.out);
        for &sample in &self.out {
            // A full ring means playback is behind; drop.
            let _ = self.playback.push(sample);
        }
    }

    /// Plays any pending tone straight away. Only while no voice is
    /// playing, since voice mixes tones in frame by frame.
    fn flush_tone(&mut self) {
        let tone = std::mem::take(&mut self.pending_tone);
        self.play(&tone);
        self.resampler.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;

    #[test]
    fn voice_survives_transmit_and_receive() {
        let (mut mic, capture) = RingBuffer::new(SAMPLE_RATE as usize);
        let (playback, mut speaker) = RingBuffer::new(SAMPLE_RATE as usize);
        let (stats_tx, stats_rx) = mpsc::channel();
        let receiver = Receiver::spawn(playback, SAMPLE_RATE, move |stats| {
            let _ = stats_tx.send(stats);
        })
        .unwrap();
        let inbox = receiver.inbox();
        let (sent_tx, sent_rx) = mpsc::channel();
        let transmitter =
            Transmitter::spawn(capture, SAMPLE_RATE, 7, 0, move |datagram: Vec<u8>| {
                let _ = sent_tx.send(datagram.clone());
                inbox.datagram(datagram);
            })
            .unwrap();

        transmitter.key_up(1);
        thread::sleep(Duration::from_millis(20));
        for i in 0..SAMPLE_RATE / 4 {
            let _ = mic
                .push((i as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.5);
        }
        thread::sleep(Duration::from_millis(150));
        transmitter.key_down();
        thread::sleep(Duration::from_millis(400));

        let sent: Vec<Vec<u8>> = sent_rx.try_iter().collect();
        assert!(sent.len() >= 5, "only {} datagrams sent", sent.len());
        let (last, _) = VoiceHeader::decode(sent.last().unwrap()).unwrap();
        assert!(last.end_of_transmission);

        let mut energy = 0.0;
        while let Ok(sample) = speaker.pop() {
            energy += sample * sample;
        }
        assert!(energy > 1.0, "nothing audible was played: {energy}");

        let stats = stats_rx.try_recv().expect("no stats reported");
        assert_eq!(stats.sent as usize, sent.len() - 1);
        assert_eq!(stats.lost, 0);
    }

    #[test]
    fn plays_tone_while_idle() {
        let (playback, speaker) = RingBuffer::new(SAMPLE_RATE as usize);
        let receiver = Receiver::spawn(playback, SAMPLE_RATE, |_| {}).unwrap();
        receiver.inbox().tone(Tone::Bonk);
        thread::sleep(Duration::from_millis(100));
        assert_eq!(speaker.slots(), Tone::Bonk.render().len());
    }

    fn voice(sequence: u16, end_of_transmission: bool) -> VoiceHeader {
        VoiceHeader {
            group_id: 0,
            sender: 7,
            transmission_id: 1,
            sequence,
            timestamp: sequence,
            end_of_transmission,
        }
    }

    #[test]
    fn counts_lost_frames_from_sequence_gaps() {
        let mut stats = StatsAccumulator::default();
        stats.on_datagram(&voice(0, false));
        stats.on_datagram(&voice(2, false));
        stats.on_datagram(&voice(4, true));
        stats.on_played(FRAME_DURATION, Duration::ZERO);
        let finished = stats.finish().unwrap();
        assert_eq!((finished.sent, finished.lost), (4, 2));
        assert_eq!(finished.jitter, FRAME_DURATION);
    }

    #[test]
    fn reports_nothing_when_nothing_played() {
        let mut stats = StatsAccumulator::default();
        stats.on_datagram(&voice(0, true));
        assert_eq!(stats.finish(), None);
    }
}
