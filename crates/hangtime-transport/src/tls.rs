//! TLS setup: self-signed certificates from the identity key, mutual
//! authentication, and acceptance of any Ed25519 peer.

use std::sync::Arc;
use std::time::Duration;

use hangtime_identity::{Fingerprint, Identity};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DigitallySignedStruct, DistinguishedName, SignatureScheme};

use crate::{ALPN, SERVER_NAME, TransportError};

/// How often an idle connection is pinged.
const KEEP_ALIVE: Duration = Duration::from_secs(5);

/// How long a connection may go without hearing from the peer.
const IDLE_TIMEOUT: Duration = Duration::from_secs(20);

/// Builds the QUIC server and client configs for `identity`.
pub(crate) fn configs(
    identity: &Identity,
) -> Result<(quinn::ServerConfig, quinn::ClientConfig), TransportError> {
    let key = PrivatePkcs8KeyDer::from(identity.pkcs8_der().to_vec());
    let signing_key = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(&key, &rcgen::PKCS_ED25519)?;
    let cert = rcgen::CertificateParams::new(vec![SERVER_NAME.to_string()])?
        .self_signed(&signing_key)?
        .der()
        .clone();

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(AcceptAnyEd25519(Arc::clone(&provider)));

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

    let transport = Arc::new(transport_config());
    let mut server = quinn::ServerConfig::with_crypto(Arc::new(
        QuicServerConfig::try_from(server).map_err(|e| TransportError::Config(e.to_string()))?,
    ));
    server.transport_config(Arc::clone(&transport));
    let mut client = quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(client).map_err(|e| TransportError::Config(e.to_string()))?,
    ));
    client.transport_config(transport);
    Ok((server, client))
}

/// Radio links sit quiet for long stretches between transmissions, so
/// both ends ping to keep an idle connection open. A peer that stops
/// answering is dropped after the idle timeout.
fn transport_config() -> quinn::TransportConfig {
    let mut config = quinn::TransportConfig::default();
    config.keep_alive_interval(Some(KEEP_ALIVE));
    config.max_idle_timeout(Some(
        IDLE_TIMEOUT
            .try_into()
            .expect("idle timeout fits a QUIC varint"),
    ));
    config
}

/// Fingerprint of the peer's identity, from its certificate.
pub(crate) fn peer_fingerprint(connection: &quinn::Connection) -> Option<Fingerprint> {
    let certs = connection
        .peer_identity()?
        .downcast::<Vec<CertificateDer<'static>>>()
        .ok()?;
    Fingerprint::of_certificate(certs.first()?)
}

/// Accepts any Ed25519 certificate and still checks handshake signatures,
/// so the peer must hold the private key for the certificate it presents.
#[derive(Debug)]
struct AcceptAnyEd25519(Arc<CryptoProvider>);

impl AcceptAnyEd25519 {
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

impl ServerCertVerifier for AcceptAnyEd25519 {
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

impl ClientCertVerifier for AcceptAnyEd25519 {
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
