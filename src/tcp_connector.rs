use anyhow::{Context, Result};
use async_trait::async_trait;
use std::time::Duration;
use tokio::net::TcpStream;

/// Abstraction for TCP connection establishment.
/// Allows testing handshake logic without actual network I/O.
///
/// For simplicity and to avoid trait object issues, this returns TcpStream for production.
/// For testing, we'll use a different approach with test-specific constructors.
#[async_trait]
pub trait TcpStreamFactory: Send + Sync + std::fmt::Debug {
    /// Connect to a peer address and return a TcpStream.
    async fn connect(&self, addr: String) -> Result<TcpStream>;
}

#[derive(Debug)]
pub struct DefaultTcpStreamFactory;

const TCP_CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

#[async_trait]
impl TcpStreamFactory for DefaultTcpStreamFactory {
    async fn connect(&self, addr: String) -> Result<TcpStream> {
        let stream = tokio::time::timeout(TCP_CONNECTION_TIMEOUT, TcpStream::connect(&addr))
            .await
            .with_context(|| format!("connection timeout to {}", addr))?
            .with_context(|| format!("failed to connect to {}", addr))?;

        // Disable Nagle's algorithm for lower latency on small packets
        // This is a standard optimization used by uTorrent, qBittorrent, and other high-performance clients
        stream.set_nodelay(true)?;

        Ok(stream)
    }
}
