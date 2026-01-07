use crate::bandwidth_limiter::GovernorRateLimiter;
use crate::encoding::{PeerMessageDecoder, PeerMessageEncoder};
use crate::peer_messages::PeerMessage;
use anyhow::Result;
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio_util::codec::{FramedRead, FramedWrite};

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

/// Production implementation of MessageIO using TCP streams with framing codecs
pub struct TcpMessageIO {
    reader: FramedRead<OwnedReadHalf, PeerMessageDecoder>,
    writer: FramedWrite<OwnedWriteHalf, PeerMessageEncoder>,
}

impl TcpMessageIO {
    /// Create TcpMessageIO from a TcpStream
    pub fn from_stream(stream: TcpStream, num_pieces: usize) -> Self {
        let (reader, writer) = stream.into_split();
        Self::new(reader, writer, num_pieces)
    }

    /// Create TcpMessageIO from split stream halves
    pub fn new(reader: OwnedReadHalf, writer: OwnedWriteHalf, num_pieces: usize) -> Self {
        let decoder = PeerMessageDecoder::new(num_pieces);
        let encoder = PeerMessageEncoder::new(num_pieces);

        Self {
            reader: FramedRead::new(reader, decoder),
            writer: FramedWrite::new(writer, encoder),
        }
    }
}

impl std::fmt::Debug for TcpMessageIO {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpMessageIO").finish()
    }
}

#[async_trait]
impl MessageIO for TcpMessageIO {
    async fn write_message(&mut self, msg: &PeerMessage) -> Result<()> {
        self.writer.send(msg.clone()).await?;
        Ok(())
    }

    async fn read_message(&mut self) -> Result<Option<PeerMessage>> {
        match self.reader.next().await {
            Some(Ok(msg)) => Ok(Some(msg)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}

/// Wraps any MessageIO with optional bandwidth throttling
pub struct ThrottledMessageIO {
    inner: Box<dyn MessageIO>,
    download_limiter: Option<Arc<GovernorRateLimiter>>,
    upload_limiter: Option<Arc<GovernorRateLimiter>>,
    bandwidth_stats: Option<Arc<crate::bandwidth_stats::BandwidthStats>>,
}

impl ThrottledMessageIO {
    pub fn new(
        inner: Box<dyn MessageIO>,
        download_limiter: Option<Arc<GovernorRateLimiter>>,
        upload_limiter: Option<Arc<GovernorRateLimiter>>,
        bandwidth_stats: Option<Arc<crate::bandwidth_stats::BandwidthStats>>,
    ) -> Self {
        Self {
            inner,
            download_limiter,
            upload_limiter,
            bandwidth_stats,
        }
    }

    fn calculate_message_size(msg: &PeerMessage) -> usize {
        match msg.to_bytes() {
            Ok(bytes) => bytes.len(),
            Err(_) => 5, // Fallback to minimal message size
        }
    }
}

impl std::fmt::Debug for ThrottledMessageIO {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThrottledMessageIO").finish()
    }
}

#[async_trait]
impl MessageIO for ThrottledMessageIO {
    async fn write_message(&mut self, msg: &PeerMessage) -> Result<()> {
        let size = Self::calculate_message_size(msg);

        // Record upload bytes
        if let Some(ref stats) = self.bandwidth_stats {
            stats.record_upload(size as u64);
        }

        if let Some(limiter) = &self.upload_limiter
            && let Some(non_zero_size) = NonZeroU32::new(size as u32)
        {
            limiter.until_n_ready(non_zero_size).await.ok();
        }

        self.inner.write_message(msg).await
    }

    async fn read_message(&mut self) -> Result<Option<PeerMessage>> {
        let msg = self.inner.read_message().await?;

        if let Some(message) = &msg {
            let size = Self::calculate_message_size(message);

            // Record download bytes
            if let Some(ref stats) = self.bandwidth_stats {
                stats.record_download(size as u64);
            }

            if let Some(limiter) = &self.download_limiter
                && let Some(non_zero_size) = NonZeroU32::new(size as u32)
            {
                limiter.until_n_ready(non_zero_size).await.ok();
            }
        }

        Ok(msg)
    }
}
