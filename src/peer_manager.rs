use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use crate::error::{AppError, Result};
use crate::types::{
    BencodeTypes, CompletedPiece, ConnectedPeer, FailedPiece, Peer, PeerConnection,
    PeerDisconnected, PeerManagerConfig, PieceDownloadRequest,
};
use log::debug;
use reqwest::{Client, Url};
use tokio::sync::{RwLock, mpsc};
use tokio::time;

use crate::encoding::Decoder;

#[derive(Debug)]
pub struct PeerManager {
    http_client: Client,

    available_peers: Arc<RwLock<HashMap<String, Peer>>>,
    connected_peers: Arc<RwLock<HashMap<String, ConnectedPeer>>>,

    decoder: Decoder,

    config: Option<PeerManagerConfig>,

    pending_pieces: Arc<RwLock<VecDeque<PieceDownloadRequest>>>,
    in_flight_pieces: Arc<RwLock<HashMap<u32, String>>>,
}

impl PeerManager {
    pub fn new(decoder: Decoder, http_client: Client) -> Self {
        Self {
            decoder,
            http_client,
            available_peers: Arc::new(RwLock::new(HashMap::new())),
            connected_peers: Arc::new(RwLock::new(HashMap::new())),
            config: None,
            pending_pieces: Arc::new(RwLock::new(VecDeque::new())),
            in_flight_pieces: Arc::new(RwLock::new(HashMap::new())),
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

    pub async fn start(
        self: Arc<Self>,
        announce_url: String,
        piece_completion_tx: mpsc::Sender<CompletedPiece>,
        piece_failure_tx: mpsc::Sender<FailedPiece>,
    ) -> Result<()> {
        let config = self.config()?;

        let info_hash = config.info_hash;
        let file_size = config.file_size;
        let max_peers = config.max_peers;

        // Create shared disconnect channel for all peers
        let (disconnect_tx, mut disconnect_rx) = mpsc::channel::<PeerDisconnected>(100);

        // Spawn disconnect handler task
        let peer_manager_disconnect = self.clone();
        tokio::task::spawn(async move {
            while let Some(disconnect) = disconnect_rx.recv().await {
                debug!(
                    "[PeerManager] Peer {} disconnected: {}",
                    disconnect.peer_addr, disconnect.reason
                );

                // Remove from connected_peers
                peer_manager_disconnect
                    .connected_peers
                    .write()
                    .await
                    .remove(&disconnect.peer_addr);

                // Re-queue any in-flight pieces from this peer
                let mut in_flight = peer_manager_disconnect.in_flight_pieces.write().await;
                let failed_pieces: Vec<u32> = in_flight
                    .iter()
                    .filter(|(_, addr)| **addr == disconnect.peer_addr)
                    .map(|(piece_idx, _)| *piece_idx)
                    .collect();

                for piece_idx in failed_pieces {
                    in_flight.remove(&piece_idx);
                    debug!(
                        "[PeerManager] Re-queueing piece {} from disconnected peer {}",
                        piece_idx, disconnect.peer_addr
                    );
                    // Note: Pieces will be reassigned by the assignment background task
                    // which picks from pending_pieces queue
                }
            }
        });

        let peer_manager = self.clone();
        let completion_tx_bg = piece_completion_tx.clone();
        let failure_tx_bg = piece_failure_tx.clone();
        let disconnect_tx_bg = disconnect_tx.clone();
        tokio::task::spawn(async move {
            loop {
                if let Err(e) = peer_manager
                    .connect_with_peers(
                        max_peers,
                        completion_tx_bg.clone(),
                        failure_tx_bg.clone(),
                        disconnect_tx_bg.clone(),
                    )
                    .await
                {
                    debug!("[PeerManager::start] Error connecting peers: {}", e);
                }

                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });

        let peer_manager_assignment = self.clone();
        tokio::task::spawn(async move {
            loop {
                let pending_piece = {
                    let mut pending = peer_manager_assignment.pending_pieces.write().await;
                    pending.pop_front()
                };

                if let Some(request) = pending_piece {
                    if let Err(_e) = peer_manager_assignment
                        .assign_piece_to_peer(request.clone())
                        .await
                    {
                        let mut pending = peer_manager_assignment.pending_pieces.write().await;
                        pending.push_back(request);
                    }
                } else {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        });

        self.clone()
            .watch_tracker(
                Duration::from_secs(2 * 60),
                announce_url,
                info_hash,
                file_size,
            )
            .await?;

        Ok(())
    }

    async fn connect_peer(
        &self,
        peer: Peer,
        completion_tx: mpsc::Sender<CompletedPiece>,
        failure_tx: mpsc::Sender<FailedPiece>,
        disconnect_tx: mpsc::Sender<PeerDisconnected>,
    ) -> Result<ConnectedPeer> {
        let config = self.config()?;

        let (mut peer_conn, download_request_tx) =
            PeerConnection::new(peer.clone(), completion_tx, failure_tx, disconnect_tx);

        peer_conn
            .handshake(config.client_peer_id, config.info_hash)
            .await?;

        let _ = peer_conn.get_peer_id().ok_or_else(|| {
            anyhow::Error::from(AppError::handshake_failed(
                "Failed to get peer ID after handshake",
            ))
        })?;

        let bitfield = peer_conn.get_bitfield().to_vec();

        peer_conn.start(config.num_pieces).await?;

        Ok(ConnectedPeer {
            peer: peer.clone(),
            download_request_tx,
            bitfield,
            active_downloads: HashSet::new(),
        })
    }

    pub async fn request_pieces(&self, requests: Vec<PieceDownloadRequest>) -> Result<()> {
        let mut pending = self.pending_pieces.write().await;
        for request in requests {
            pending.push_back(request);
        }
        debug!(
            "[PeerManager::request_pieces] Queued {} pieces for download",
            pending.len()
        );
        Ok(())
    }

    async fn assign_piece_to_peer(&self, request: PieceDownloadRequest) -> Result<()> {
        let piece_index = request.piece_index;

        let mut connected_peers = self.connected_peers.write().await;

        let best_peer = connected_peers
            .values_mut()
            .filter(|p| {
                p.bitfield
                    .get(piece_index as usize)
                    .copied()
                    .unwrap_or(false)
                    && !p.active_downloads.contains(&piece_index)
            })
            .min_by_key(|p| p.active_downloads.len())
            .ok_or(anyhow::Error::from(AppError::NoPeersAvailable))?;

        best_peer
            .download_request_tx
            .send(request)
            .await
            .map_err(|e| {
                anyhow::Error::from(AppError::channel_send(format!(
                    "Failed to send download request: {}",
                    e
                )))
            })?;

        best_peer.active_downloads.insert(piece_index);

        let mut in_flight = self.in_flight_pieces.write().await;
        in_flight.insert(piece_index, best_peer.peer.get_addr());

        debug!(
            "[PeerManager::assign_piece_to_peer] Assigned piece {} to peer {}",
            piece_index,
            best_peer.peer.get_addr()
        );

        Ok(())
    }

    // Watch the tracker server provided in the torrent file
    // and update the available peers for the peer manager to be able to manager them.
    //
    // Won't block, just spawn a task to run.
    pub async fn watch_tracker(
        self: Arc<Self>,
        interval: Duration,
        endpoint: String,
        info_hash: [u8; 20],
        file_size: usize,
    ) -> Result<()> {
        let available_peers = self.available_peers.clone();
        let peer_manager = self.clone();
        let mut interval = time::interval(interval);

        tokio::task::spawn(async move {
            loop {
                interval.tick().await;
                match peer_manager
                    .get_peers(endpoint.clone(), info_hash, file_size)
                    .await
                {
                    Ok(peers) => {
                        let mut hash_map = available_peers.write().await;
                        for peer in peers {
                            hash_map.insert(peer.get_addr(), peer);
                        }
                    }
                    Err(e) => {
                        debug!("[watch_tracker] failed to get peers: {:?}", e);
                    }
                }
            }
        });

        Ok(())
    }

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
                        debug!(
                            "[get_peers] Successfully connected to tracker after {} attempts ({} seconds)",
                            attempt,
                            elapsed.as_secs()
                        );
                    }
                    return Ok(peers);
                }
                Err(e) => {
                    let is_tracker_error = if let Some(app_err) = e.downcast_ref::<AppError>() {
                        matches!(app_err, AppError::TrackerRejected(_))
                    } else {
                        false
                    };

                    if is_tracker_error {
                        debug!("[get_peers] Tracker rejection (attempt {}): {}", attempt, e);
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
        // URL-encode the info_hash (percent-encoding, not hex)
        let info_hash_encoded: String = info_hash.iter().map(|b| format!("%{:02X}", b)).collect();

        let params = [
            ("info_hash", info_hash_encoded),
            ("port", "6881".to_string()),
            ("uploaded", "0".to_string()),
            ("peer_id", "postman-000000000001".to_string()),
            ("downloaded", "0".to_string()),
            ("left", file_size.to_string()),
            ("compact", "1".to_string()),
        ];

        let raw_query = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        let mut url = Url::parse(endpoint)?;
        url.set_query(Some(&raw_query));

        let req = self.http_client.get(url).build()?;

        let res = self.http_client.execute(req).await?;
        if !res.status().is_success() {
            return Err(AppError::tracker_rejected(format!(
                "Tracker returned status: {}",
                res.status()
            ))
            .into());
        }

        let body = res.bytes().await?;
        debug!("[try_get_peers] body: {}", String::from_utf8_lossy(&body));
        let (_, val) = self.decoder.from_bytes(&body)?;

        self.parse_peers(val)
    }

    fn parse_peers(&self, val: BencodeTypes) -> Result<Vec<Peer>> {
        match val {
            BencodeTypes::Dictionary(dict) => {
                if let Some(failure_reason) = dict.get("failure reason") {
                    let reason = match failure_reason {
                        BencodeTypes::String(s) => s.clone(),
                        _ => "Unknown failure reason".to_string(),
                    };
                    return Err(AppError::tracker_rejected(reason).into());
                }

                let peers_bencode = dict.get("peers").ok_or_else(|| {
                    anyhow::Error::from(AppError::missing_field(
                        "peers field not found in response",
                    ))
                })?;

                match peers_bencode {
                    BencodeTypes::List(peers_list) => {
                        let mut peers = Vec::new();
                        for peer_bencode in peers_list {
                            match peer_bencode {
                                BencodeTypes::Dictionary(peer_dict) => {
                                    let ip = peer_dict.get("ip").ok_or_else(|| {
                                        anyhow::Error::from(AppError::missing_field(
                                            "ip field not found in peer",
                                        ))
                                    })?;

                                    let port = peer_dict.get("port").ok_or_else(|| {
                                        anyhow::Error::from(AppError::missing_field(
                                            "port field not found in peer",
                                        ))
                                    })?;

                                    let ip_string = match ip {
                                        BencodeTypes::String(s) => s.clone(),
                                        _ => {
                                            return Err(AppError::InvalidFieldType {
                                                field: "ip".to_string(),
                                                expected: "string".to_string(),
                                            }
                                            .into());
                                        }
                                    };

                                    let port_u16 = match port {
                                        BencodeTypes::Integer(i) => {
                                            if *i < 0 || *i > u16::MAX as isize {
                                                return Err(AppError::invalid_bencode(
                                                    "port value out of range",
                                                )
                                                .into());
                                            }
                                            *i as u16
                                        }
                                        _ => {
                                            return Err(AppError::InvalidFieldType {
                                                field: "port".to_string(),
                                                expected: "integer".to_string(),
                                            }
                                            .into());
                                        }
                                    };

                                    peers.push(Peer::new(ip_string, port_u16));
                                }
                                _ => {
                                    return Err(AppError::InvalidFieldType {
                                        field: "peer".to_string(),
                                        expected: "dictionary".to_string(),
                                    }
                                    .into());
                                }
                            }
                        }
                        Ok(peers)
                    }
                    _ => Err(AppError::InvalidFieldType {
                        field: "peers".to_string(),
                        expected: "list".to_string(),
                    }
                    .into()),
                }
            }
            _ => Err(AppError::InvalidFieldType {
                field: "response".to_string(),
                expected: "dictionary".to_string(),
            }
            .into()),
        }
    }

    /// Connect to available peers up to the specified limit.
    ///
    /// This method attempts to establish connections with peers from the available peer pool
    /// until reaching the maximum number of concurrent peers. It's called automatically by
    /// the background orchestration task in `start()`, but can also be invoked manually
    /// when immediate peer connections are needed.
    ///
    /// # Arguments
    /// * `max_peers` - Maximum number of concurrent peer connections to maintain
    /// * `completion_tx` - Channel to send completed piece notifications
    /// * `failure_tx` - Channel to send failed piece notifications
    ///
    /// # Returns
    /// Returns `Ok(usize)` with the number of successfully connected peers, or an error if
    /// the operation fails critically.
    pub async fn connect_with_peers(
        &self,
        max_peers: usize,
        completion_tx: mpsc::Sender<CompletedPiece>,
        failure_tx: mpsc::Sender<FailedPiece>,
        disconnect_tx: mpsc::Sender<PeerDisconnected>,
    ) -> Result<usize> {
        let peers_to_connect = {
            let available = self.available_peers.read().await;
            let connected = self.connected_peers.read().await;
            let needed = max_peers.saturating_sub(connected.len());

            available
                .values()
                .filter(|p| !connected.contains_key(&p.get_addr()))
                .take(needed)
                .cloned()
                .collect::<Vec<_>>()
        };

        let mut successful_connections = 0;

        for peer in peers_to_connect {
            let peer_addr = peer.get_addr();

            match self
                .connect_peer(
                    peer,
                    completion_tx.clone(),
                    failure_tx.clone(),
                    disconnect_tx.clone(),
                )
                .await
            {
                Ok(connected_peer) => {
                    debug!(
                        "[PeerManager::connect_with_peers] Connected to peer {}",
                        peer_addr
                    );
                    self.connected_peers
                        .write()
                        .await
                        .insert(peer_addr, connected_peer);
                    successful_connections += 1;
                }
                Err(e) => {
                    debug!(
                        "[PeerManager::connect_with_peers] Failed to connect to peer {}: {}",
                        peer_addr, e
                    );
                }
            }
        }

        debug!(
            "[PeerManager::connect_with_peers] Connected {} new peers",
            successful_connections
        );
        Ok(successful_connections)
    }

    /// Request a piece download from available peers.
    ///
    /// This method queues a piece download request and attempts immediate assignment to
    /// an available peer. The download happens asynchronously - this method returns
    /// immediately and the completion is signaled via the `piece_completion_tx` channel
    /// provided during initialization.
    ///
    /// # Arguments
    /// * `request` - The piece download request containing piece index and metadata
    ///
    /// # Returns
    /// Returns `Ok(())` if the request was queued successfully, or an error if queuing fails.
    ///
    /// # Notes
    /// - The piece is first added to the pending queue
    /// - An immediate assignment attempt is made to an available peer
    /// - If no peer is available, the piece remains queued and will be assigned by
    ///   the background orchestration task when a peer becomes available
    #[allow(dead_code)]
    pub async fn download_piece(&self, request: PieceDownloadRequest) -> Result<()> {
        let piece_index = request.piece_index;

        // Add to pending queue
        {
            let mut pending = self.pending_pieces.write().await;
            pending.push_back(request.clone());
        }

        // Attempt immediate assignment
        if let Err(e) = self.assign_piece_to_peer(request).await {
            debug!(
                "[PeerManager::download_piece] Could not immediately assign piece {}: {}. \
                 Will retry via background task.",
                piece_index, e
            );
        } else {
            debug!(
                "[PeerManager::download_piece] Piece {} assigned immediately",
                piece_index
            );
        }

        Ok(())
    }
}
