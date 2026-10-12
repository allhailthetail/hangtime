//! Control messages. Layout: `docs/PROTOCOL.md`, "Control messages".
//!
//! Each message on the control stream is framed as
//! `type (u16) | length (u16) | body`, big-endian. Unknown types are
//! skipped, and bytes after the fields a receiver knows are ignored, so
//! later minor versions can add messages and append fields.

use std::fmt;

/// Size of the frame header: type and body length.
pub const FRAME_HEADER_LEN: usize = 4;

/// Longest display name sent in `HELLO`, in bytes.
pub const MAX_DISPLAY_NAME: usize = 32;

/// Length of a `JOIN` membership proof (HMAC-SHA256).
pub const PROOF_LEN: usize = 32;

/// Minor protocol version this build speaks within `hangtime/2`.
pub const MINOR_VERSION: u16 = 0;

const HELLO: u16 = 0x0001;
const JOIN: u16 = 0x0010;
const LEAVE: u16 = 0x0011;
const KEY_UP: u16 = 0x0020;
const KEY_DOWN: u16 = 0x0021;
// 0x0023 FLOOR_GRANT and 0x0024 FLOOR_DENY are reserved for server mode.

/// A message on the control stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMessage {
    /// First message each side sends.
    Hello {
        minor_version: u16,
        radio_id: u32,
        capabilities: u32,
        display_name: String,
    },
    /// Start monitoring a group. Scoped groups carry a membership proof.
    Join {
        group_id: u32,
        proof: Option<[u8; PROOF_LEN]>,
    },
    /// Stop monitoring a group.
    Leave { group_id: u32 },
    /// Request the floor for a transmission.
    KeyUp { group_id: u32, transmission_id: u16 },
    /// Release the floor; starts hang time.
    KeyDown { group_id: u32, transmission_id: u16 },
}

impl ControlMessage {
    /// Encodes the message as a complete frame.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        let kind = match self {
            Self::Hello {
                minor_version,
                radio_id,
                capabilities,
                display_name,
            } => {
                body.extend_from_slice(&minor_version.to_be_bytes());
                body.extend_from_slice(&radio_id.to_be_bytes());
                body.extend_from_slice(&capabilities.to_be_bytes());
                let name = truncate_utf8(display_name, MAX_DISPLAY_NAME);
                body.push(name.len() as u8);
                body.extend_from_slice(name.as_bytes());
                HELLO
            }
            Self::Join { group_id, proof } => {
                body.extend_from_slice(&group_id.to_be_bytes());
                match proof {
                    Some(proof) => {
                        body.push(PROOF_LEN as u8);
                        body.extend_from_slice(proof);
                    }
                    None => body.push(0),
                }
                JOIN
            }
            Self::Leave { group_id } => {
                body.extend_from_slice(&group_id.to_be_bytes());
                LEAVE
            }
            Self::KeyUp {
                group_id,
                transmission_id,
            } => {
                body.extend_from_slice(&group_id.to_be_bytes());
                body.extend_from_slice(&transmission_id.to_be_bytes());
                KEY_UP
            }
            Self::KeyDown {
                group_id,
                transmission_id,
            } => {
                body.extend_from_slice(&group_id.to_be_bytes());
                body.extend_from_slice(&transmission_id.to_be_bytes());
                KEY_DOWN
            }
        };
        let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + body.len());
        frame.extend_from_slice(&kind.to_be_bytes());
        frame.extend_from_slice(&(body.len() as u16).to_be_bytes());
        frame.extend_from_slice(&body);
        frame
    }

    /// Parses a frame header into (message type, body length).
    pub fn decode_header(header: [u8; FRAME_HEADER_LEN]) -> (u16, usize) {
        (
            u16::from_be_bytes([header[0], header[1]]),
            usize::from(u16::from_be_bytes([header[2], header[3]])),
        )
    }

    /// Decodes a message body. `Ok(None)` for message types this build
    /// does not know, which receivers skip.
    pub fn decode(kind: u16, body: &[u8]) -> Result<Option<Self>, ControlError> {
        let mut reader = Reader(body);
        let message = match kind {
            HELLO => {
                let minor_version = reader.u16()?;
                let radio_id = reader.u32()?;
                let capabilities = reader.u32()?;
                let len = usize::from(reader.u8()?);
                let name = reader.bytes(len)?;
                let display_name = String::from_utf8_lossy(name).into_owned();
                Self::Hello {
                    minor_version,
                    radio_id,
                    capabilities,
                    display_name,
                }
            }
            JOIN => {
                let group_id = reader.u32()?;
                let proof = match usize::from(reader.u8()?) {
                    0 => None,
                    PROOF_LEN => Some(reader.bytes(PROOF_LEN)?.try_into().expect("checked length")),
                    _ => return Err(ControlError::Malformed),
                };
                Self::Join { group_id, proof }
            }
            LEAVE => Self::Leave {
                group_id: reader.u32()?,
            },
            KEY_UP => Self::KeyUp {
                group_id: reader.u32()?,
                transmission_id: reader.u16()?,
            },
            KEY_DOWN => Self::KeyDown {
                group_id: reader.u32()?,
                transmission_id: reader.u16()?,
            },
            _ => return Ok(None),
        };
        Ok(Some(message))
    }
}

/// Cuts `s` to at most `max` bytes without splitting a character.
fn truncate_utf8(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn bytes(&mut self, len: usize) -> Result<&'a [u8], ControlError> {
        if self.0.len() < len {
            return Err(ControlError::Malformed);
        }
        let (head, rest) = self.0.split_at(len);
        self.0 = rest;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8, ControlError> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ControlError> {
        Ok(u16::from_be_bytes(
            self.bytes(2)?.try_into().expect("2 bytes"),
        ))
    }

    fn u32(&mut self) -> Result<u32, ControlError> {
        Ok(u32::from_be_bytes(
            self.bytes(4)?.try_into().expect("4 bytes"),
        ))
    }
}

/// A control message body was too short or inconsistent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlError {
    Malformed,
}

impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed control message")
    }
}

impl std::error::Error for ControlError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(message: ControlMessage) -> ControlMessage {
        let frame = message.encode();
        let (kind, len) = ControlMessage::decode_header(frame[..4].try_into().unwrap());
        assert_eq!(len, frame.len() - FRAME_HEADER_LEN);
        ControlMessage::decode(kind, &frame[FRAME_HEADER_LEN..])
            .unwrap()
            .unwrap()
    }

    #[test]
    fn round_trips_every_message() {
        let messages = [
            ControlMessage::Hello {
                minor_version: 0,
                radio_id: 0x1234_5678,
                capabilities: 0,
                display_name: "desk".into(),
            },
            ControlMessage::Join {
                group_id: 7,
                proof: Some([9; PROOF_LEN]),
            },
            ControlMessage::Join {
                group_id: 7,
                proof: None,
            },
            ControlMessage::Leave { group_id: 7 },
            ControlMessage::KeyUp {
                group_id: 7,
                transmission_id: 42,
            },
            ControlMessage::KeyDown {
                group_id: 7,
                transmission_id: 42,
            },
        ];
        for message in messages {
            assert_eq!(round_trip(message.clone()), message);
        }
    }

    #[test]
    fn encodes_documented_key_up_layout() {
        let frame = ControlMessage::KeyUp {
            group_id: 0x0102_0304,
            transmission_id: 0x0506,
        }
        .encode();
        assert_eq!(frame, [0x00, 0x20, 0x00, 0x06, 1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn truncates_long_names_on_a_char_boundary() {
        let name = "é".repeat(20); // 40 bytes
        let ControlMessage::Hello { display_name, .. } = round_trip(ControlMessage::Hello {
            minor_version: 0,
            radio_id: 1,
            capabilities: 0,
            display_name: name,
        }) else {
            panic!("not a hello");
        };
        assert_eq!(display_name, "é".repeat(16));
    }

    #[test]
    fn skips_unknown_types_and_ignores_trailing_bytes() {
        assert_eq!(ControlMessage::decode(0x7fff, &[1, 2, 3]), Ok(None));
        let mut body = 7u32.to_be_bytes().to_vec();
        body.extend_from_slice(&[0xff; 8]);
        assert_eq!(
            ControlMessage::decode(LEAVE, &body),
            Ok(Some(ControlMessage::Leave { group_id: 7 }))
        );
    }

    #[test]
    fn rejects_short_bodies() {
        assert_eq!(
            ControlMessage::decode(KEY_UP, &[0, 0, 0, 7]),
            Err(ControlError::Malformed)
        );
        assert_eq!(
            ControlMessage::decode(JOIN, &[0, 0, 0, 7, 5]),
            Err(ControlError::Malformed)
        );
    }
}
