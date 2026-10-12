//! Node identity: a persistent Ed25519 keypair (ADR-0008).
//!
//! The keypair is generated on first run and stored as PKCS#8 DER in the
//! user's config folder. The fingerprint is the SHA-256 of the raw public
//! key, and the radio ID is the fingerprint's first four bytes, so both
//! stay the same across runs. Never commit identity files.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};

mod der;

/// Length of an Ed25519 public key.
pub const PUBLIC_KEY_LEN: usize = 32;

/// This node's Ed25519 keypair.
pub struct Identity {
    pkcs8: Vec<u8>,
    public_key: [u8; PUBLIC_KEY_LEN],
}

impl Identity {
    /// Loads the identity at `path`, or generates and saves a new one if
    /// the file does not exist.
    pub fn load_or_create(path: &Path) -> Result<Self, IdentityError> {
        match fs::read(path) {
            Ok(pkcs8) => Self::from_pkcs8(pkcs8),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let identity = Self::generate()?;
                match identity.save_new(path) {
                    Ok(()) => Ok(identity),
                    // Another process created it first; use theirs.
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        Self::from_pkcs8(fs::read(path)?)
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Generates a new identity in memory without saving it.
    pub fn generate() -> Result<Self, IdentityError> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| IdentityError::KeyGeneration)?;
        Self::from_pkcs8(pkcs8.as_ref().to_vec())
    }

    fn from_pkcs8(pkcs8: Vec<u8>) -> Result<Self, IdentityError> {
        let keypair = Ed25519KeyPair::from_pkcs8(&pkcs8).map_err(|_| IdentityError::InvalidKey)?;
        let public_key = keypair
            .public_key()
            .as_ref()
            .try_into()
            .map_err(|_| IdentityError::InvalidKey)?;
        Ok(Self { pkcs8, public_key })
    }

    /// Writes the key to a new file, failing if one already exists. Only
    /// the owner can read it on Unix; on Windows the per-user config
    /// folder already restricts access.
    fn save_new(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(path)?;
        file.write_all(&self.pkcs8)?;
        file.sync_all()
    }

    /// Default location for an identity file named `name`:
    /// `%APPDATA%\hangtime` on Windows, `~/.config/hangtime` on Linux,
    /// `~/Library/Application Support/hangtime` on macOS.
    pub fn default_path(name: &str) -> Option<PathBuf> {
        directories::BaseDirs::new().map(|dirs| dirs.config_dir().join("hangtime").join(name))
    }

    /// The private key as PKCS#8 DER, for building the TLS certificate.
    pub fn pkcs8_der(&self) -> &[u8] {
        &self.pkcs8
    }

    pub fn public_key(&self) -> &[u8; PUBLIC_KEY_LEN] {
        &self.public_key
    }

    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of_public_key(&self.public_key)
    }

    pub fn radio_id(&self) -> u32 {
        self.fingerprint().radio_id()
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the private key.
        write!(f, "Identity({})", self.fingerprint())
    }
}

/// SHA-256 of a node's Ed25519 public key. Shown to users to decide trust.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub fn of_public_key(public_key: &[u8; PUBLIC_KEY_LEN]) -> Self {
        let digest = ring::digest::digest(&ring::digest::SHA256, public_key);
        Self(digest.as_ref().try_into().expect("SHA-256 is 32 bytes"))
    }

    /// Fingerprint of the Ed25519 key in an X.509 certificate. `None` if
    /// the certificate is malformed or holds another key type.
    pub fn of_certificate(certificate_der: &[u8]) -> Option<Self> {
        der::ed25519_public_key(certificate_der).map(|key| Self::of_public_key(&key))
    }

    /// Radio ID: the fingerprint's first four bytes.
    pub fn radio_id(&self) -> u32 {
        u32::from_be_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Lowercase hex without separators, for TXT records and comparison.
    pub fn hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, byte) in self.0.iter().enumerate() {
            if i > 0 && i % 2 == 0 {
                f.write_str(":")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

#[derive(Debug)]
pub enum IdentityError {
    Io(io::Error),
    /// The stored key is not a valid Ed25519 PKCS#8 key.
    InvalidKey,
    KeyGeneration,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "identity file: {e}"),
            Self::InvalidKey => write!(f, "identity file does not hold a valid Ed25519 key"),
            Self::KeyGeneration => write!(f, "could not generate an identity key"),
        }
    }
}

impl std::error::Error for IdentityError {}

impl From<io::Error> for IdentityError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hangtime-identity-test-{}", std::process::id()));
        dir.join(name)
    }

    #[test]
    fn persists_across_loads() {
        let path = temp_path("persist.key");
        let _ = fs::remove_file(&path);
        let first = Identity::load_or_create(&path).unwrap();
        let second = Identity::load_or_create(&path).unwrap();
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_eq!(first.radio_id(), second.radio_id());
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn rejects_corrupt_file() {
        let path = temp_path("corrupt.key");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"not a key").unwrap();
        assert!(matches!(
            Identity::load_or_create(&path),
            Err(IdentityError::InvalidKey)
        ));
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn radio_id_is_fingerprint_prefix() {
        let fingerprint = Fingerprint([0xab; 32]);
        assert_eq!(fingerprint.radio_id(), 0xabab_abab);
        assert!(fingerprint.to_string().starts_with("abab:abab:"));
        assert_eq!(fingerprint.hex(), "ab".repeat(32));
    }

    #[test]
    fn debug_hides_private_key() {
        let identity = Identity::generate().unwrap();
        assert!(format!("{identity:?}").starts_with("Identity("));
    }
}
