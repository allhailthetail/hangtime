//! Talkgroup membership proofs (ADR-0009).
//!
//! A scoped group's key is a shared passphrase. Possession of the key is
//! membership: `JOIN` carries
//! `HMAC-SHA256(key, "hangtime-join" || group ID || joiner fingerprint ||
//! recipient fingerprint)`. Binding both fingerprints, in that order,
//! means a proof sent to one peer is useless to anyone else.

use ring::hmac;

const CONTEXT: &[u8] = b"hangtime-join";

/// A talkgroup this node knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: u32,
    pub name: String,
    /// Shared passphrase. `None` for open groups and All Call.
    pub key: Option<String>,
}

impl Group {
    /// The proof this node (`joiner`) sends `recipient` in `JOIN`.
    /// `None` for open groups.
    pub fn join_proof(&self, joiner: &[u8; 32], recipient: &[u8; 32]) -> Option<[u8; 32]> {
        let key = self.key.as_ref()?;
        let tag = hmac::sign(&hmac_key(key), &message(self.id, joiner, recipient));
        Some(tag.as_ref().try_into().expect("HMAC-SHA256 is 32 bytes"))
    }

    /// Checks a `JOIN` from `joiner` to this node (`recipient`). Open
    /// groups accept any join; keyed groups need a valid proof.
    pub fn accepts_join(
        &self,
        proof: Option<&[u8; 32]>,
        joiner: &[u8; 32],
        recipient: &[u8; 32],
    ) -> bool {
        match (&self.key, proof) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(key), Some(proof)) => {
                hmac::verify(&hmac_key(key), &message(self.id, joiner, recipient), proof).is_ok()
            }
        }
    }
}

fn hmac_key(passphrase: &str) -> hmac::Key {
    hmac::Key::new(hmac::HMAC_SHA256, passphrase.as_bytes())
}

fn message(group_id: u32, joiner: &[u8; 32], recipient: &[u8; 32]) -> Vec<u8> {
    let mut message = Vec::with_capacity(CONTEXT.len() + 4 + 64);
    message.extend_from_slice(CONTEXT);
    message.extend_from_slice(&group_id.to_be_bytes());
    message.extend_from_slice(joiner);
    message.extend_from_slice(recipient);
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: [u8; 32] = [1; 32];
    const BOB: [u8; 32] = [2; 32];
    const MALLORY: [u8; 32] = [3; 32];

    fn ops(key: Option<&str>) -> Group {
        Group {
            id: 100,
            name: "Ops".into(),
            key: key.map(str::to_string),
        }
    }

    #[test]
    fn key_holders_verify_each_other() {
        let group = ops(Some("shared secret"));
        let proof = group.join_proof(&ALICE, &BOB).unwrap();
        assert!(group.accepts_join(Some(&proof), &ALICE, &BOB));
    }

    #[test]
    fn rejects_wrong_key_missing_proof_and_replay() {
        let group = ops(Some("shared secret"));
        let wrong = ops(Some("guess")).join_proof(&MALLORY, &BOB).unwrap();
        assert!(!group.accepts_join(Some(&wrong), &MALLORY, &BOB));
        assert!(!group.accepts_join(None, &MALLORY, &BOB));
        // Alice's proof to Bob can't be replayed by Mallory, or to Mallory.
        let proof = group.join_proof(&ALICE, &BOB).unwrap();
        assert!(!group.accepts_join(Some(&proof), &MALLORY, &BOB));
        assert!(!group.accepts_join(Some(&proof), &ALICE, &MALLORY));
    }

    #[test]
    fn proof_is_bound_to_the_group_id() {
        let group = ops(Some("shared secret"));
        let other = Group {
            id: 200,
            ..group.clone()
        };
        let proof = other.join_proof(&ALICE, &BOB).unwrap();
        assert!(!group.accepts_join(Some(&proof), &ALICE, &BOB));
    }

    #[test]
    fn open_groups_need_no_proof() {
        let group = ops(None);
        assert_eq!(group.join_proof(&ALICE, &BOB), None);
        assert!(group.accepts_join(None, &ALICE, &BOB));
    }
}
