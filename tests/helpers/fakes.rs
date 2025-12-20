use anyhow::Result;
use async_trait::async_trait;
use bittorrent_from_scratch::messages::PeerMessage;
use bittorrent_from_scratch::traits::MessageIO;
use tokio::sync::mpsc;

/// Fake MessageIO for testing using in-memory channels.
/// Provides real async message passing without network I/O.
#[derive(Debug)]
pub struct FakeMessageIO {
    /// Channel for receiving messages (simulates reading from network)
    read_rx: mpsc::UnboundedReceiver<PeerMessage>,
    /// Channel for sending messages (simulates writing to network)
    write_tx: mpsc::UnboundedSender<PeerMessage>,
}

impl FakeMessageIO {
    /// Create a pair of FakeMessageIO instances connected to each other.
    /// Messages written to one can be read from the other, and vice versa.
    pub fn pair() -> (Self, Self) {
        let (tx1, rx1) = mpsc::unbounded_channel();
        let (tx2, rx2) = mpsc::unbounded_channel();

        let fake1 = Self {
            read_rx: rx1,
            write_tx: tx2,
        };

        let fake2 = Self {
            read_rx: rx2,
            write_tx: tx1,
        };

        (fake1, fake2)
    }
}

#[async_trait]
impl MessageIO for FakeMessageIO {
    async fn write_message(&mut self, msg: &PeerMessage) -> Result<()> {
        self.write_tx
            .send(msg.clone())
            .map_err(|_| anyhow::anyhow!("Channel closed"))?;
        Ok(())
    }

    async fn read_message(&mut self) -> Result<Option<PeerMessage>> {
        match self.read_rx.recv().await {
            Some(msg) => Ok(Some(msg)),
            None => Ok(None), // Channel closed, stream ended
        }
    }
}
