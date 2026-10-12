//! Just enough DER to find the public key in an X.509 certificate.
//!
//! Walks the structure (Certificate -> TBSCertificate -> fields) rather
//! than searching for byte patterns, so a crafted certificate cannot plant
//! a decoy key in another field.

use crate::PUBLIC_KEY_LEN;

const SEQUENCE: u8 = 0x30;
const VERSION_TAG: u8 = 0xa0;

/// SubjectPublicKeyInfo for Ed25519 (RFC 8410) up to the key bytes:
/// SEQUENCE { SEQUENCE { OID 1.3.101.112 }, BIT STRING (0 unused bits) }.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// One DER element split off the front of a buffer.
struct Tlv<'a> {
    tag: u8,
    /// The whole element: tag, length, and value.
    whole: &'a [u8],
    value: &'a [u8],
    /// Everything after the element.
    rest: &'a [u8],
}

fn take_tlv(input: &[u8]) -> Option<Tlv<'_>> {
    let (&tag, after_tag) = input.split_first()?;
    let (&first, mut after_len) = after_tag.split_first()?;
    let len = if first < 0x80 {
        usize::from(first)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || after_len.len() < count {
            return None;
        }
        let mut len = 0usize;
        for &byte in &after_len[..count] {
            len = (len << 8) | usize::from(byte);
        }
        after_len = &after_len[count..];
        len
    };
    if after_len.len() < len {
        return None;
    }
    let header_len = input.len() - after_len.len();
    let (value, rest) = after_len.split_at(len);
    Some(Tlv {
        tag,
        whole: &input[..header_len + len],
        value,
        rest,
    })
}

/// Takes one element that must be a SEQUENCE.
fn take_sequence(input: &[u8]) -> Option<Tlv<'_>> {
    take_tlv(input).filter(|tlv| tlv.tag == SEQUENCE)
}

/// Returns the Ed25519 public key from a DER certificate, or `None` if
/// the certificate is malformed or holds a different key type.
pub fn ed25519_public_key(certificate: &[u8]) -> Option<[u8; PUBLIC_KEY_LEN]> {
    let cert = take_sequence(certificate)?.value;
    let tbs = take_sequence(cert)?.value;
    // TBSCertificate: [0] version (optional), serial, signature, issuer,
    // validity, subject, subjectPublicKeyInfo, ...
    let mut fields = tbs;
    if fields.first() == Some(&VERSION_TAG) {
        fields = take_tlv(fields)?.rest;
    }
    for _ in 0..5 {
        fields = take_tlv(fields)?.rest;
    }
    let spki = take_sequence(fields)?.whole;
    spki.strip_prefix(&ED25519_SPKI_PREFIX)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal certificate shaped like a real one around `spki`.
    fn certificate(spki: &[u8], with_version: bool) -> Vec<u8> {
        fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
            let mut out = vec![tag];
            if value.len() < 0x80 {
                out.push(value.len() as u8);
            } else {
                out.extend_from_slice(&[0x82, (value.len() >> 8) as u8, value.len() as u8]);
            }
            out.extend_from_slice(value);
            out
        }
        let mut tbs = Vec::new();
        if with_version {
            tbs.extend(tlv(VERSION_TAG, &tlv(0x02, &[2])));
        }
        tbs.extend(tlv(0x02, &[1])); // serial
        tbs.extend(tlv(SEQUENCE, &[0x06, 0x03, 0x2b, 0x65, 0x70])); // signature
        // A decoy key pattern inside the issuer must be ignored.
        let mut decoy = ED25519_SPKI_PREFIX.to_vec();
        decoy.extend_from_slice(&[0xee; 32]);
        tbs.extend(tlv(SEQUENCE, &decoy)); // issuer
        tbs.extend(tlv(SEQUENCE, &[])); // validity
        tbs.extend(tlv(SEQUENCE, &[])); // subject
        tbs.extend_from_slice(spki);
        let mut cert = tlv(SEQUENCE, &tbs);
        cert.extend(tlv(SEQUENCE, &[0x06, 0x03, 0x2b, 0x65, 0x70]));
        cert.extend(tlv(0x03, &[0; 65]));
        tlv(SEQUENCE, &cert)
    }

    fn ed25519_spki(key: [u8; 32]) -> Vec<u8> {
        let mut spki = ED25519_SPKI_PREFIX.to_vec();
        spki.extend_from_slice(&key);
        spki
    }

    #[test]
    fn finds_key_with_and_without_version() {
        for with_version in [true, false] {
            let cert = certificate(&ed25519_spki([7; 32]), with_version);
            assert_eq!(ed25519_public_key(&cert), Some([7; 32]));
        }
    }

    #[test]
    fn rejects_other_key_types() {
        // P-256 SubjectPublicKeyInfo header (truncated key is enough).
        let p256 = [
            0x30, 0x13, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01,
        ];
        assert_eq!(ed25519_public_key(&certificate(&p256, true)), None);
    }

    #[test]
    fn rejects_truncated_input() {
        let cert = certificate(&ed25519_spki([7; 32]), true);
        for len in [0, 1, 10, cert.len() - 1] {
            assert_eq!(ed25519_public_key(&cert[..len]), None);
        }
    }
}
