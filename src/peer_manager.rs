use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::bandwidth_limiter::BandwidthLimiter;
use crate::encoding::Decoder;
use crate::error::{AppError, Result};
use crate::peer::{Peer, PeerAddr, PeerSource};
use crate::peer_connection::PeerConnection;
use crate::peer_messages::{HaveMessage, PeerMessage};
use crate::piece_manager::{FilePieceManager, PieceManager};
use crate::tcp_connector::DefaultTcpStreamFactory;
use crate::tracker_client::{AnnounceRequest, HttpTrackerClient, TrackerClient};

use crate::dht::DhtClient;

use crate::terminal_ui::{DhtStats, ProgressDisplay, ProgressStats};
use crate::{BandwidthStats, PeerConnectionStats};
use anyhow::anyhow;
use async_trait::async_trait;
use log::{debug, info, warn};
use reqwest::Client;
use std::collections::HashSet;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio::time;

const WATCH_TRACKER_DELAY: u64 = 30;
const PROGRESS_REPORT_INTERVAL_SECS: u64 = 2;
const PEER_CONNECTOR_INTERVAL_SECS: u64 = 10;
const STALE_DOWNLOAD_CHECK_INTERVAL_SECS: u64 = 30;
const MAX_CONCURRENT_HANDSHAKES: usize = 10;
const CLIENT_PEER_ID: &[u8; 20] = b"bittorrent-rust-0001";

/// Maximum number of pieces that can be downloaded concurrently from a single peer.
pub const MAX_PIECES_PER_PEER: usize = 3;

#[derive(Debug, Clone)]
pub struct PieceDownloadRequest {
    pub piece_index: u32,
    pub piece_length: usize,
    pub expected_hash: [u8; 20],
}

#[derive(Debug, Clone)]
pub struct PeerManagerConfig {
    pub info_hash: [u8; 20],
    pub client_peer_id: [u8; 20],
    pub file_size: usize,
    pub num_pieces: usize,
    pub piece_length: usize,
}

#[derive(Debug)]
pub struct ConnectedPeer {
    peer: Peer,
    download_request_tx: mpsc::UnboundedSender<PieceDownloadRequest>,
    message_tx: mpsc::UnboundedSender<PeerMessage>,
    bitfield: Arc<RwLock<HashSet<u32>>>,
}

impl ConnectedPeer {
    pub fn new(
        peer: Peer,
        download_request_tx: mpsc::UnboundedSender<PieceDownloadRequest>,
        message_tx: mpsc::UnboundedSender<PeerMessage>,
        bitfield: Arc<RwLock<HashSet<u32>>>,
    ) -> Self {
        Self {
            peer,
            download_request_tx,
            message_tx,
            bitfield,
        }
    }

    pub async fn has_piece(&self, piece_index: u32) -> bool {
        let bf = self.bitfield.read().await;
        bf.contains(&piece_index)
    }

    pub async fn piece_count(&self) -> usize {
        let bf = self.bitfield.read().await;
        bf.len()
    }

    pub async fn bitfield_len(&self) -> usize {
        self.bitfield.read().await.len()
    }

    pub fn request_piece(&self, request: PieceDownloadRequest) -> Result<()> {
        self.download_request_tx
            .send(request)
            .map_err(|e| anyhow!("Failed to send download request: {}", e))
    }

    pub fn send_message(&self, msg: PeerMessage) -> Result<()> {
        self.message_tx
            .send(msg)
            .map_err(|e| anyhow!("Failed to send message: {}", e))
    }

    pub fn get_sender(&self) -> mpsc::UnboundedSender<PieceDownloadRequest> {
        self.download_request_tx.clone()
    }

    pub fn peer_addr(&self) -> String {
        self.peer.get_addr()
    }

    pub fn peer(&self) -> &Peer {
        &self.peer
    }
}

#[derive(Debug, Clone)]
pub struct StorePieceRequest {
    pub piece_index: u32,
    pub peer_addr: String,
    pub data: Vec<u8>,
}

#[derive(Debug)]
pub struct FailedPiece {
    pub piece_index: u32,
    pub peer_addr: String,
    pub error: AppError,
    pub push_front: bool,
}

#[derive(Debug)]
pub struct PeerDisconnected {
    pub peer: Peer,
    pub error: AppError,
}

/// Unified event type for peer manager events
#[derive(Debug)]
pub enum PeerEvent {
    StorePiece(StorePieceRequest),
    Failure(FailedPiece),
    Disconnect(PeerDisconnected),
    PeerChoked(String),
    PeerUnchoked(String),
}

/// Handle for managing PeerManager lifecycle
pub struct PeerManagerHandle {
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
}

impl PeerManagerHandle {
    pub fn new(shutdown_tx: tokio::sync::broadcast::Sender<()>) -> Self {
        Self { shutdown_tx }
    }

    /// Signal shutdown to all background tasks
    pub fn shutdown(self) {
        let _ = self.shutdown_tx.send(());
    }
}

#[async_trait]
pub trait PeerConnectionFactory: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn connect(
        &self,
        peer: Peer,
        event_tx: mpsc::UnboundedSender<PeerEvent>,
        client_peer_id: [u8; 20],
        info_hash: [u8; 20],
        num_pieces: usize,
        piece_manager: Arc<dyn PieceManager>,
        bandwidth_limiter: Option<BandwidthLimiter>,
        bandwidth_stats: Arc<BandwidthStats>,
        broadcast_rx: broadcast::Receiver<PeerMessage>,
    ) -> Result<ConnectedPeer>;
}

pub struct DefaultPeerConnectionFactory;

#[async_trait]
impl PeerConnectionFactory for DefaultPeerConnectionFactory {
    async fn connect(
        &self,
        peer: Peer,
        event_tx: mpsc::UnboundedSender<PeerEvent>,
        client_peer_id: [u8; 20],
        info_hash: [u8; 20],
        num_pieces: usize,
        piece_manager: Arc<dyn PieceManager>,
        bandwidth_limiter: Option<BandwidthLimiter>,
        bandwidth_stats: Arc<BandwidthStats>,
        broadcast_rx: broadcast::Receiver<PeerMessage>,
    ) -> Result<ConnectedPeer> {
        let tcp_connector = Arc::new(DefaultTcpStreamFactory);
        let (mut peer_conn, download_request_tx, bitfield, message_tx) = PeerConnection::new(
            peer.clone(),
            event_tx,
            tcp_connector,
            piece_manager,
            bandwidth_limiter,
            bandwidth_stats,
            broadcast_rx,
        );

        peer_conn.handshake(client_peer_id, info_hash).await?;
        peer_conn.start(num_pieces).await?;

        Ok(ConnectedPeer::new(
            peer.clone(),
            download_request_tx,
            message_tx,
            bitfield,
        ))
    }
}

pub struct PeerManager {
    tracker_client: Option<Arc<dyn TrackerClient>>,
    dht_client: Option<Arc<dyn DhtClient>>,

    // Manages the Peers based on the peer addr
    available_peers: Arc<RwLock<HashMap<PeerAddr, Peer>>>,
    connected_peers: Arc<RwLock<HashMap<PeerAddr, ConnectedPeer>>>,

    // Controls the state of the Pieces being downloaded AND file I/O
    piece_manager: Arc<dyn PieceManager>,

    config: Option<PeerManagerConfig>,
    connector: Arc<dyn PeerConnectionFactory>,

    bandwidth_limiter: Option<BandwidthLimiter>,
    bandwidth_stats: Arc<BandwidthStats>,
    connection_stats: Arc<PeerConnectionStats>,
    max_peers: usize,
    max_pieces_per_peer: usize,

    // Limits concurrent handshake attempts to prevent network overload
    handshake_semaphore: Arc<tokio::sync::Semaphore>,

    // Broadcast channel for efficiently sending messages to all peers
    message_broadcast_tx: broadcast::Sender<PeerMessage>,
}

impl PeerManager {
    pub fn new(
        decoder: Decoder,
        http_client: Client,
        bandwidth_limiter: Option<BandwidthLimiter>,
        bandwidth_stats: Arc<BandwidthStats>,
        connection_stats: Arc<PeerConnectionStats>,
        max_peers: usize,
    ) -> Self {
        let tracker_client = Arc::new(HttpTrackerClient::new(http_client, decoder));
        Self::new_with_connector(
            tracker_client,
            Arc::new(DefaultPeerConnectionFactory),
            bandwidth_limiter,
            bandwidth_stats,
            connection_stats,
            max_peers,
        )
    }

    pub fn new_with_connector(
        tracker_client: Arc<dyn TrackerClient>,
        connector: Arc<dyn PeerConnectionFactory>,
        bandwidth_limiter: Option<BandwidthLimiter>,
        bandwidth_stats: Arc<BandwidthStats>,
        connection_stats: Arc<PeerConnectionStats>,
        max_peers: usize,
    ) -> Self {
        Self::new_with_dht(
            Some(tracker_client),
            None,
            connector,
            bandwidth_limiter,
            bandwidth_stats,
            connection_stats,
            max_peers,
        )
    }

    pub fn new_with_dht(
        tracker_client: Option<Arc<dyn TrackerClient>>,
        dht_client: Option<Arc<dyn crate::dht::DhtClient>>,
        connector: Arc<dyn PeerConnectionFactory>,
        bandwidth_limiter: Option<BandwidthLimiter>,
        bandwidth_stats: Arc<BandwidthStats>,
        connection_stats: Arc<PeerConnectionStats>,
        max_peers: usize,
    ) -> Self {
        let handshake_semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_HANDSHAKES));

        // Create broadcast channel for messages with capacity of 100
        let (message_broadcast_tx, _) = broadcast::channel(100);

        Self {
            tracker_client,
            dht_client,
            available_peers: Arc::new(RwLock::new(HashMap::new())),
            connected_peers: Arc::new(RwLock::new(HashMap::new())),
            config: None,
            piece_manager: Arc::new(FilePieceManager::new()),
            connector,
            bandwidth_limiter,
            bandwidth_stats,
            connection_stats,
            max_peers,
            max_pieces_per_peer: MAX_PIECES_PER_PEER,
            handshake_semaphore,
            message_broadcast_tx,
        }
    }

    pub async fn initialize(
        &mut self,
        config: PeerManagerConfig,
        file_path: std::path::PathBuf,
        file_size: u64,
        piece_length: usize,
    ) -> Result<()> {
        let num_pieces = config.num_pieces;
        self.piece_manager
            .initialize(num_pieces, file_path, file_size, piece_length)
            .await?;
        self.config = Some(config);
        Ok(())
    }

    fn config(&self) -> Result<&PeerManagerConfig> {
        self.config.as_ref().ok_or_else(|| {
            anyhow::Error::from(AppError::missing_field("PeerManager not initialized"))
        })
    }

    /// Check if a peer is connected by address
    pub async fn is_peer_connected(&self, peer_addr: &str) -> bool {
        let connected_peers = self.connected_peers.read().await;
        connected_peers.contains_key(peer_addr)
    }

    /// Get count of connected peers
    pub async fn connected_peer_count(&self) -> usize {
        let connected_peers = self.connected_peers.read().await;
        connected_peers.len()
    }

    /// Get count of available peers
    pub async fn available_peer_count(&self) -> usize {
        let available_peers = self.available_peers.read().await;
        available_peers.len()
    }

    /// Get a list of all connected peer addresses
    pub async fn get_connected_peer_addrs(&self) -> Vec<String> {
        let connected_peers = self.connected_peers.read().await;
        connected_peers.keys().cloned().collect()
    }

    /// Get the Nth connected peer's socket address
    pub async fn get_nth_peer_addr(&self, index: usize) -> Option<std::net::SocketAddr> {
        let connected_peers = self.connected_peers.read().await;
        connected_peers
            .keys()
            .nth(index)
            .and_then(|addr_str| addr_str.parse().ok())
    }

    /// Update file information after fetching metadata (for magnet links)
    pub async fn update_file_info(
        &self,
        file_path: std::path::PathBuf,
        file_size: u64,
        piece_length: usize,
        num_pieces: usize,
    ) -> Result<()> {
        self.piece_manager
            .initialize(num_pieces, file_path, file_size, piece_length)
            .await
    }

    /// Announce to tracker without file size requirement (for metadata fetching)
    async fn announce_to_tracker(
        &self,
        tracker_url: &str,
        info_hash: [u8; 20],
    ) -> Result<Vec<Peer>> {
        let Some(tracker_client) = &self.tracker_client else {
            anyhow::bail!("Tracker client is disabled");
        };

        let request = AnnounceRequest {
            endpoint: tracker_url.to_string(),
            info_hash: info_hash.to_vec(),
            peer_id: String::from_utf8_lossy(CLIENT_PEER_ID).to_string(),
            port: 6881,
            uploaded: 0,
            downloaded: 0,
            left: 0,
        };

        let response = tracker_client.announce(request).await?;
        Ok(response.peers)
    }

    /// Discover peers from tracker and/or DHT without starting the PeerManager
    pub async fn discover_peers(
        &self,
        info_hash: [u8; 20],
        tracker_url: Option<String>,
    ) -> Result<Vec<std::net::SocketAddr>> {
        use std::net::SocketAddr;

        let mut peer_addrs = Vec::new();

        // Try tracker first if available
        if let Some(url) = tracker_url {
            match self.announce_to_tracker(&url, info_hash).await {
                Ok(peers) => {
                    info!("Tracker returned {} peers", peers.len());
                    peer_addrs.extend(peers.into_iter().map(|p| SocketAddr::new(p.ip, p.port)));
                }
                Err(e) => {
                    warn!("Tracker announce failed: {}", e);
                }
            }
        }

        // Try DHT if available and we don't have enough peers
        if peer_addrs.len() < 10
            && let Some(dht) = &self.dht_client
        {
            match dht.get_peers(info_hash).await {
                Ok(dht_peers) => {
                    info!("DHT returned {} peers", dht_peers.len());
                    peer_addrs.extend(dht_peers.into_iter().map(|p| SocketAddr::new(p.ip, p.port)));
                }
                Err(e) => {
                    warn!("DHT peer discovery failed: {}", e);
                }
            }
        }

        Ok(peer_addrs)
    }

    /// Get a copy of the available peers HashMap
    pub async fn get_available_peers_snapshot(&self) -> HashMap<String, Peer> {
        let available_peers = self.available_peers.read().await;
        available_peers.clone()
    }

    /// Add peers to the available peers pool
    ///
    /// This is useful for adding peers from sources other than the tracker,
    /// such as DHT, PEX (peer exchange), or manual configuration.
    pub async fn add_available_peers(&self, peers: Vec<Peer>) {
        let mut available_peers = self.available_peers.write().await;
        for peer in peers {
            available_peers.insert(peer.get_addr(), peer);
        }
    }

    /// Get the number of pieces a connected peer has
    ///
    /// Returns None if the peer is not connected
    pub async fn get_peer_piece_count(&self, peer_addr: &str) -> Option<usize> {
        let connected_peers = self.connected_peers.read().await;
        let peer = connected_peers.get(peer_addr)?;
        Some(peer.piece_count().await)
    }

    /// Get peer address for a piece (if in-flight).
    /// Used for debug logging after piece completion/failure.
    pub async fn cleanup_piece_tracking(&self, piece_index: u32) -> Option<String> {
        self.piece_manager.get_peer_for_piece(piece_index).await
    }

    pub async fn get_piece_snapshot(&self) -> crate::piece_manager::PieceStats {
        self.piece_manager.get_snapshot().await
    }

    pub async fn is_piece_completed(&self, piece_index: u32) -> bool {
        self.piece_manager.is_completed(piece_index).await
    }

    pub async fn is_piece_pending(&self, piece_index: u32) -> bool {
        self.piece_manager.is_pending(piece_index).await
    }

    pub async fn is_piece_in_flight(&self, piece_index: u32) -> bool {
        self.piece_manager.is_in_flight(piece_index).await
    }

    pub async fn start(self: Arc<Self>, announce_url: Option<String>) -> Result<PeerManagerHandle> {
        let config = self.config()?;

        let info_hash = config.info_hash;
        let file_size = config.file_size;
        let num_pieces = config.num_pieces;

        // Create shutdown channel for graceful termination in tests
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);

        // Create unified event channel for all peer events
        let (event_tx, event_rx) = mpsc::unbounded_channel::<PeerEvent>();

        // Spawn unified event handler task
        tokio::task::spawn(self.clone().handle_peer_events(event_rx, num_pieces));

        // Spawn task to connect with peers
        let max_peers = self.max_peers;
        tokio::task::spawn(self.clone().handle_peer_connector(
            max_peers,
            event_tx.clone(),
            shutdown_rx.resubscribe(),
        ));

        // Spawn task to assign pieces to peers
        tokio::task::spawn(
            self.clone()
                .handle_piece_assignment(shutdown_rx.resubscribe()),
        );

        // Spawn task to check for stale downloads (timeouts)
        tokio::task::spawn(
            self.clone()
                .handle_stale_download_checker(event_tx.clone(), shutdown_rx.resubscribe()),
        );

        // Spawn task to show the current status of the pieces
        let peer_manager_progress = self.clone();
        let bandwidth_stats = self.bandwidth_stats.clone();
        let connection_stats = self.connection_stats.clone();
        let available_peers = self.available_peers.clone();
        let dht_client = self.dht_client.clone();
        tokio::task::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(PROGRESS_REPORT_INTERVAL_SECS));
            let mut display = ProgressDisplay::new();

            loop {
                interval.tick().await;

                let snapshot = peer_manager_progress.get_piece_snapshot().await;
                let (completed, pending, in_flight) = (
                    snapshot.completed_count,
                    snapshot.pending_count,
                    snapshot.in_flight_count,
                );

                let (total_peers, choking_peers) = connection_stats.get_stats();

                let (tracker_count, dht_count) = {
                    let peers = available_peers.read().await;
                    let tracker = peers
                        .values()
                        .filter(|p| p.source == PeerSource::Tracker)
                        .count();
                    let dht = peers
                        .values()
                        .filter(|p| p.source == PeerSource::Dht)
                        .count();
                    (tracker, dht)
                };
                let available_peer_count = tracker_count + dht_count;

                let download_rate = bandwidth_stats.get_download_rate();
                let upload_rate = bandwidth_stats.get_upload_rate();

                let dht_stats = if let Some(ref dht_client) = dht_client {
                    if let Ok(stats) = dht_client.get_stats().await {
                        Some(DhtStats {
                            total_nodes: stats.total_nodes,
                            buckets_used: stats.buckets_used,
                            buckets_full: stats.buckets_full,
                            ipv4_nodes: stats.ipv4_nodes,
                            ipv6_nodes: stats.ipv6_nodes,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };

                let stats = ProgressStats {
                    total_peers,
                    choking_peers,
                    available_peer_count,
                    tracker_count,
                    dht_count,
                    completed,
                    num_pieces,
                    pending,
                    in_flight,
                    download_rate,
                    upload_rate,
                    dht_stats,
                };

                display.print(&stats);
            }
        });

        if self.tracker_client.is_some() {
            if let Some(url) = announce_url {
                self.clone()
                    .watch_tracker(
                        Duration::from_secs(WATCH_TRACKER_DELAY),
                        url,
                        info_hash,
                        file_size,
                    )
                    .await?;
            } else {
                warn!("Tracker client available but no announce URL provided");
            }
        }

        if self.dht_client.is_some() {
            tokio::task::spawn(self.clone().watch_dht(info_hash, shutdown_rx.resubscribe()));
        }

        Ok(PeerManagerHandle::new(shutdown_tx))
    }

    /// Process a single completed piece event.
    /// Writes piece data to disk and updates state atomically.
    pub async fn process_completion(
        &self,
        piece_index: u32,
        piece_data: Vec<u8>,
        peer_addr: String,
    ) -> Result<()> {
        // Write to disk AND update state atomically
        self.piece_manager
            .complete_piece(piece_index, piece_data)
            .await?;

        // Broadcast Have to all connected peers
        self.broadcast_have(piece_index).await;

        if let Some(peer) = self.cleanup_piece_tracking(piece_index).await {
            let active_count = self.piece_manager.get_peer_piece_count(&peer).await;

            debug!(
                "Peer {} completed piece {}, active_downloads now: {}",
                peer_addr, piece_index, active_count
            );
        }

        Ok(())
    }

    /// Process a single failed piece event.
    pub async fn process_failure(&self, failed: FailedPiece) -> Result<()> {
        let piece_index = failed.piece_index;

        if self.piece_manager.is_completed(piece_index).await {
            debug!(
                "Ignoring failure for piece {} - already completed (race condition)",
                piece_index
            );
            return Ok(());
        }

        if failed.push_front {
            debug!(
                "Piece {} queue full on peer {}, retrying immediately with different peer",
                piece_index, failed.peer_addr
            );

            self.cleanup_piece_tracking(piece_index).await;

            self.piece_manager.retry_piece(piece_index, true).await;
            return Ok(());
        }

        if let Some(peer_addr) = self.cleanup_piece_tracking(piece_index).await {
            let active_count = self.piece_manager.get_peer_piece_count(&peer_addr).await;

            debug!(
                "Peer {} failed piece {} ({}), active_downloads now: {}",
                peer_addr, piece_index, failed.error, active_count
            );
        }

        let retry_count = self
            .piece_manager
            .retry_piece(piece_index, false)
            .await
            .unwrap_or(0);

        debug!(
            "Piece {} failed (attempt {}): {}, re-queuing",
            piece_index, retry_count, failed.error
        );

        Ok(())
    }

    /// Process a single peer disconnect event.
    pub async fn process_disconnect(&self, disconnect: PeerDisconnected) -> Result<()> {
        let peer_addr = disconnect.peer.get_addr();
        debug!("Peer {} disconnected: {}", peer_addr, disconnect.error);

        let pieces_in_flight = self.piece_manager.get_peer_pieces(&peer_addr).await;

        {
            let mut connected_peers = self.connected_peers.write().await;
            if connected_peers.remove(&peer_addr).is_some() {
                self.connection_stats.decrement_peers();
                self.connection_stats.remove_peer(&peer_addr);
            }
        }

        if !pieces_in_flight.is_empty() {
            debug!(
                "Marking {} pieces as failed due to peer disconnect: {}",
                pieces_in_flight.len(),
                peer_addr
            );

            for piece_idx in pieces_in_flight {
                self.process_failure(FailedPiece {
                    piece_index: piece_idx,
                    peer_addr: peer_addr.clone(),
                    error: disconnect.error.clone(),
                    push_front: false,
                })
                .await?;
            }
        }

        Ok(())
    }

    /// Broadcast Have message to all connected peers using broadcast channel
    async fn broadcast_have(&self, piece_index: u32) {
        let have_msg = PeerMessage::Have(HaveMessage { piece_index });

        // Send to broadcast channel - all subscribed peers will receive it
        // Ignore error if there are no receivers (no peers connected yet)
        let _ = self.message_broadcast_tx.send(have_msg);
    }

    /// Unified event handler for all peer events - merges completion, failure, and disconnect handlers
    async fn handle_peer_events(
        self: Arc<Self>,
        mut event_rx: mpsc::UnboundedReceiver<PeerEvent>,
        _num_pieces: usize,
    ) {
        while let Some(event) = event_rx.recv().await {
            match event {
                PeerEvent::StorePiece(completed) => {
                    let _ = self
                        .process_completion(
                            completed.piece_index,
                            completed.data,
                            completed.peer_addr,
                        )
                        .await;
                }
                PeerEvent::Failure(failed) => {
                    let _ = self.process_failure(failed).await;
                }
                PeerEvent::Disconnect(disconnect) => {
                    let _ = self.process_disconnect(disconnect).await;
                }
                PeerEvent::PeerChoked(addr) => {
                    self.connection_stats.set_choking(&addr);
                }
                PeerEvent::PeerUnchoked(addr) => {
                    self.connection_stats.set_unchoked(&addr);
                }
            }
        }
    }

    /// Periodically connect to peers - extracted for testing
    async fn handle_peer_connector(
        self: Arc<Self>,
        max_peers: usize,
        event_tx: mpsc::UnboundedSender<PeerEvent>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        let mut interval = tokio::time::interval(Duration::from_secs(PEER_CONNECTOR_INTERVAL_SECS));
        interval.tick().await;

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Peer connector task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    let _ = self
                        .connect_with_peers(max_peers, event_tx.clone())
                        .await;
                }
            }
        }
    }

    /// Try to assign one piece from the queue to an available peer.
    ///
    /// Returns Ok(true) if a piece was assigned, Ok(false) if queue is empty.
    pub async fn try_assign_piece(&self) -> Result<bool> {
        let piece_index = self.piece_manager.pop_pending().await;

        if let Some(index) = piece_index {
            let request = self.piece_manager.get_request(index).await;

            if let Some(req) = request {
                if let Err(e) = self.assign_piece_to_peer(req.clone()).await {
                    self.piece_manager.requeue_piece(index).await;
                    return Err(e);
                }
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    /// Assign pending pieces to available peers - extracted for testing
    async fn handle_piece_assignment(self: Arc<Self>, mut shutdown_rx: broadcast::Receiver<()>) {
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Piece assignment task shutting down");
                    break;
                }
                _ = async {
                    match self.try_assign_piece().await {
                        Ok(true) => {
                            // Successfully assigned, continue immediately
                        }
                        Ok(false) | Err(_) => {
                            // Queue empty or assignment failed, wait before retry
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }
                } => {}
            }
        }
    }

    /// Background task to check for stale downloads that have exceeded the timeout
    async fn handle_stale_download_checker(
        self: Arc<Self>,
        event_tx: mpsc::UnboundedSender<PeerEvent>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        let mut interval =
            tokio::time::interval(Duration::from_secs(STALE_DOWNLOAD_CHECK_INTERVAL_SECS));
        interval.tick().await; // Skip first immediate tick

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Stale download checker task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    let stale_downloads = self.piece_manager.get_stale_downloads().await;

                    if !stale_downloads.is_empty() {
                        debug!("Found {} stale downloads exceeding timeout", stale_downloads.len());

                        for (piece_index, peer_addr) in stale_downloads {
                            debug!(
                                "Piece {} download timed out on peer {}, retrying",
                                piece_index, peer_addr
                            );

                            let _ = event_tx.send(PeerEvent::Failure(FailedPiece {
                                piece_index,
                                peer_addr,
                                error: AppError::DownloadTimeout,
                                push_front: false,
                            }));
                        }
                    }
                }
            }
        }
    }

    /// Creates the connection with a peer over the network.
    pub async fn connect_peer(
        &self,
        peer: Peer,
        event_tx: mpsc::UnboundedSender<PeerEvent>,
    ) -> Result<ConnectedPeer> {
        let config = self.config()?;

        self.connector
            .connect(
                peer,
                event_tx,
                config.client_peer_id,
                config.info_hash,
                config.num_pieces,
                self.piece_manager.clone(),
                self.bandwidth_limiter.clone(),
                self.bandwidth_stats.clone(),
                self.message_broadcast_tx.subscribe(),
            )
            .await
    }

    pub async fn request_pieces(&self, requests: Vec<PieceDownloadRequest>) -> Result<()> {
        for request in requests {
            self.download_piece(request).await?;
        }
        Ok(())
    }

    /// Assign a piece download to the best available peer.
    ///
    /// Selects a peer that has the piece and is least busy (fewest active downloads),
    /// then sends the download request to that peer's channel.
    pub async fn assign_piece_to_peer(&self, request: PieceDownloadRequest) -> Result<()> {
        let piece_index = request.piece_index;

        // Phase 1: Collect candidates and debug info with READ lock
        // Strategy: Collect all data in one pass to minimize lock hold time
        let (best_peer_addr, debug_info) = {
            let connected_peers = self.connected_peers.read().await;

            // Collect debug info for all peers (data only, no formatting under lock)
            let mut peer_debug_data = Vec::new();
            let mut candidates: Vec<(String, usize)> = Vec::new();

            for (addr, peer) in connected_peers.iter() {
                let peer_addr = addr.clone();
                let has_piece = peer.has_piece(piece_index).await;
                let is_busy = self
                    .piece_manager
                    .is_peer_downloading_piece(&peer_addr, piece_index)
                    .await;
                let active_count = self.piece_manager.get_peer_piece_count(&peer_addr).await;
                let num_pieces = peer.piece_count().await;
                let bitfield_len = peer.bitfield_len().await;

                peer_debug_data.push((
                    addr.clone(),
                    has_piece,
                    is_busy,
                    active_count,
                    num_pieces,
                    bitfield_len,
                ));

                // Only consider peers that:
                // 1. Have the piece
                // 2. Are not already downloading this specific piece
                // 3. Are not at max_pieces_per_peer capacity
                if has_piece && !is_busy && active_count < self.max_pieces_per_peer {
                    candidates.push((addr.clone(), active_count));
                }
            }

            let best_addr = candidates
                .iter()
                .min_by_key(|(_, active_count)| active_count)
                .map(|(addr, _)| addr.clone())
                .ok_or(anyhow::Error::from(AppError::NoPeersAvailable))?;

            (best_addr, peer_debug_data)
        }; // Read lock released here

        for (addr, has_piece, is_busy, active_count, num_pieces, bitfield_len) in debug_info {
            debug!(
                "Peer {}, piece={}, has_piece={}, already_downloading={}, active_downloads={}, total_pieces={}/{}",
                addr, piece_index, has_piece, is_busy, active_count, num_pieces, bitfield_len
            );
        }

        debug!(
            "Peer {} selected to download the piece {}",
            best_peer_addr, piece_index
        );

        // Phase 2: Get sender channel with brief read lock
        let sender = {
            let connected_peers = self.connected_peers.read().await;
            connected_peers
                .get(&best_peer_addr)
                .ok_or(anyhow::Error::from(AppError::NoPeersAvailable))?
                .get_sender()
        }; // Read lock released here

        // Phase 3: Send request WITHOUT holding any locks
        sender.send(request.clone())?;

        // Phase 4: Transition piece to InFlight state
        self.piece_manager
            .start_download(piece_index, best_peer_addr.clone())
            .await
            .ok_or(anyhow::Error::msg(
                "Failed to start download - piece not in Pending state",
            ))?;

        Ok(())
    }

    /// Watch the tracker server provided in the torrent file
    /// and update the available peers for the peer manager to be able to manager them.
    ///
    /// Won't block, just spawn a task to run.
    pub async fn watch_tracker(
        self: Arc<Self>,
        interval: Duration,
        endpoint: String,
        info_hash: [u8; 20],
        file_size: usize,
    ) -> Result<()> {
        let available_peers = self.available_peers.clone();
        let peer_manager = self.clone();

        tokio::task::spawn(async move {
            // First tracker request happens immediately
            match peer_manager
                .get_peers(endpoint.clone(), info_hash, file_size)
                .await
            {
                Ok(peers) => {
                    let mut hash_map = available_peers.write().await;
                    for peer in &peers {
                        hash_map.insert(peer.get_addr(), peer.clone());
                    }
                    debug!(
                        "Populated available_peers with {} peers from initial tracker request",
                        peers.len()
                    );
                    drop(hash_map);

                    peer_manager
                        .drop_useless_peers_if_at_capacity(peer_manager.max_peers)
                        .await;
                }
                Err(e) => {
                    debug!("Initial tracker request failed: {}", e);
                }
            }

            let mut interval = time::interval(interval);
            interval.tick().await; // Skip first immediate tick

            loop {
                interval.tick().await;
                match peer_manager
                    .get_peers(endpoint.clone(), info_hash, file_size)
                    .await
                {
                    Ok(peers) => {
                        let mut hash_map = available_peers.write().await;
                        let previous_count = hash_map.len();
                        for peer in &peers {
                            hash_map.insert(peer.get_addr(), peer.clone());
                        }
                        debug!(
                            "Updated available_peers: {} total (added {} new peers)",
                            hash_map.len(),
                            hash_map.len().saturating_sub(previous_count)
                        );
                        drop(hash_map);

                        peer_manager
                            .drop_useless_peers_if_at_capacity(peer_manager.max_peers)
                            .await;
                    }
                    Err(e) => {
                        debug!("Tracker request failed: {}", e);
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn watch_dht(
        self: Arc<Self>,
        info_hash: [u8; 20],
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        let Some(dht_client) = self.dht_client.clone() else {
            return;
        };

        let available_peers = self.available_peers.clone();
        let peer_manager = self.clone();

        let mut retry_count = 0;
        const MAX_IMMEDIATE_RETRIES: u32 = 3;
        const RETRY_DELAY_SECS: u64 = 30;

        loop {
            match dht_client.get_peers(info_hash).await {
                Ok(peers) => {
                    let mut hash_map = available_peers.write().await;
                    for peer in &peers {
                        hash_map.insert(peer.get_addr(), peer.clone());
                    }

                    if retry_count == 0 {
                        info!("DHT found {} initial peers", peers.len());
                    } else {
                        info!("DHT found {} peers on retry {}", peers.len(), retry_count);
                    }
                    drop(hash_map);

                    peer_manager
                        .drop_useless_peers_if_at_capacity(peer_manager.max_peers)
                        .await;

                    if !peers.is_empty() || retry_count >= MAX_IMMEDIATE_RETRIES {
                        break;
                    }

                    retry_count += 1;
                    warn!(
                        "DHT found 0 peers, retrying in {} seconds (attempt {}/{})",
                        RETRY_DELAY_SECS, retry_count, MAX_IMMEDIATE_RETRIES
                    );

                    tokio::select! {
                        _ = shutdown_rx.recv() => {
                            debug!("DHT watch task shutting down during retry");
                            return;
                        }
                        _ = time::sleep(Duration::from_secs(RETRY_DELAY_SECS)) => {}
                    }
                }
                Err(e) => {
                    if retry_count == 0 {
                        warn!("Initial DHT request failed: {}", e);
                    } else {
                        warn!("DHT request failed on retry {}: {}", retry_count, e);
                    }

                    if retry_count >= MAX_IMMEDIATE_RETRIES {
                        break;
                    }

                    retry_count += 1;
                    tokio::select! {
                        _ = shutdown_rx.recv() => {
                            debug!("DHT watch task shutting down during error retry");
                            return;
                        }
                        _ = time::sleep(Duration::from_secs(RETRY_DELAY_SECS)) => {}
                    }
                }
            }
        }

        let mut interval = time::interval(Duration::from_secs(2 * 60));
        interval.tick().await;

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("DHT watch task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    match dht_client.get_peers(info_hash).await {
                        Ok(peers) => {
                            let mut hash_map = available_peers.write().await;
                            let previous_count = hash_map.len();
                            for peer in &peers {
                                hash_map.insert(peer.get_addr(), peer.clone());
                            }
                            debug!(
                                "DHT updated available_peers: {} total (added {} new peers)",
                                hash_map.len(),
                                hash_map.len().saturating_sub(previous_count)
                            );
                            drop(hash_map);

                            peer_manager
                                .drop_useless_peers_if_at_capacity(peer_manager.max_peers)
                                .await;
                        }
                        Err(e) => {
                            debug!("DHT request failed: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Fetches peers from the tracker server to connect with.
    pub async fn get_peers(
        &self,
        endpoint: String,
        info_hash: [u8; 20],
        file_size: usize,
    ) -> Result<Vec<Peer>> {
        let max_duration = Duration::from_secs(5 * 60);
        let max_backoff = Duration::from_secs(60);
        let start_time = std::time::Instant::now();
        let mut attempt = 0;
        let mut last_error: Option<anyhow::Error> = None;

        loop {
            attempt += 1;
            let elapsed = start_time.elapsed();

            if elapsed >= max_duration {
                let err_msg = last_error
                    .as_ref()
                    .map(|e: &anyhow::Error| e.to_string())
                    .unwrap_or_else(|| "Unknown error".to_string());
                return Err(AppError::tracker_rejected(format!(
                    "Failed to get peers after {} attempts over {} seconds: {}",
                    attempt - 1,
                    elapsed.as_secs(),
                    err_msg
                ))
                .into());
            }

            match self.try_get_peers(&endpoint, info_hash, file_size).await {
                Ok(peers) => {
                    if attempt > 1 {
                        debug!("Tracker connected after {} attempts", attempt);
                    }
                    debug!("Tracker request completed: {} peers available", peers.len());
                    return Ok(peers);
                }
                Err(e) => {
                    let is_tracker_error = if let Some(app_err) = e.downcast_ref::<AppError>() {
                        matches!(app_err, AppError::TrackerRejected(_))
                    } else {
                        false
                    };

                    if is_tracker_error {
                        debug!("Tracker rejected connection: {}", e);
                        return Err(e);
                    }

                    last_error = Some(e);

                    // Exponential backoff: 10x multiplier with cap at 60 seconds
                    // Attempt 1: 100ms * 10^0 = 100ms
                    // Attempt 2: 100ms * 10^1 = 1s
                    // Attempt 3: 100ms * 10^2 = 10s
                    // Attempt 4: 100ms * 10^3 = 100s -> capped to 60s
                    // Attempt 5+: continues at 60s intervals
                    let base_delay = Duration::from_millis(100);
                    let exponential_delay = base_delay * 10_u32.pow((attempt - 1).min(6));
                    let delay = exponential_delay.min(max_backoff);

                    let remaining_time = max_duration.saturating_sub(elapsed);
                    if delay >= remaining_time {
                        let err_msg = last_error
                            .as_ref()
                            .map(|e: &anyhow::Error| e.to_string())
                            .unwrap_or_else(|| "Unknown error".to_string());
                        return Err(AppError::tracker_rejected(format!(
                            "Failed to get peers after {} attempts over {} seconds: {}",
                            attempt,
                            elapsed.as_secs(),
                            err_msg
                        ))
                        .into());
                    }

                    time::sleep(delay).await;
                }
            }
        }
    }

    async fn try_get_peers(
        &self,
        endpoint: &str,
        info_hash: [u8; 20],
        file_size: usize,
    ) -> Result<Vec<Peer>> {
        let Some(tracker_client) = &self.tracker_client else {
            anyhow::bail!("Tracker client is disabled");
        };

        let request = AnnounceRequest {
            endpoint: endpoint.to_string(),
            info_hash: info_hash.to_vec(),
            peer_id: String::from_utf8_lossy(CLIENT_PEER_ID).to_string(),
            port: 6881,
            uploaded: 0,
            downloaded: 0,
            left: file_size,
        };

        let response = tracker_client.announce(request).await?;
        Ok(response.peers)
    }

    /// Drop peers with empty bitfields or only completed pieces when at max capacity.
    ///
    /// This is called after tracker announces to make room for potentially
    /// better peers when we're at maximum connections. Peers are considered useless if:
    /// 1. They have an empty bitfield (no pieces at all), OR
    /// 2. All pieces they have are already completed (we don't need them anymore)
    pub async fn drop_useless_peers_if_at_capacity(&self, max_peers: usize) {
        let peers_to_drop = {
            let connected = self.connected_peers.read().await;

            if connected.len() < max_peers {
                return;
            }

            let mut useless_peers = Vec::new();
            for (addr, peer) in connected.iter() {
                let bitfield_len = peer.bitfield_len().await;
                if bitfield_len == 0 {
                    useless_peers.push(addr.clone());
                    continue;
                }

                let mut has_needed_piece = false;
                for piece_idx in 0..bitfield_len {
                    if peer.has_piece(piece_idx as u32).await
                        && !self.piece_manager.is_completed(piece_idx as u32).await
                    {
                        has_needed_piece = true;
                        break;
                    }
                }

                if !has_needed_piece {
                    useless_peers.push(addr.clone());
                }
            }

            useless_peers
        };

        if peers_to_drop.is_empty() {
            let choking_peers = {
                let connected = self.connected_peers.read().await;
                let (_, choking_count) = self.connection_stats.get_stats();

                if choking_count == 0 {
                    return;
                }

                connected
                    .keys()
                    .take(choking_count)
                    .cloned()
                    .collect::<Vec<_>>()
            };

            if choking_peers.is_empty() {
                return;
            }

            debug!(
                "At max capacity ({} peers), strategies 1-2 found no peers to drop. Dropping {} choking peers",
                max_peers,
                choking_peers.len()
            );

            for addr in choking_peers {
                let mut connected = self.connected_peers.write().await;
                if let Some(peer) = connected.remove(&addr) {
                    self.connection_stats.decrement_peers();
                    self.connection_stats.remove_peer(&addr);
                    info!("Dropped peer {} (choking us)", addr);
                    drop(peer);
                    drop(connected);
                }
            }

            return;
        }

        debug!(
            "At max capacity ({} peers), dropping {} useless peers",
            max_peers,
            peers_to_drop.len()
        );

        for addr in peers_to_drop {
            let mut connected = self.connected_peers.write().await;
            if let Some(peer) = connected.remove(&addr) {
                self.connection_stats.decrement_peers();
                self.connection_stats.remove_peer(&addr);
                info!("Dropped peer {} (no needed pieces available)", addr);
                drop(peer);
                drop(connected);
            }
        }
    }

    /// Connect to available peers up to the specified limit.
    ///
    /// This method attempts to establish connections with peers from the available peer pool
    /// until reaching the maximum number of concurrent peers. It's called automatically by
    /// the background orchestration task in `start()`, but can also be invoked manually
    /// when immediate peer connections are needed.
    pub async fn connect_with_peers(
        &self,
        max_peers: usize,
        event_tx: mpsc::UnboundedSender<PeerEvent>,
    ) -> Result<usize> {
        let (peers_to_connect, available_count, connected_count, needed) = {
            let available = self.available_peers.read().await;
            let connected = self.connected_peers.read().await;
            let needed = max_peers.saturating_sub(connected.len());

            let mut peers = available
                .values()
                .filter(|p| !connected.contains_key(&p.get_addr()))
                .cloned()
                .collect::<Vec<_>>();

            // Prioritize IPv4 peers over IPv6 peers
            peers.sort_by_key(|p| p.ip.is_ipv6());
            peers.truncate(needed);

            (peers, available.len(), connected.len(), needed)
        };

        if peers_to_connect.is_empty() {
            if needed > 0 {
                debug!(
                    "Need {} more peers but no new peers available (available={}, connected={})",
                    needed, available_count, connected_count
                );
            } else {
                debug!(
                    "At capacity ({}/{} peers connected)",
                    connected_count, max_peers
                );
            }
        } else {
            debug!(
                "Connection attempt: available_peers={}, connected_peers={}, need_peers={}, attempting={}",
                available_count,
                connected_count,
                needed,
                peers_to_connect.len()
            );
        }

        // Spawn connection attempts without waiting (non-blocking)
        // Use semaphore to limit concurrent handshakes and prevent network overload
        let num_attempts = peers_to_connect.len();
        for peer in peers_to_connect {
            let connected_peers = self.connected_peers.clone();
            let available_peers = self.available_peers.clone();
            let connector = self.connector.clone();
            let config = self.config()?.clone();
            let event_tx = event_tx.clone();
            let piece_manager = self.piece_manager.clone();
            let bandwidth_limiter = self.bandwidth_limiter.clone();
            let bandwidth_stats = self.bandwidth_stats.clone();
            let connection_stats = self.connection_stats.clone();
            let semaphore = self.handshake_semaphore.clone();
            let broadcast_rx = self.message_broadcast_tx.subscribe();

            tokio::spawn(async move {
                let peer_addr = peer.get_addr();

                // Acquire semaphore permit before attempting handshake
                let _permit = semaphore.acquire().await.unwrap();

                match connector
                    .connect(
                        peer,
                        event_tx,
                        config.client_peer_id,
                        config.info_hash,
                        config.num_pieces,
                        piece_manager,
                        bandwidth_limiter,
                        bandwidth_stats,
                        broadcast_rx,
                    )
                    .await
                {
                    Ok(connected_peer) => {
                        info!("Successfully connected to peer {}", peer_addr);

                        // Remove from available peers once connected
                        available_peers.write().await.remove(&peer_addr);

                        let mut connected = connected_peers.write().await;
                        connected.insert(peer_addr.clone(), connected_peer);
                        connection_stats.increment_peers();
                    }
                    Err(e) => {
                        debug!("Failed to connect to peer {}: {}", peer_addr, e);
                        // Remove from available peers after failed connection attempt
                        available_peers.write().await.remove(&peer_addr);
                    }
                }
                // Permit automatically released when _permit is dropped
            });
        }

        Ok(num_attempts)
    }

    /// Queue a piece download request. The background piece assignment task will assign it to a peer.
    pub async fn download_piece(&self, request: PieceDownloadRequest) -> Result<()> {
        self.piece_manager.queue_piece(request).await
    }

    pub async fn verify_completed_pieces(&self, expected_hashes: &[[u8; 20]]) -> Result<Vec<u32>> {
        self.piece_manager
            .verify_completed_pieces(expected_hashes)
            .await
    }

    /// For testing: pop a piece from the pending queue
    #[doc(hidden)]
    pub async fn pop_pending_for_test(&self) -> Option<u32> {
        self.piece_manager.pop_pending().await
    }

    /// For testing: mark a piece as being downloaded by a peer
    #[doc(hidden)]
    pub async fn start_download(&self, piece_index: u32, peer_addr: String) {
        self.piece_manager
            .start_download(piece_index, peer_addr)
            .await;
    }

    /// For testing: check if a piece is completed
    #[doc(hidden)]
    pub async fn has_piece(&self, piece_index: u32) -> bool {
        self.piece_manager.has_piece(piece_index).await
    }
}
