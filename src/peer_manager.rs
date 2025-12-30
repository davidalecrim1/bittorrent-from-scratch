use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::encoding::Decoder;
use crate::error::{AppError, Result};
use crate::piece_state::PieceStateManager;
use crate::tcp_connector::DefaultTcpStreamFactory;
use crate::tracker_client::{HttpTrackerClient, TrackerClient};
use crate::types::{
    AnnounceRequest, CompletedPiece, ConnectedPeer, DownloadComplete, FailedPiece, Peer,
    PeerConnection, PeerDisconnected, PeerEvent, PeerManagerConfig, PeerManagerHandle,
    PieceDownloadRequest,
};
use async_trait::async_trait;
use log::{debug, info};
use reqwest::Client;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio::time;

const WATCH_TRACKER_DELAY: u64 = 30;
const MAX_PEERS_TO_CONNECT: usize = 20;

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
    ) -> Result<ConnectedPeer> {
        let tcp_connector = Arc::new(DefaultTcpStreamFactory);
        let (mut peer_conn, download_request_tx, bitfield) =
            PeerConnection::new(peer.clone(), event_tx, tcp_connector);

        peer_conn.handshake(client_peer_id, info_hash).await?;
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

    piece_state: Arc<PieceStateManager>,

    connector: Arc<dyn PeerConnectionFactory>,
}

impl PeerManager {
    pub fn new(decoder: Decoder, http_client: Client) -> Self {
        let tracker_client = Arc::new(HttpTrackerClient::new(http_client, decoder));
        Self::new_with_connector(tracker_client, Arc::new(DefaultPeerConnectionFactory))
    }

    pub fn new_with_connector(
        tracker_client: Arc<dyn TrackerClient>,
        connector: Arc<dyn PeerConnectionFactory>,
    ) -> Self {
        Self {
            tracker_client,
            available_peers: Arc::new(RwLock::new(HashMap::new())),
            connected_peers: Arc::new(RwLock::new(HashMap::new())),
            config: None,
            piece_state: Arc::new(PieceStateManager::new()),
            connector,
        }
    }

    pub async fn initialize(&mut self, config: PeerManagerConfig) -> Result<()> {
        let num_pieces = config.num_pieces;
        self.piece_state.initialize(num_pieces).await;
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
        let peer_addr = self.piece_state.get_peer_for_piece(piece_index).await?;

        let mut connected_peers = self.connected_peers.write().await;
        if let Some(peer) = connected_peers.get_mut(&peer_addr) {
            peer.mark_complete(piece_index);
        }

        Some(peer_addr)
    }

    pub async fn get_piece_snapshot(&self) -> crate::piece_state::PieceStats {
        self.piece_state.get_snapshot().await
    }

    pub async fn is_piece_completed(&self, piece_index: u32) -> bool {
        self.piece_state.is_completed(piece_index).await
    }

    pub async fn is_piece_pending(&self, piece_index: u32) -> bool {
        self.piece_state.is_pending(piece_index).await
    }

    pub async fn is_piece_in_flight(&self, piece_index: u32) -> bool {
        self.piece_state.is_in_flight(piece_index).await
    }

    pub async fn start(
        self: Arc<Self>,
        announce_url: String,
        piece_completion_tx: mpsc::UnboundedSender<CompletedPiece>,
        download_complete_tx: mpsc::Sender<DownloadComplete>,
    ) -> Result<PeerManagerHandle> {
        let config = self.config()?;

        let info_hash = config.info_hash;
        let file_size = config.file_size;
        let num_pieces = config.num_pieces;

        // Create shutdown channel for graceful termination in tests
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);

        // Create unified event channel for all peer events
        let (event_tx, event_rx) = mpsc::unbounded_channel::<PeerEvent>();

        // Spawn unified event handler task
        tokio::task::spawn(self.clone().handle_peer_events(
            event_rx,
            piece_completion_tx.clone(),
            download_complete_tx.clone(),
            num_pieces,
        ));

        // Spawn task to connect with peers
        tokio::task::spawn(self.clone().handle_peer_connector(
            MAX_PEERS_TO_CONNECT,
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
        tokio::task::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;

                // Get atomic snapshot from PieceStateManager
                let snapshot = peer_manager_progress.get_piece_snapshot().await;
                let (completed, pending, in_flight) = (
                    snapshot.completed_count,
                    snapshot.pending_count,
                    snapshot.in_flight_count,
                );

                let connected_peers = peer_manager_progress.connected_peers.read().await.len();
                let percentage = (completed * 100) / num_pieces;

                // Print to terminal on same line (overwrites previous)
                // Note: We use print! (not info!) to avoid newlines from the logger
                use std::io::Write;

                // Use ANSI escape codes to clear the line properly
                print!(
                    "\x1B[2K\r[Progress] {}/{} pieces ({}%) | {} peers | {} pending | {} in-flight",
                    completed, num_pieces, percentage, connected_peers, pending, in_flight
                );
                std::io::stdout().flush().unwrap();
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
        file_writer_tx: &mpsc::UnboundedSender<CompletedPiece>,
        download_complete_tx: &mpsc::Sender<DownloadComplete>,
        num_pieces: usize,
    ) -> Result<()> {
        let piece_index = completed.piece_index;

        let _ = self.piece_state.complete_piece(piece_index).await;

        if let Some(peer_addr) = self.cleanup_piece_tracking(piece_index).await {
            let connected_peers = self.connected_peers.read().await;
            let active_count = connected_peers
                .get(&peer_addr)
                .map(|p| p.active_download_count())
                .unwrap_or(0);

            debug!(
                "Peer {} completed piece {}, active_downloads now: {}",
                peer_addr, piece_index, active_count
            );
        }

        let progress = self.piece_state.get_snapshot().await.completed_count;

        file_writer_tx.send(completed)?;

        if progress >= num_pieces {
            download_complete_tx.send(DownloadComplete).await?;
            debug!(
                "All {} pieces completed, sent DownloadComplete signal",
                num_pieces
            );
        }

        Ok(())
    }

    /// Process a single failed piece event.
    pub async fn process_failure(&self, failed: FailedPiece) -> Result<()> {
        let piece_index = failed.piece_index;

        if self.piece_state.is_completed(piece_index).await {
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

            self.piece_state
                .retry_piece(piece_index, &failed.error.to_string(), true)
                .await;
            return Ok(());
        }

        if let Some(peer_addr) = self.cleanup_piece_tracking(piece_index).await {
            let connected_peers = self.connected_peers.read().await;
            let active_count = connected_peers
                .get(&peer_addr)
                .map(|p| p.active_download_count())
                .unwrap_or(0);

            debug!(
                "Peer {} failed piece {} ({}), active_downloads now: {}",
                peer_addr, piece_index, failed.error, active_count
            );
        }

        let retry_count = self
            .piece_state
            .retry_piece(piece_index, &failed.error.to_string(), false)
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
        debug!("Peer {} disconnected: {}", peer_addr, disconnect.reason);

        let pieces_in_flight = self.piece_state.get_peer_pieces(&peer_addr).await;

        {
            let mut connected_peers = self.connected_peers.write().await;
            connected_peers.remove(&peer_addr);
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
                    error: AppError::PeerDisconnected,
                    push_front: false,
                })
                .await?;
            }
        }

        Ok(())
    }

    /// Unified event handler for all peer events - merges completion, failure, and disconnect handlers
    async fn handle_peer_events(
        self: Arc<Self>,
        mut event_rx: mpsc::UnboundedReceiver<PeerEvent>,
        file_writer_tx: mpsc::UnboundedSender<CompletedPiece>,
        download_complete_tx: mpsc::Sender<DownloadComplete>,
        num_pieces: usize,
    ) {
        while let Some(event) = event_rx.recv().await {
            match event {
                PeerEvent::Completion(completed) => {
                    let _ = self
                        .process_completion(
                            completed,
                            &file_writer_tx,
                            &download_complete_tx,
                            num_pieces,
                        )
                        .await;
                }
                PeerEvent::Failure(failed) => {
                    let _ = self.process_failure(failed).await;
                }
                PeerEvent::Disconnect(disconnect) => {
                    let _ = self.process_disconnect(disconnect).await;
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
        // Connect immediately on startup (don't wait for first interval tick)
        let _ = self.connect_with_peers(max_peers, event_tx.clone()).await;

        let mut interval = tokio::time::interval(Duration::from_secs(10));
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
    /// Returns Ok(true) if a piece was assigned, Ok(false) if queue is empty,
    /// or Err if assignment failed and piece was re-queued.
    pub async fn try_assign_piece(&self) -> Result<bool> {
        let piece_index = self.piece_state.pop_pending().await;

        if let Some(index) = piece_index {
            let request = self.piece_state.get_request(index).await;

            if let Some(req) = request {
                if let Err(e) = self.assign_piece_to_peer(req.clone()).await {
                    self.piece_state.requeue_piece(index).await;
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
                            tokio::time::sleep(Duration::from_millis(100)).await;
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
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        interval.tick().await; // Skip first immediate tick

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("Stale download checker task shutting down");
                    break;
                }
                _ = interval.tick() => {
                    let stale_downloads = self.piece_state.get_stale_downloads().await;

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
                let has_piece = peer.has_piece(piece_index as usize).await;
                let is_busy = peer.is_downloading(piece_index);
                let active_count = peer.active_download_count();
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

                if has_piece && !is_busy {
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

        // Debug logging happens WITHOUT holding any locks
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
        sender.send(request.clone()).await?;

        // Phase 4: Update state with brief WRITE lock
        {
            let mut connected_peers = self.connected_peers.write().await;
            if let Some(peer) = connected_peers.get_mut(&best_peer_addr) {
                peer.mark_downloading(piece_index);
            }
        } // Write lock released here

        let _ = self
            .piece_state
            .start_download(piece_index, best_peer_addr)
            .await;

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
                        .drop_useless_peers_if_at_capacity(MAX_PEERS_TO_CONNECT)
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
                            .drop_useless_peers_if_at_capacity(MAX_PEERS_TO_CONNECT)
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

    /// Drop peers with empty bitfields when at max capacity.
    ///
    /// This is called after tracker announces to make room for potentially
    /// better peers when we're at maximum connections. Peers with no pieces
    /// (empty bitfield) are useless and should be dropped first.
    pub async fn drop_useless_peers_if_at_capacity(&self, max_peers: usize) {
        let peers_to_drop = {
            let connected = self.connected_peers.read().await;

            if connected.len() < max_peers {
                return;
            }

            let mut useless_peers = Vec::new();
            for (addr, peer) in connected.iter() {
                let piece_count = peer.piece_count().await;
                if piece_count == 0 {
                    useless_peers.push(addr.clone());
                }
            }

            useless_peers
        };

        if peers_to_drop.is_empty() {
            return;
        }

        debug!(
            "At max capacity ({} peers), dropping {} peers with empty bitfields",
            max_peers,
            peers_to_drop.len()
        );

        let mut connected = self.connected_peers.write().await;
        for addr in peers_to_drop {
            if let Some(peer) = connected.remove(&addr) {
                info!(
                    "Dropped peer {} (empty bitfield, no pieces available)",
                    addr
                );
                drop(peer);
            }
        }
    }

    /// Connect to available peers up to the specified limit.
    ///
    /// This method attempts to establish connections with peers from the available peer pool
    /// until reaching the maximum number of concurrent peers. It's called automatically by
    /// the background orchestration task in `start()`, but can also be invoked manually
    /// when immediate peer connections are needed.
    ///
    pub async fn connect_with_peers(
        &self,
        max_peers: usize,
        event_tx: mpsc::UnboundedSender<PeerEvent>,
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

        if peers_to_connect.is_empty() {
            if needed > 0 {
                debug!(
                    "Peer connector heartbeat: need {} more peers but no new peers available (available={}, connected={})",
                    needed, available_count, connected_count
                );
            } else {
                debug!(
                    "Peer connector heartbeat: at capacity ({}/{} peers connected)",
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
        let num_attempts = peers_to_connect.len();
        for peer in peers_to_connect {
            let connected_peers = self.connected_peers.clone();
            let available_peers = self.available_peers.clone();
            let connector = self.connector.clone();
            let config = self.config()?.clone();
            let event_tx = event_tx.clone();

            tokio::spawn(async move {
                let peer_addr = peer.get_addr();
                match connector
                    .connect(
                        peer,
                        event_tx,
                        config.client_peer_id,
                        config.info_hash,
                        config.num_pieces,
                    )
                    .await
                {
                    Ok(connected_peer) => {
                        info!("Successfully connected to peer {}", peer_addr);
                        let mut connected = connected_peers.write().await;
                        connected.insert(peer_addr, connected_peer);
                    }
                    Err(e) => {
                        debug!("Failed to connect to peer {}: {}", peer_addr, e);
                        available_peers.write().await.remove(&peer_addr);
                    }
                }
            });
        }

        Ok(num_attempts)
    }

    /// Fetches a piece from the pending list and try to assign it to a connected peer.
    pub async fn download_piece(&self, request: PieceDownloadRequest) -> Result<()> {
        let _ = self.piece_state.queue_piece(request.clone()).await;

        let _ = self.assign_piece_to_peer(request).await;

        Ok(())
    }
}
