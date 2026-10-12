//! Voice datagram header. Layout: `docs/PROTOCOL.md`, "Voice datagram".

use std::fmt;
use std::time::Duration;

/// Size of the fixed voice datagram header in bytes.
pub const VOICE_HEADER_LEN: usize = 16;

/// Voice datagram format version written by this build.
pub const VOICE_VERSION: u8 = 1;

/// One voice frame, and the unit of the header timestamp.
pub const FRAME_DURATION: Duration = Duration::from_millis(20);

const FLAG_END_OF_TRANSMISSION: u8 = 0b0000_0001;

/// Header that precedes each Opus frame in a voice datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceHeader {
    /// Talkgroup or private-call identifier.
    pub group_id: u32,
    /// Radio ID of the talker.
    pub sender: u32,
    /// Incremented on each key-up; wraps.
    pub transmission_id: u16,
    /// Starts at 0 at key-up; +1 per datagram sent.
    pub sequence: u16,
    /// Starts at 0 at key-up; counts 20 ms frames.
    pub timestamp: u16,
    /// Last datagram of this transmission.
    pub end_of_transmission: bool,
}

impl VoiceHeader {
    /// Encodes the header. Reserved flag bits are written as 0.
    pub fn encode(&self) -> [u8; VOICE_HEADER_LEN] {
        let mut buf = [0u8; VOICE_HEADER_LEN];
        buf[0] = VOICE_VERSION;
        if self.end_of_transmission {
            buf[1] |= FLAG_END_OF_TRANSMISSION;
        }
        buf[2..6].copy_from_slice(&self.group_id.to_be_bytes());
        buf[6..10].copy_from_slice(&self.sender.to_be_bytes());
        buf[10..12].copy_from_slice(&self.transmission_id.to_be_bytes());
        buf[12..14].copy_from_slice(&self.sequence.to_be_bytes());
        buf[14..16].copy_from_slice(&self.timestamp.to_be_bytes());
        buf
    }

    /// Splits a datagram into its header and payload. Reserved flag bits
    /// are ignored.
    pub fn decode(datagram: &[u8]) -> Result<(Self, &[u8]), DecodeError> {
        let Some((header, payload)) = datagram.split_first_chunk::<VOICE_HEADER_LEN>() else {
            return Err(DecodeError::TooShort(datagram.len()));
        };
        if header[0] != VOICE_VERSION {
            return Err(DecodeError::UnsupportedVersion(header[0]));
        }
        let be16 = |i: usize| u16::from_be_bytes([header[i], header[i + 1]]);
        let be32 =
            |i: usize| u32::from_be_bytes([header[i], header[i + 1], header[i + 2], header[i + 3]]);
        let decoded = Self {
            group_id: be32(2),
            sender: be32(6),
            transmission_id: be16(10),
            sequence: be16(12),
            timestamp: be16(14),
            end_of_transmission: header[1] & FLAG_END_OF_TRANSMISSION != 0,
        };
        Ok((decoded, payload))
    }
}

/// Why a voice datagram was rejected. Receivers drop such datagrams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Shorter than [`VOICE_HEADER_LEN`]; holds the actual length.
    TooShort(usize),
    /// Version byte this build does not understand.
    UnsupportedVersion(u8),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort(len) => write!(
                f,
                "voice datagram is {len} bytes, shorter than the {VOICE_HEADER_LEN}-byte header"
            ),
            Self::UnsupportedVersion(v) => write!(f, "unsupported voice datagram version {v}"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: VoiceHeader = VoiceHeader {
        group_id: 0x0102_0304,
        sender: 0x0A0B_0C0D,
        transmission_id: 0x1122,
        sequence: 0x3344,
        timestamp: 0x5566,
        end_of_transmission: true,
    };

    #[test]
    fn encodes_documented_layout() {
        assert_eq!(
            HEADER.encode(),
            [
                1, 0x01, 0x01, 0x02, 0x03, 0x04, 0x0A, 0x0B, 0x0C, 0x0D, 0x11, 0x22, 0x33, 0x44,
                0x55, 0x66
            ]
        );
    }

    #[test]
    fn round_trips_with_payload() {
        let mut datagram = HEADER.encode().to_vec();
        datagram.extend_from_slice(b"opus");
        assert_eq!(VoiceHeader::decode(&datagram), Ok((HEADER, &b"opus"[..])));
    }

    #[test]
    fn accepts_empty_payload() {
        let encoded = HEADER.encode();
        assert_eq!(VoiceHeader::decode(&encoded), Ok((HEADER, &[][..])));
    }

    #[test]
    fn ignores_reserved_flags() {
        let mut encoded = VoiceHeader {
            end_of_transmission: false,
            ..HEADER
        }
        .encode();
        encoded[1] = 0b1111_1110;
        let (decoded, _) = VoiceHeader::decode(&encoded).unwrap();
        assert!(!decoded.end_of_transmission);
    }

    #[test]
    fn rejects_short_datagram() {
        assert_eq!(
            VoiceHeader::decode(&HEADER.encode()[..15]),
            Err(DecodeError::TooShort(15))
        );
    }

    #[test]
    fn rejects_unknown_version() {
        let mut encoded = HEADER.encode();
        encoded[0] = 2;
        assert_eq!(
            VoiceHeader::decode(&encoded),
            Err(DecodeError::UnsupportedVersion(2))
        );
    }
}
