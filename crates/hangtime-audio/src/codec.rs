//! Opus encoder and decoder configured per ADR-0003.

use opus::{Application, Bitrate, Channels};

use crate::{AudioError, FRAME_SAMPLES, SAMPLE_RATE};

/// Target Opus bitrate in bits per second.
const BITRATE: i32 = 20_000;

/// Expected packet loss, in percent, that the encoder tunes FEC for.
const EXPECTED_LOSS_PERCENT: i32 = 10;

/// Encoded packet buffer size. 20 kbps at 20 ms is about 50 bytes.
pub const MAX_PACKET: usize = 512;

/// Mono 48 kHz voice encoder: 20 ms frames, in-band FEC, DTX.
pub struct VoiceEncoder {
    inner: opus::Encoder,
}

impl VoiceEncoder {
    pub fn new() -> Result<Self, AudioError> {
        let mut inner = opus::Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip)?;
        inner.set_bitrate(Bitrate::Bits(BITRATE))?;
        inner.set_inband_fec(true)?;
        inner.set_packet_loss_perc(EXPECTED_LOSS_PERCENT)?;
        inner.set_dtx(true)?;
        Ok(Self { inner })
    }

    /// Encodes one frame. Returns `None` when DTX says the frame does not
    /// need to be sent.
    pub fn encode<'a>(
        &mut self,
        pcm: &[f32; FRAME_SAMPLES],
        out: &'a mut [u8; MAX_PACKET],
    ) -> Result<Option<&'a [u8]>, AudioError> {
        let len = self.inner.encode_float(pcm, out)?;
        // libopus: a packet of 2 bytes or less does not need transmitting.
        Ok((len > 2).then(|| &out[..len]))
    }
}

/// Mono 48 kHz voice decoder.
pub struct VoiceDecoder {
    inner: opus::Decoder,
}

impl VoiceDecoder {
    pub fn new() -> Result<Self, AudioError> {
        Ok(Self {
            inner: opus::Decoder::new(SAMPLE_RATE, Channels::Mono)?,
        })
    }

    /// Decodes a received frame.
    pub fn decode(
        &mut self,
        packet: &[u8],
        out: &mut [f32; FRAME_SAMPLES],
    ) -> Result<(), AudioError> {
        self.inner.decode_float(packet, out, false)?;
        Ok(())
    }

    /// Rebuilds a lost frame from the FEC data in the frame after it.
    pub fn recover(
        &mut self,
        next: &[u8],
        out: &mut [f32; FRAME_SAMPLES],
    ) -> Result<(), AudioError> {
        self.inner.decode_float(next, out, true)?;
        Ok(())
    }

    /// Fills a lost frame with packet loss concealment.
    pub fn conceal(&mut self, out: &mut [f32; FRAME_SAMPLES]) -> Result<(), AudioError> {
        self.inner.decode_float(&[], out, false)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone() -> [f32; FRAME_SAMPLES] {
        std::array::from_fn(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5)
    }

    #[test]
    fn round_trips_a_tone() {
        let mut encoder = VoiceEncoder::new().unwrap();
        let mut decoder = VoiceDecoder::new().unwrap();
        let mut packet = [0u8; MAX_PACKET];
        let mut pcm = [0f32; FRAME_SAMPLES];
        for _ in 0..5 {
            let encoded = encoder.encode(&tone(), &mut packet).unwrap().unwrap();
            assert!(encoded.len() < 200);
            decoder.decode(encoded, &mut pcm).unwrap();
        }
        let energy: f32 = pcm.iter().map(|s| s * s).sum();
        assert!(energy > 1.0, "decoded tone is silent: {energy}");
    }

    #[test]
    fn dtx_skips_silence() {
        let mut encoder = VoiceEncoder::new().unwrap();
        let mut packet = [0u8; MAX_PACKET];
        let skipped = (0..50)
            .filter(|_| {
                encoder
                    .encode(&[0.0; FRAME_SAMPLES], &mut packet)
                    .unwrap()
                    .is_none()
            })
            .count();
        assert!(skipped > 0);
    }

    #[test]
    fn conceals_lost_frame() {
        let mut decoder = VoiceDecoder::new().unwrap();
        let mut pcm = [0f32; FRAME_SAMPLES];
        decoder.conceal(&mut pcm).unwrap();
    }
}
