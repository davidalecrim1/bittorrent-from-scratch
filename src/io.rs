use crate::encoding::{PeerMessageDecoder, PeerMessageEncoder};
use crate::messages::PeerMessage;
use crate::traits::MessageIO;
use anyhow::Result;
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio_util::codec::{FramedRead, FramedWrite};

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
