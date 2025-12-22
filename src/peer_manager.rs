use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use crate::encoding::Decoder;
use crate::error::{AppError, Result};
use crate::tcp_connector::RealTcpConnector;
use crate::tracker_client::HttpTrackerClient;
use crate::traits::{AnnounceRequest, TrackerClient};
use crate::types::{
    CompletedPiece, ConnectedPeer, DownloadComplete, FailedPiece, Peer, PeerConnection,
    PeerDisconnected, PeerManagerConfig, PeerManagerHandle, PieceDownloadRequest,
};
use async_trait::async_trait;
use log::{debug, error, info};
use reqwest::Client;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio::time;

const MAX_FAILED_PIECE_RETRY: usize = 10;
const CHANNEL_SIZE: usize = 100;
const WATCH_TRACKER_DELAY: u64 = 2 * 60;

#[async_trait]
pub trait PeerConnector: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn connect(
        &self,
        peer: Peer,
        completion_tx: mpsc::Sender<CompletedPiece>,
        failure_tx: mpsc::Sender<FailedPiece>,
        disconnect_tx: mpsc::Sender<PeerDisconnected>,
        client_peer_id: [u8; 20],
        info_hash: [u8; 20],
        num_pieces: usize,
    ) -> Result<ConnectedPeer>;
}

pub struct RealPeerConnector;

#[async_trait]
impl PeerConnector for RealPeerConnector {
    async fn connect(
        &self,
        peer: Peer,
        completion_tx: mpsc::Sender<CompletedPiece>,
        failure_tx: mpsc::Sender<FailedPiece>,
        disconnect_tx: mpsc::Sender<PeerDisconnected>,
        client_peer_id: [u8; 20],
        info_hash: [u8; 20],
        num_pieces: usize,
    ) -> Result<ConnectedPeer> {
        let tcp_connector = Arc::new(RealTcpConnector);
        let (mut peer_conn, download_request_tx, bitfield) = PeerConnection::new(
            peer.clone(),
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
        );

        peer_conn.handshake(client_peer_id, info_hash).await?;

        let _ = peer_conn.get_peer_id().ok_or_else(|| {
            anyhow::Error::from(AppError::handshake_failed(
                "Failed to get peer ID after handshake",
            ))
        })?;

        peer_conn.start(num_pieces).await?;

        Ok(ConnectedPeer::new(
            peer.clone(),
            download_request_tx,
            bitfield,
        ))
    }
}

pub struct PeerManager {
    // Some fields are public to allow testability.
    tracker_client: Arc<dyn TrackerClient>,

    pub available_peers: Arc<RwLock<HashMap<String, Peer>>>,
    pub connected_peers: Arc<RwLock<HashMap<String, ConnectedPeer>>>,

    config: Option<PeerManagerConfig>,

    pub pending_pieces: Arc<RwLock<VecDeque<PieceDownloadRequest>>>,
    pub in_flight_pieces: Arc<RwLock<HashMap<u32, String>>>,
    pub completed_pieces: Arc<RwLock<usize>>,

    pub failed_attempts: Arc<RwLock<HashMap<u32, usize>>>,
    pub piece_requests: Arc<RwLock<HashMap<u32, PieceDownloadRequest>>>,

    connector: Arc<dyn PeerConnector>,
}

impl PeerManager {
    pub fn new(decoder: Decoder, http_client: Client) -> Self {
        let tracker_client = Arc::new(HttpTrackerClient::new(http_client, decoder));
        Self::new_with_connector(tracker_client, Arc::new(RealPeerConnector))
    }

    pub fn new_with_connector(
        tracker_client: Arc<dyn TrackerClient>,
        connector: Arc<dyn PeerConnector>,
    ) -> Self {
        Self {
            tracker_client,
            available_peers: Arc::new(RwLock::new(HashMap::new())),
            connected_peers: Arc::new(RwLock::new(HashMap::new())),
            config: None,
            pending_pieces: Arc::new(RwLock::new(VecDeque::new())),
            in_flight_pieces: Arc::new(RwLock::new(HashMap::new())),
            completed_pieces: Arc::new(RwLock::new(0)),
            failed_attempts: Arc::new(RwLock::new(HashMap::new())),
            piece_requests: Arc::new(RwLock::new(HashMap::new())),
            connector,
        }
    }

    pub async fn initialize(&mut self, config: PeerManagerConfig) -> Result<()> {
        self.config = Some(config);
        Ok(())
    }

    fn config(&self) -> Result<&PeerManagerConfig> {
        self.config.as_ref().ok_or_else(|| {
            anyhow::Error::from(AppError::missing_field("PeerManager not initialized"))
        })
    }

    /// Cleanup piece tracking after completion/failure.
    pub async fn cleanup_piece_tracking(&self, piece_index: u32) -> Option<String> {
        let mut in_flight = self.in_flight_pieces.write().await;
        let peer_addr = in_flight.remove(&piece_index)?;

        let mut connected_peers = self.connected_peers.write().await;
        if let Some(peer) = connected_peers.get_mut(&peer_addr) {
            peer.active_downloads.remove(&piece_index);
        }

        Some(peer_addr)
    }

    pub async fn start(
        self: Arc<Self>,
        announce_url: String,
        piece_completion_tx: mpsc::Sender<CompletedPiece>,
        download_complete_tx: mpsc::Sender<DownloadComplete>,
    ) -> Result<PeerManagerHandle> {
        let config = self.config()?;

        let info_hash = config.info_hash;
        let file_size = config.file_size;
        let max_peers = config.max_peers;
        let num_pieces = config.num_pieces;

        // Create shutdown channel for graceful termination in tests
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);

        // Create shared disconnect channel for all peers
        let (disconnect_tx, disconnect_rx) = mpsc::channel::<PeerDisconnected>(CHANNEL_SIZE);

        // Create channels for PeerConnections to send events
        let (completion_tx, completion_rx) = mpsc::channel::<CompletedPiece>(CHANNEL_SIZE);
        let (failure_tx, failure_rx) = mpsc::channel::<FailedPiece>(CHANNEL_SIZE);

        // Spawn completion handler task
        tokio::task::spawn(self.clone().handle_completion_events(
            completion_rx,
            piece_completion_tx.clone(),
            download_complete_tx.clone(),
            num_pieces,
        ));

        // Spawn failure handler task with retry logic
        tokio::task::spawn(self.clone().handle_failure_events(failure_rx));

        // Spawn disconnect handler task
        tokio::task::spawn(
            self.clone()
                .handle_disconnect_events(disconnect_rx, failure_tx.clone()),
        );

        // Spawn task to connect with peers
        tokio::task::spawn(self.clone().handle_peer_connector(
            max_peers,
            completion_tx.clone(),
            failure_tx.clone(),
            disconnect_tx.clone(),
            shutdown_rx.resubscribe(),
        ));

        // Spawn task to assign pieces to peers
        tokio::task::spawn(
            self.clone()
                .handle_piece_assignment(shutdown_rx.resubscribe()),
        );

        // Spawn task to show the current status of the pieces
        let peer_manager_progress = self.clone();
        tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                let completed = *peer_manager_progress.completed_pieces.read().await;
                let connected_peers = peer_manager_progress.connected_peers.read().await.len();
                let pending = peer_manager_progress.pending_pieces.read().await.len();
                let in_flight = peer_manager_progress.in_flight_pieces.read().await.len();
                let percentage = (completed * 100) / num_pieces;

                info!(
                    "[Progress] {}/{} pieces ({}%) | {} connected peers | {} pending pieces | {} in-flight pieces",
                    completed, num_pieces, percentage, connected_peers, pending, in_flight
                );

                interval.tick().await;
            }
        });

        self.clone()
            .watch_tracker(
                Duration::from_secs(WATCH_TRACKER_DELAY),
                announce_url,
                info_hash,
                file_size,
            )
            .await?;

        Ok(PeerManagerHandle::new(shutdown_tx))
    }

    /// Process a single completed piece event.
    ///
    /// Returns true to continue processing, or false when download is complete.
    pub async fn process_completion(
        &self,
        completed: CompletedPiece,
        file_writer_tx: &mpsc::Sender<CompletedPiece>,
        download_complete_tx: &mpsc::Sender<DownloadComplete>,
        num_pieces: usize,
    ) -> bool {
        let piece_index = completed.piece_index;

        if let Some(peer_addr) = self.cleanup_piece_tracking(piece_index).await {
            let connected_peers = self.connected_peers.read().await;
            let active_count = connected_peers
                .get(&peer_addr)
                .map(|p| p.active_downloads.len())
                .unwrap_or(0);

            debug!(
                "Peer {} completed piece {}, active_downloads now: {}",
                peer_addr, piece_index, active_count
            );
        }

        // Increment progress
        let progress = {
            let mut completed_count = self.completed_pieces.write().await;
            *completed_count += 1;
            *completed_count
        };

        // Forward to FileManager (only successful pieces)
        let _ = file_writer_tx.send(completed).await;

        // Check if all pieces complete
        if progress >= num_pieces {
            let _ = download_complete_tx.send(DownloadComplete).await;
            debug!(
                "All {} pieces completed, sent DownloadComplete signal",
                num_pieces
            );
            return false; // Download complete
        }

        true // Continue processing
    }

    /// Handle completed piece events - extracted for testing
    async fn handle_completion_events(
        self: Arc<Self>,
        mut completion_rx: mpsc::Receiver<CompletedPiece>,
        file_writer_tx: mpsc::Sender<CompletedPiece>,
        download_complete_tx: mpsc::Sender<DownloadComplete>,
        num_pieces: usize,
    ) {
        while let Some(completed) = completion_rx.recv().await {
            if !self
                .process_completion(
                    completed,
                    &file_writer_tx,
                    &download_complete_tx,
                    num_pieces,
                )
                .await
            {
                break; // Download complete
            }
        }
    }

    /// Process a single failed piece event.
    ///
    /// Returns Ok(true) if piece was requeued, Ok(false) if max retries exceeded.
    pub async fn process_failure(&self, failed: FailedPiece) -> Result<bool> {
        let piece_index = failed.piece_index;

        if let Some(peer_addr) = self.cleanup_piece_tracking(piece_index).await {
            let connected_peers = self.connected_peers.read().await;
            let active_count = connected_peers
                .get(&peer_addr)
                .map(|p| p.active_downloads.len())
                .unwrap_or(0);

            debug!(
                "Peer {} failed piece {} ({}), active_downloads now: {}",
                peer_addr, piece_index, failed.reason, active_count
            );
        }

        // Check retry attempts
        let attempts = {
            let mut failed_attempts = self.failed_attempts.write().await;
            let attempts = failed_attempts.entry(piece_index).or_insert(0);
            *attempts += 1;
            *attempts
        };

        if attempts >= MAX_FAILED_PIECE_RETRY {
            error!(
                "Piece {} failed after {} attempts ({}), aborting download",
                piece_index, MAX_FAILED_PIECE_RETRY, failed.reason
            );
            return Ok(false); // Max retries exceeded
        }

        debug!(
            "Piece {} failed (attempt {}/{}): {}, re-queuing",
            piece_index, attempts, MAX_FAILED_PIECE_RETRY, failed.reason
        );

        // Re-queue for retry using stored request
        let request = {
            let piece_requests = self.piece_requests.read().await;
            piece_requests.get(&piece_index).cloned()
        };

        if let Some(request) = request {
            let mut pending = self.pending_pieces.write().await;
            pending.push_back(request);
            Ok(true) // Requeued successfully
        } else {
            error!(
                "Cannot retry piece {}: original request not found",
                piece_index
            );
            Err(anyhow::anyhow!(
                "Original request not found for piece {}",
                piece_index
            ))
        }
    }

    /// Handle failed piece events - extracted for testing
    async fn handle_failure_events(self: Arc<Self>, mut failure_rx: mpsc::Receiver<FailedPiece>) {
        while let Some(failed) = failure_rx.recv().await {
            let _ = self.process_failure(failed).await;
        }
    }

    /// Process a single peer disconnect event.
    ///
    /// Returns the number of pieces that were re-queued for download.
    pub async fn process_disconnect(
        &self,
        disconnect: PeerDisconnected,
        failure_tx: &mpsc::Sender<FailedPiece>,
    ) -> Result<usize> {
        let peer_addr = disconnect.peer.get_addr();
        debug!("Peer {} disconnected: {}", peer_addr, disconnect.reason);

        // Extract peer's active downloads before removing
        let active_downloads = {
            let mut connected_peers = self.connected_peers.write().await;
            connected_peers
                .remove(&peer_addr)
                .map(|peer| peer.active_downloads)
        };

        // Clean up in-flight pieces for this peer
        if let Some(downloads) = active_downloads
            && !downloads.is_empty()
        {
            debug!(
                "Marking {} pieces as failed due to peer disconnect: {}",
                downloads.len(),
                peer_addr
            );

            let count = downloads.len();
            let mut in_flight = self.in_flight_pieces.write().await;

            for piece_idx in downloads {
                // Remove from global in-flight tracking
                in_flight.remove(&piece_idx);

                // Send to internal failure handler for retry logic
                let _ = failure_tx
                    .send(FailedPiece {
                        piece_index: piece_idx,
                        reason: "peer_disconnected".to_string(),
                    })
                    .await;
            }

            Ok(count)
        } else {
            Ok(0)
        }
    }

    /// Handle peer disconnect events - extracted for testing
    async fn handle_disconnect_events(
        self: Arc<Self>,
        mut disconnect_rx: mpsc::Receiver<PeerDisconnected>,
        failure_tx: mpsc::Sender<FailedPiece>,
    ) {
        while let Some(disconnect) = disconnect_rx.recv().await {
            let _ = self.process_disconnect(disconnect, &failure_tx).await;
        }
    }

    /// Periodically connect to peers - extracted for testing
    async fn handle_peer_connector(
        self: Arc<Self>,
        max_peers: usize,
        completion_tx: mpsc::Sender<CompletedPiece>,
        failure_tx: mpsc::Sender<FailedPiece>,
        disconnect_tx: mpsc::Sender<PeerDisconnected>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Peer connector task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    let _ = self
                        .connect_with_peers(
                            max_peers,
                            completion_tx.clone(),
                            failure_tx.clone(),
                            disconnect_tx.clone(),
                        )
                        .await;
                }
            }
        }
    }

    /// Try to assign one piece from the queue to an available peer.
    ///
    /// Returns Ok(true) if a piece was assigned, Ok(false) if queue is empty,
    /// or Err if assignment failed and piece was re-queued.
    pub async fn try_assign_piece(&self) -> Result<bool> {
        let pending_piece = {
            let mut pending = self.pending_pieces.write().await;
            pending.pop_front()
        };

        if let Some(request) = pending_piece {
            if let Err(e) = self.assign_piece_to_peer(request.clone()).await {
                let mut pending = self.pending_pieces.write().await;
                pending.push_back(request);
                return Err(e); // Assignment failed, piece re-queued
            }
            Ok(true) // Successfully assigned
        } else {
            Ok(false) // Queue is empty
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
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                } => {}
            }
        }
    }

    /// Creates the connection with a peer over the network.
    pub async fn connect_peer(
        &self,
        peer: Peer,
        completion_tx: mpsc::Sender<CompletedPiece>,
        failure_tx: mpsc::Sender<FailedPiece>,
        disconnect_tx: mpsc::Sender<PeerDisconnected>,
    ) -> Result<ConnectedPeer> {
        let config = self.config()?;

        self.connector
            .connect(
                peer,
                completion_tx,
                failure_tx,
                disconnect_tx,
                config.client_peer_id,
                config.info_hash,
                config.num_pieces,
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

        let mut connected_peers = self.connected_peers.write().await;

        // Debug: Log bitfield state for all connected peers
        for (addr, peer) in connected_peers.iter() {
            let has_piece = peer.has_piece(piece_index as usize).await;
            let is_busy = peer.active_downloads.contains(&piece_index);
            let num_pieces = peer.piece_count().await;
            let bitfield_len = peer.bitfield_len().await;
            debug!(
                "Peer {}, piece={}, has_piece={}, already_downloading={}, active_downloads={}, total_pieces={}/{}",
                piece_index,
                addr,
                has_piece,
                is_busy,
                peer.active_downloads.len(),
                num_pieces,
                bitfield_len
            );
        }

        // Selects the best peer to request the piece
        // Collect candidates that have the piece and aren't already downloading it
        let mut candidates: Vec<(String, usize)> = Vec::new();
        for (addr, peer) in connected_peers.iter() {
            let has_piece = peer.has_piece(piece_index as usize).await;
            if has_piece && !peer.active_downloads.contains(&piece_index) {
                candidates.push((addr.clone(), peer.active_downloads.len()));
            }
        }

        let best_peer_addr = candidates
            .iter()
            .min_by_key(|(_, active_count)| active_count)
            .map(|(addr, _)| addr.clone())
            .ok_or(anyhow::Error::from(AppError::NoPeersAvailable))?;

        let best_peer = connected_peers
            .get_mut(&best_peer_addr)
            .ok_or(anyhow::Error::from(AppError::NoPeersAvailable))?;

        debug!(
            "Peer {} selected to download the piece {}",
            best_peer.peer.get_addr(),
            piece_index
        );

        best_peer.request_piece(request.clone()).await?;
        best_peer.active_downloads.insert(piece_index);

        let mut in_flight = self.in_flight_pieces.write().await;
        in_flight.insert(piece_index, best_peer.peer.get_addr());

        // Store the request for potential retries
        let mut piece_requests = self.piece_requests.write().await;
        piece_requests.insert(piece_index, request);

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
                    }
                    Err(e) => {
                        debug!("Tracker request failed: {}", e);
                    }
                }
            }
        });

        Ok(())
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
        let request = AnnounceRequest {
            endpoint: endpoint.to_string(),
            info_hash: info_hash.to_vec(),
            peer_id: "postman-000000000001".to_string(),
            port: 6881,
            uploaded: 0,
            downloaded: 0,
            left: file_size,
        };

        let response = self.tracker_client.announce(request).await?;
        Ok(response.peers)
    }

    /// Connect to available peers up to the specified limit.
    ///
    /// This method attempts to establish connections with peers from the available peer pool
    /// until reaching the maximum number of concurrent peers. It's called automatically by
    /// the background orchestration task in `start()`, but can also be invoked manually
    /// when immediate peer connections are needed.
    ///
    /// Returns `Ok(usize)` with the number of successfully connected peers, or an error if
    /// the operation fails critically.
    pub async fn connect_with_peers(
        &self,
        max_peers: usize,
        completion_tx: mpsc::Sender<CompletedPiece>,
        failure_tx: mpsc::Sender<FailedPiece>,
        disconnect_tx: mpsc::Sender<PeerDisconnected>,
    ) -> Result<usize> {
        let (peers_to_connect, available_count, connected_count, needed) = {
            let available = self.available_peers.read().await;
            let connected = self.connected_peers.read().await;
            let needed = max_peers.saturating_sub(connected.len());

            let peers = available
                .values()
                .filter(|p| !connected.contains_key(&p.get_addr()))
                .take(needed)
                .cloned()
                .collect::<Vec<_>>();

            (peers, available.len(), connected.len(), needed)
        };

        let mut successful_connections = 0;

        debug!(
            "Connection attempt: available_peers={}, connected_peers={}, needed={}, attempting={}",
            available_count,
            connected_count,
            needed,
            peers_to_connect.len()
        );

        // Spawn concurrent connection attempts
        let mut tasks = Vec::new();
        for peer in peers_to_connect {
            let completion_tx = completion_tx.clone();
            let failure_tx = failure_tx.clone();
            let disconnect_tx = disconnect_tx.clone();
            let config = self.config()?.clone();
            let connector = self.connector.clone();

            let task = tokio::spawn(async move {
                let peer_addr = peer.get_addr();
                let result = connector
                    .connect(
                        peer.clone(),
                        completion_tx,
                        failure_tx,
                        disconnect_tx,
                        config.client_peer_id,
                        config.info_hash,
                        config.num_pieces,
                    )
                    .await;

                (peer_addr, result)
            });
            tasks.push(task);
        }

        // Collect results
        for task in tasks {
            match task.await {
                Ok((peer_addr, Ok(connected_peer))) => {
                    info!("Successfully connected to peer {}", peer_addr);
                    self.connected_peers
                        .write()
                        .await
                        .insert(peer_addr.clone(), connected_peer);
                    successful_connections += 1;
                }
                Ok((peer_addr, Err(e))) => {
                    debug!("Failed to connect to peer {}: {}", peer_addr, e);
                    self.available_peers.write().await.remove(&peer_addr);
                }
                Err(e) => {
                    error!("Connection task panicked: {}", e);
                }
            }
        }

        if successful_connections > 0 {
            debug!("Connected {} new peers", successful_connections);
        }

        Ok(successful_connections)
    }

    /// Fetches a piece from the pending list and try to assign it to a connected peer.
    pub async fn download_piece(&self, request: PieceDownloadRequest) -> Result<()> {
        // Add to pending queue first (safety net)
        {
            let mut pending = self.pending_pieces.write().await;
            pending.push_back(request.clone());
        }

        // Attempt immediate assignment to a peer (errors handled by background task)
        let _ = self.assign_piece_to_peer(request).await;

        Ok(())
    }
}
