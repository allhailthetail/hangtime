//! QUIC transport.
//!
//! Voice frames go in QUIC datagrams; control and signaling go on QUIC
//! streams. Voice is never sent on a reliable stream.
//!
//! Connection setup follows `docs/PROTOCOL.md`: ALPN `hangtime/2`, mutual
//! TLS with self-signed certificates built from each node's Ed25519
//! identity key (ADR-0008), then a `HELLO` exchange on the control
//! stream. Any Ed25519 identity is accepted; callers must show the peer as
//! untrusted until the user confirms its [`Fingerprint`].

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
pub use hangtime_identity::Fingerprint;
use hangtime_identity::Identity;
use hangtime_proto::{ControlError, ControlMessage, FRAME_HEADER_LEN, MINOR_VERSION};
use tokio::sync::{Mutex, mpsc};

mod tls;

/// QUIC application protocol identifier. The suffix is the protocol
/// major version.
pub const ALPN: &[u8] = b"hangtime/2";

/// Default UDP port.
pub const DEFAULT_PORT: u16 = 4747;

/// Server name sent in the TLS handshake. Certificates are not checked
/// against it.
const SERVER_NAME: &str = "hangtime";

/// How long a new connection may take to exchange `HELLO`.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// A local QUIC endpoint that both accepts and makes connections.
pub struct Node {
    endpoint: quinn::Endpoint,
    fingerprint: Fingerprint,
    hello: ControlMessage,
}

impl Node {
    /// Binds a UDP socket, presenting a certificate for `identity` and
    /// introducing itself as `display_name`. Must be called inside a Tokio
    /// runtime.
    pub fn bind(
        addr: SocketAddr,
        identity: &Identity,
        display_name: &str,
    ) -> Result<Self, TransportError> {
        let (server, client) = tls::configs(identity)?;
        let mut endpoint = quinn::Endpoint::server(server, addr)?;
        endpoint.set_default_client_config(client);
        Ok(Self {
            endpoint,
            fingerprint: identity.fingerprint(),
            hello: ControlMessage::Hello {
                minor_version: MINOR_VERSION,
                radio_id: identity.radio_id(),
                capabilities: 0,
                display_name: display_name.to_string(),
            },
        })
    }

    /// This node's identity fingerprint.
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Connects to a peer. The dialer opens the control stream and says
    /// `HELLO` first.
    pub async fn connect(&self, addr: SocketAddr) -> Result<Peer, TransportError> {
        dial(self.endpoint.clone(), self.hello.clone(), addr).await
    }

    /// Connects to whichever of `addrs` answers first. A node advertises
    /// every interface address, and some (VPNs, virtual adapters) are
    /// unreachable, so trying them one by one could stall on timeouts.
    pub async fn connect_any(&self, addrs: &[SocketAddr]) -> Result<Peer, TransportError> {
        let mut attempts = tokio::task::JoinSet::new();
        for &addr in addrs {
            attempts.spawn(dial(self.endpoint.clone(), self.hello.clone(), addr));
        }
        let mut last_error = TransportError::Closed;
        while let Some(attempt) = attempts.join_next().await {
            match attempt {
                // Dropping the set aborts the slower attempts.
                Ok(Ok(peer)) => return Ok(peer),
                Ok(Err(e)) => last_error = e,
                Err(_) => {}
            }
        }
        Err(last_error)
    }

    /// Waits for the next incoming connection. `None` once the endpoint
    /// is closed.
    pub async fn accept(&self) -> Option<Result<Peer, TransportError>> {
        let incoming = self.endpoint.accept().await?;
        let handshake = async {
            let connection = incoming.await?;
            let (mut send, mut recv) = connection.accept_bi().await?;
            let hello = read_message(&mut recv).await?;
            send.write_all(&self.hello.encode()).await?;
            Peer::new(connection, send, recv, hello)
        };
        Some(
            tokio::time::timeout(HELLO_TIMEOUT, handshake)
                .await
                .unwrap_or(Err(TransportError::HelloTimeout)),
        )
    }
}

async fn dial(
    endpoint: quinn::Endpoint,
    hello: ControlMessage,
    addr: SocketAddr,
) -> Result<Peer, TransportError> {
    let handshake = async {
        let connection = endpoint.connect(addr, SERVER_NAME)?.await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        send.write_all(&hello.encode()).await?;
        let hello = read_message(&mut recv).await?;
        Peer::new(connection, send, recv, hello)
    };
    tokio::time::timeout(HELLO_TIMEOUT, handshake)
        .await
        .map_err(|_| TransportError::HelloTimeout)?
}

/// Reads the next control message this build knows, skipping unknown
/// types.
async fn read_message(recv: &mut quinn::RecvStream) -> Result<ControlMessage, TransportError> {
    loop {
        let mut header = [0u8; FRAME_HEADER_LEN];
        recv.read_exact(&mut header).await?;
        let (kind, len) = ControlMessage::decode_header(header);
        let mut body = vec![0u8; len];
        recv.read_exact(&mut body).await?;
        if let Some(message) = ControlMessage::decode(kind, &body)? {
            return Ok(message);
        }
    }
}

/// An established connection to another node. Cheap to clone.
#[derive(Clone)]
pub struct Peer {
    connection: quinn::Connection,
    fingerprint: Fingerprint,
    display_name: Arc<str>,
    control_out: mpsc::UnboundedSender<Vec<u8>>,
    control_in: Arc<Mutex<quinn::RecvStream>>,
}

impl Peer {
    fn new(
        connection: quinn::Connection,
        mut send: quinn::SendStream,
        recv: quinn::RecvStream,
        hello: ControlMessage,
    ) -> Result<Self, TransportError> {
        let fingerprint =
            tls::peer_fingerprint(&connection).ok_or(TransportError::NoPeerIdentity)?;
        // The claimed radio ID must match the key the peer proved it holds.
        let ControlMessage::Hello {
            radio_id,
            display_name,
            ..
        } = hello
        else {
            return Err(TransportError::BadHello);
        };
        if radio_id != fingerprint.radio_id() {
            return Err(TransportError::BadHello);
        }
        // Control writes go through one task so callers never block.
        let (control_out, mut outgoing) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            while let Some(frame) = outgoing.recv().await {
                if send.write_all(&frame).await.is_err() {
                    return;
                }
            }
        });
        Ok(Self {
            connection,
            fingerprint,
            display_name: display_name.into(),
            control_out,
            control_in: Arc::new(Mutex::new(recv)),
        })
    }

    /// The peer's identity fingerprint. Untrusted until the user
    /// confirms it.
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    /// The peer's radio ID, checked against its identity key.
    pub fn radio_id(&self) -> u32 {
        self.fingerprint.radio_id()
    }

    /// The name the peer gave in `HELLO`. Not verified.
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn remote_address(&self) -> SocketAddr {
        self.connection.remote_address()
    }

    /// Closes the connection, for example a duplicate of one already open.
    pub fn close(&self, reason: &str) {
        self.connection.close(0u32.into(), reason.as_bytes());
    }

    /// Current round-trip time estimate from QUIC.
    pub fn rtt(&self) -> Duration {
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

    /// Queues a control message. Fails only once the connection is gone.
    pub fn send_control(&self, message: &ControlMessage) -> Result<(), TransportError> {
        self.control_out
            .send(message.encode())
            .map_err(|_| TransportError::Closed)
    }

    /// Waits for the next control message. Call from one task at a time.
    pub async fn recv_control(&self) -> Result<ControlMessage, TransportError> {
        read_message(&mut *self.control_in.lock().await).await
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
    ControlRead(quinn::ReadExactError),
    ControlWrite(quinn::WriteError),
    Control(ControlError),
    /// The peer completed the handshake without an Ed25519 certificate.
    NoPeerIdentity,
    /// The peer's first message was not a `HELLO` matching its key.
    BadHello,
    HelloTimeout,
    Closed,
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
            Self::ControlRead(e) => write!(f, "control stream: {e}"),
            Self::ControlWrite(e) => write!(f, "control stream: {e}"),
            Self::Control(e) => write!(f, "{e}"),
            Self::NoPeerIdentity => write!(f, "peer presented no Ed25519 identity"),
            Self::BadHello => write!(f, "peer sent an invalid HELLO"),
            Self::HelloTimeout => write!(f, "peer did not say HELLO in time"),
            Self::Closed => write!(f, "connection closed"),
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
    quinn::ReadExactError => ControlRead,
    quinn::WriteError => ControlWrite,
    ControlError => Control,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pair() -> (Node, Node, Peer, Peer) {
        let localhost: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = Node::bind(localhost, &Identity::generate().unwrap(), "listener").unwrap();
        let dialer = Node::bind(localhost, &Identity::generate().unwrap(), "dialer").unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let (accepted, connected) = tokio::join!(listener.accept(), dialer.connect(listen_addr));
        let (accepted, connected) = (accepted.unwrap().unwrap(), connected.unwrap());
        (listener, dialer, accepted, connected)
    }

    #[tokio::test]
    async fn exchanges_hello_and_voice() {
        let (listener, dialer, accepted, connected) = pair().await;
        assert_eq!(accepted.fingerprint(), dialer.fingerprint());
        assert_eq!(connected.fingerprint(), listener.fingerprint());
        assert_eq!(accepted.display_name(), "dialer");
        assert_eq!(connected.display_name(), "listener");
        assert_eq!(connected.radio_id(), listener.fingerprint().radio_id());

        connected.send_voice(b"voice".to_vec()).unwrap();
        assert_eq!(&accepted.recv_voice().await.unwrap()[..], b"voice");
    }

    #[tokio::test]
    async fn connect_any_skips_unreachable_addresses() {
        let localhost: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = Node::bind(localhost, &Identity::generate().unwrap(), "listener").unwrap();
        let dialer = Node::bind(localhost, &Identity::generate().unwrap(), "dialer").unwrap();
        // TEST-NET-1 (RFC 5737) never answers.
        let dead: SocketAddr = "192.0.2.1:4747".parse().unwrap();
        let addrs = [dead, listener.local_addr().unwrap()];
        let started = std::time::Instant::now();
        let (accepted, connected) = tokio::join!(listener.accept(), dialer.connect_any(&addrs));
        accepted.unwrap().unwrap();
        assert_eq!(connected.unwrap().display_name(), "listener");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn carries_control_messages_both_ways() {
        let (_listener, _dialer, accepted, connected) = pair().await;
        let key_up = ControlMessage::KeyUp {
            group_id: 7,
            transmission_id: 1,
        };
        connected.send_control(&key_up).unwrap();
        assert_eq!(accepted.recv_control().await.unwrap(), key_up);
        let leave = ControlMessage::Leave { group_id: 7 };
        accepted.send_control(&leave).unwrap();
        assert_eq!(connected.recv_control().await.unwrap(), leave);
    }
}
