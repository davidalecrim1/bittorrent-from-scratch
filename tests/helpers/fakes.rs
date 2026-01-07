use anyhow::Result;
use async_trait::async_trait;
use bittorrent_from_scratch::bandwidth_limiter::BandwidthLimiter;
use bittorrent_from_scratch::io::MessageIO;
use bittorrent_from_scratch::peer_manager::PeerConnectionFactory;
use bittorrent_from_scratch::peer_messages::PeerMessage;
use bittorrent_from_scratch::piece_manager::{InMemoryPieceManager, PieceManager};
use bittorrent_from_scratch::tracker_client::TrackerClient;
use bittorrent_from_scratch::types::{AnnounceRequest, AnnounceResponse, ConnectedPeer, Peer};
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

/// Fake peer connection factory for testing with configurable bitfields.
/// Follows FakeMessageIO pattern: configuration-based, not expectation-based.
pub struct FakePeerConnectionFactory {
    peer_bitfields: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Vec<bool>>>>,
    /// Receivers are stored but never read - they exist solely to keep channels alive.
    /// Without this, the channel would close immediately when connect() returns.
    _receivers: Arc<
        tokio::sync::Mutex<
            Vec<mpsc::Receiver<bittorrent_from_scratch::types::PieceDownloadRequest>>,
        >,
    >,
}

impl FakePeerConnectionFactory {
    pub fn new() -> Self {
        Self {
            peer_bitfields: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            _receivers: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    /// Configure a specific bitfield for a peer address.
    /// Must be called before connect() is invoked for that peer.
    pub async fn with_bitfield(&self, peer_addr: String, bitfield: Vec<bool>) {
        self.peer_bitfields.lock().await.insert(peer_addr, bitfield);
    }

    /// Convenience: Configure peer to have all pieces.
    pub async fn with_all_pieces(&self, peer_addr: String, num_pieces: usize) {
        let bitfield = vec![true; num_pieces];
        self.with_bitfield(peer_addr, bitfield).await;
    }

    /// Convenience: Configure peer to have no pieces (same as default).
    pub async fn with_no_pieces(&self, peer_addr: String) {
        self.with_bitfield(peer_addr, Vec::new()).await;
    }

    /// Convenience: Configure peer to have specific pieces.
    /// Example: with_specific_pieces("127.0.0.1:6881", vec![0, 2, 5], 10)
    /// Creates bitfield [true, false, true, false, false, true, false, false, false, false]
    pub async fn with_specific_pieces(
        &self,
        peer_addr: String,
        piece_indices: Vec<usize>,
        total_pieces: usize,
    ) {
        let mut bitfield = vec![false; total_pieces];
        for &idx in &piece_indices {
            if idx < total_pieces {
                bitfield[idx] = true;
            }
        }
        self.with_bitfield(peer_addr, bitfield).await;
    }
}

#[async_trait]
impl PeerConnectionFactory for FakePeerConnectionFactory {
    async fn connect(
        &self,
        peer: Peer,
        _event_tx: mpsc::UnboundedSender<bittorrent_from_scratch::types::PeerEvent>,
        _client_peer_id: [u8; 20],
        _info_hash: [u8; 20],
        _num_pieces: usize,
        _piece_manager: Arc<dyn PieceManager>,
        _bandwidth_limiter: Option<BandwidthLimiter>,
        _bandwidth_stats: Arc<bittorrent_from_scratch::BandwidthStats>,
        _broadcast_rx: tokio::sync::broadcast::Receiver<
            bittorrent_from_scratch::peer_messages::PeerMessage,
        >,
    ) -> Result<ConnectedPeer> {
        let (download_request_tx, rx) =
            mpsc::channel::<bittorrent_from_scratch::types::PieceDownloadRequest>(10);
        let (message_tx, _message_rx) = mpsc::channel::<PeerMessage>(10);

        // Store receiver to keep channel alive
        self._receivers.lock().await.push(rx);

        // Look up configured bitfield for this peer address
        let peer_addr = peer.get_addr();
        let configured_bitfield = self.peer_bitfields.lock().await.get(&peer_addr).cloned();

        // Use configured bitfield or default to empty HashSet (backward compatible)
        // Convert Vec<bool> to HashSet<u32>
        let bitfield_set: std::collections::HashSet<u32> = configured_bitfield
            .unwrap_or_else(Vec::new)
            .iter()
            .enumerate()
            .filter_map(|(idx, &has)| if has { Some(idx as u32) } else { None })
            .collect();
        let bitfield = Arc::new(tokio::sync::RwLock::new(bitfield_set));

        Ok(ConnectedPeer::new(
            peer,
            download_request_tx,
            message_tx,
            bitfield,
        ))
    }
}

/// Helper function to create a test piece manager for tests
pub fn create_test_piece_manager() -> Arc<dyn PieceManager> {
    Arc::new(InMemoryPieceManager::new())
}

/// Helper function to create test bandwidth stats
pub fn create_test_bandwidth_stats() -> Arc<bittorrent_from_scratch::BandwidthStats> {
    Arc::new(bittorrent_from_scratch::BandwidthStats::new(
        std::time::Duration::from_secs(3),
    ))
}

/// Helper function to create test connection stats
pub fn create_test_connection_stats() -> Arc<bittorrent_from_scratch::PeerConnectionStats> {
    Arc::new(bittorrent_from_scratch::PeerConnectionStats::default())
}

/// Create a broadcast receiver for testing peer connections.
/// Returns a receiver that will never receive messages but won't error on recv().
/// The sender is leaked to keep the channel open indefinitely.
pub fn create_test_broadcast_receiver()
-> tokio::sync::broadcast::Receiver<bittorrent_from_scratch::peer_messages::PeerMessage> {
    let (tx, rx) = tokio::sync::broadcast::channel(100);
    std::mem::forget(tx); // Leak sender to keep channel open
    rx
}
