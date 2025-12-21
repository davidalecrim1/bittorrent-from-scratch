use anyhow::Result;
use async_trait::async_trait;
use bittorrent_from_scratch::messages::PeerMessage;
use bittorrent_from_scratch::peer_manager::PeerConnector;
use bittorrent_from_scratch::traits::{
    AnnounceRequest, AnnounceResponse, MessageIO, TrackerClient,
};
use bittorrent_from_scratch::types::{ConnectedPeer, FailedPiece, Peer};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

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

/// Mock tracker client for testing
pub struct MockTrackerClient {
    responses: Mutex<VecDeque<Result<AnnounceResponse>>>,
}

impl MockTrackerClient {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
        }
    }

    pub async fn expect_announce(&self, peers: Vec<Peer>, interval: Option<u64>) {
        self.responses
            .lock()
            .await
            .push_back(Ok(AnnounceResponse { peers, interval }));
    }

    pub async fn expect_failure(&self, error_msg: &str) {
        self.responses
            .lock()
            .await
            .push_back(Err(anyhow::anyhow!(error_msg.to_string())));
    }
}

#[async_trait]
impl TrackerClient for MockTrackerClient {
    async fn announce(&self, _request: AnnounceRequest) -> Result<AnnounceResponse> {
        self.responses.lock().await.pop_front().unwrap_or_else(|| {
            Ok(AnnounceResponse {
                peers: vec![],
                interval: Some(60),
            })
        })
    }
}

/// Mock peer connector for testing
pub struct MockPeerConnector {}

impl MockPeerConnector {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl PeerConnector for MockPeerConnector {
    async fn connect(
        &self,
        peer: Peer,
        _completion_tx: mpsc::Sender<bittorrent_from_scratch::types::CompletedPiece>,
        _failure_tx: mpsc::Sender<FailedPiece>,
        _disconnect_tx: mpsc::Sender<bittorrent_from_scratch::types::PeerDisconnected>,
        _client_peer_id: [u8; 20],
        _info_hash: [u8; 20],
        _num_pieces: usize,
    ) -> Result<ConnectedPeer> {
        let (download_request_tx, _rx) =
            mpsc::channel::<bittorrent_from_scratch::types::PieceDownloadRequest>(10);
        let bitfield = Arc::new(tokio::sync::RwLock::new(Vec::new()));

        Ok(ConnectedPeer::new(peer, download_request_tx, bitfield))
    }
}
