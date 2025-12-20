use crate::messages::PeerMessage;
use anyhow::Result;
use async_trait::async_trait;
use tokio::net::TcpStream;

// Re-export types used in trait signatures for convenience
pub use crate::types::{AnnounceRequest, AnnounceResponse};

/// Abstraction for TCP connection establishment.
/// Allows testing handshake logic without actual network I/O.
///
/// For simplicity and to avoid trait object issues, this returns TcpStream for production.
/// For testing, we'll use a different approach with test-specific constructors.
#[async_trait]
pub trait TcpConnector: Send + Sync + std::fmt::Debug {
    /// Connect to a peer address and return a TcpStream.
    async fn connect(&self, addr: String) -> Result<TcpStream>;
}

/// Abstraction for peer-to-peer message I/O after handshake.
/// Handles BitTorrent peer protocol message encoding/decoding.
#[async_trait]
pub trait MessageIO: Send + Sync + std::fmt::Debug {
    /// Write a peer protocol message to the stream.
    async fn write_message(&mut self, msg: &PeerMessage) -> Result<()>;

    /// Read the next peer protocol message from the stream.
    /// Returns None if the stream has ended gracefully.
    async fn read_message(&mut self) -> Result<Option<PeerMessage>>;
}

/// Abstraction for tracker HTTP communication.
/// Allows testing peer discovery logic without making real HTTP requests.
#[async_trait]
pub trait TrackerClient: Send + Sync {
    /// Send an announce request to the tracker.
    async fn announce(&self, request: AnnounceRequest) -> Result<AnnounceResponse>;
}
