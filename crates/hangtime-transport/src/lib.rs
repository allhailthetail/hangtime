//! QUIC transport.
//!
//! Voice frames go in QUIC datagrams; control and signaling go on QUIC
//! streams. Voice is never sent on a reliable stream.
//!
//! Connection setup follows `docs/PROTOCOL.md`: ALPN `hangtime/1`, mutual
//! TLS with self-signed certificates built from each node's Ed25519
//! identity key (ADR-0008). Any Ed25519 certificate is accepted; callers
//! must show the peer as untrusted until the user confirms its
//! [`Fingerprint`] (ADR-0007).

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
pub use hangtime_identity::Fingerprint;
use hangtime_identity::Identity;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DigitallySignedStruct, DistinguishedName, SignatureScheme};

/// QUIC application protocol identifier. The suffix is the protocol
/// major version.
pub const ALPN: &[u8] = b"hangtime/1";

/// Default UDP port.
pub const DEFAULT_PORT: u16 = 4747;

/// Server name sent in the TLS handshake. Certificates are not checked
/// against it.
const SERVER_NAME: &str = "hangtime";

/// A local QUIC endpoint that both accepts and makes connections.
pub struct Node {
    endpoint: quinn::Endpoint,
    fingerprint: Fingerprint,
}

impl Node {
    /// Binds a UDP socket, presenting a self-signed certificate for
    /// `identity`. Must be called inside a Tokio runtime.
    pub fn bind(addr: SocketAddr, identity: &Identity) -> Result<Self, TransportError> {
        let key = PrivatePkcs8KeyDer::from(identity.pkcs8_der().to_vec());
        let signing_key = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&key, &rcgen::PKCS_ED25519)?;
        let cert = rcgen::CertificateParams::new(vec![SERVER_NAME.to_string()])?
            .self_signed(&signing_key)?
            .der()
            .clone();
        let fingerprint = identity.fingerprint();

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(AcceptAnyCert(Arc::clone(&provider)));

        let mut server = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_client_cert_verifier(verifier.clone())
            .with_single_cert(vec![cert.clone()], key.clone_key().into())?;
        server.alpn_protocols = vec![ALPN.to_vec()];

        let mut client = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_auth_cert(vec![cert], key.into())?;
        client.alpn_protocols = vec![ALPN.to_vec()];

        let server = quinn::ServerConfig::with_crypto(Arc::new(
            QuicServerConfig::try_from(server)
                .map_err(|e| TransportError::Config(e.to_string()))?,
        ));
        let client = quinn::ClientConfig::new(Arc::new(
            QuicClientConfig::try_from(client)
                .map_err(|e| TransportError::Config(e.to_string()))?,
        ));
        let mut endpoint = quinn::Endpoint::server(server, addr)?;
        endpoint.set_default_client_config(client);
        Ok(Self {
            endpoint,
            fingerprint,
        })
    }

    /// This node's identity fingerprint.
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Connects to a peer.
    pub async fn connect(&self, addr: SocketAddr) -> Result<Peer, TransportError> {
        let connection = self.endpoint.connect(addr, SERVER_NAME)?.await?;
        Peer::new(connection)
    }

    /// Waits for the next incoming connection. `None` once the endpoint
    /// is closed.
    pub async fn accept(&self) -> Option<Result<Peer, TransportError>> {
        let incoming = self.endpoint.accept().await?;
        Some(match incoming.await {
            Ok(connection) => Peer::new(connection),
            Err(e) => Err(e.into()),
        })
    }
}

/// An established connection to another node. Cheap to clone.
#[derive(Clone)]
pub struct Peer {
    connection: quinn::Connection,
    fingerprint: Fingerprint,
}

impl Peer {
    fn new(connection: quinn::Connection) -> Result<Self, TransportError> {
        let fingerprint = connection
            .peer_identity()
            .and_then(|identity| identity.downcast::<Vec<CertificateDer<'static>>>().ok())
            .and_then(|certs| {
                certs
                    .first()
                    .and_then(|cert| Fingerprint::of_certificate(cert))
            })
            .ok_or(TransportError::NoPeerIdentity)?;
        Ok(Self {
            connection,
            fingerprint,
        })
    }

    /// The peer's identity fingerprint. Untrusted until the user
    /// confirms it.
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub fn remote_address(&self) -> SocketAddr {
        self.connection.remote_address()
    }

    /// Current round-trip time estimate from QUIC.
    pub fn rtt(&self) -> std::time::Duration {
        self.connection.rtt()
    }

    /// Sends one voice datagram. Never retransmitted.
    pub fn send_voice(&self, datagram: Vec<u8>) -> Result<(), TransportError> {
        Ok(self.connection.send_datagram(Bytes::from(datagram))?)
    }

    /// Waits for the next voice datagram.
    pub async fn recv_voice(&self) -> Result<Bytes, TransportError> {
        Ok(self.connection.read_datagram().await?)
    }
}

/// Accepts any Ed25519 certificate and still checks handshake signatures,
/// so the peer must hold the private key for the certificate it presents.
#[derive(Debug)]
struct AcceptAnyCert(Arc<CryptoProvider>);

impl AcceptAnyCert {
    fn verify12(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify13(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn require_ed25519(cert: &CertificateDer<'_>) -> Result<(), rustls::Error> {
    match Fingerprint::of_certificate(cert) {
        Some(_) => Ok(()),
        None => Err(rustls::Error::InvalidCertificate(
            CertificateError::BadEncoding,
        )),
    }
}

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        require_ed25519(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verify12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verify13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

impl ClientCertVerifier for AcceptAnyCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        require_ed25519(end_entity)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verify12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verify13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.schemes()
    }
}

#[derive(Debug)]
pub enum TransportError {
    Io(std::io::Error),
    Tls(rustls::Error),
    Certificate(rcgen::Error),
    Config(String),
    Connect(quinn::ConnectError),
    Connection(quinn::ConnectionError),
    SendVoice(quinn::SendDatagramError),
    /// The peer completed the handshake without an Ed25519 certificate.
    NoPeerIdentity,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "socket: {e}"),
            Self::Tls(e) => write!(f, "TLS: {e}"),
            Self::Certificate(e) => write!(f, "certificate: {e}"),
            Self::Config(e) => write!(f, "QUIC config: {e}"),
            Self::Connect(e) => write!(f, "connect: {e}"),
            Self::Connection(e) => write!(f, "connection: {e}"),
            Self::SendVoice(e) => write!(f, "send voice: {e}"),
            Self::NoPeerIdentity => write!(f, "peer presented no Ed25519 identity"),
        }
    }
}

impl std::error::Error for TransportError {}

macro_rules! from_error {
    ($($source:ty => $variant:ident),* $(,)?) => {
        $(impl From<$source> for TransportError {
            fn from(e: $source) -> Self {
                Self::$variant(e)
            }
        })*
    };
}

from_error! {
    std::io::Error => Io,
    rustls::Error => Tls,
    rcgen::Error => Certificate,
    quinn::ConnectError => Connect,
    quinn::ConnectionError => Connection,
    quinn::SendDatagramError => SendVoice,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exchanges_voice_datagrams_with_mutual_fingerprints() {
        let localhost: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = Node::bind(localhost, &Identity::generate().unwrap()).unwrap();
        let dialer = Node::bind(localhost, &Identity::generate().unwrap()).unwrap();
        let listen_addr = listener.local_addr().unwrap();

        let (accepted, connected) = tokio::join!(listener.accept(), dialer.connect(listen_addr));
        let accepted = accepted.unwrap().unwrap();
        let connected = connected.unwrap();

        assert_eq!(accepted.fingerprint(), dialer.fingerprint());
        assert_eq!(connected.fingerprint(), listener.fingerprint());

        connected.send_voice(b"voice".to_vec()).unwrap();
        assert_eq!(&accepted.recv_voice().await.unwrap()[..], b"voice");
    }
}
