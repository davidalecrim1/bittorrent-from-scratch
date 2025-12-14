#![allow(dead_code)]

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use crate::types::{
    CompletedPiece, ConnectedPeer, FailedPiece, Peer, PeerConnection, PeerManagerConfig,
    PieceDownloadRequest,
};
use crate::{encoding::Encoder, types::BencodeTypes};
use anyhow::anyhow;
use log::debug;
use reqwest::{Client, Url};
use tokio::sync::{RwLock, mpsc};
use tokio::time;

use crate::encoding::Decoder;
use anyhow::Result;

#[derive(Debug)]
pub struct PeerManager {
    http_client: Client,

    // Available to connect from the Tracker server.
    available_peers: Arc<RwLock<HashMap<String, Peer>>>,

    // Encoder and decoder are used to encode and decode
    // the data from a torrent file and some specific messages with tracker.
    decoder: Decoder,
    encoder: Encoder,

    // New fields for orchestration
    connected_peers: Arc<RwLock<HashMap<String, ConnectedPeer>>>,
    config: Option<PeerManagerConfig>,
    pending_pieces: Arc<RwLock<VecDeque<crate::types::PieceDownloadRequest>>>,
    in_flight_pieces: Arc<RwLock<HashMap<u32, String>>>,
    failed_attempts: Arc<RwLock<HashMap<u32, usize>>>,
}

impl PeerManager {
    pub fn new(decoder: Decoder, encoder: Encoder, http_client: Client) -> Self {
        Self {
            decoder,
            encoder,
            http_client,
            available_peers: Arc::new(RwLock::new(HashMap::new())),
            connected_peers: Arc::new(RwLock::new(HashMap::new())),
            config: None,
            pending_pieces: Arc::new(RwLock::new(VecDeque::new())),
            in_flight_pieces: Arc::new(RwLock::new(HashMap::new())),
            failed_attempts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn initialize(&mut self, config: PeerManagerConfig) -> Result<()> {
        self.config = Some(config);
        Ok(())
    }

    pub async fn start(
        self: Arc<Self>,
        announce_url: String,
        piece_completion_tx: mpsc::Sender<CompletedPiece>,
        piece_failure_tx: mpsc::Sender<FailedPiece>,
    ) -> Result<()> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| anyhow!("PeerManager not initialized"))?;

        let info_hash = config.info_hash;
        let file_size = config.file_size;
        let max_peers = config.max_peers;

        let peer_manager = self.clone();
        tokio::task::spawn(async move {
            loop {
                let peers_to_connect = {
                    let available = peer_manager.available_peers.read().await;
                    let connected = peer_manager.connected_peers.read().await;
                    let needed = max_peers.saturating_sub(connected.len());

                    available
                        .values()
                        .filter(|p| !connected.contains_key(&p.get_addr()))
                        .take(needed)
                        .cloned()
                        .collect::<Vec<_>>()
                };

                for peer in peers_to_connect {
                    let peer_manager_clone = peer_manager.clone();
                    let completion_tx_clone = piece_completion_tx.clone();
                    let failure_tx_clone = piece_failure_tx.clone();
                    let peer_addr = peer.get_addr();

                    match peer_manager_clone
                        .connect_peer(peer, completion_tx_clone, failure_tx_clone)
                        .await
                    {
                        Ok(connected_peer) => {
                            debug!("[PeerManager::start] Connected to peer {}", peer_addr);
                            peer_manager
                                .connected_peers
                                .write()
                                .await
                                .insert(peer_addr, connected_peer);
                        }
                        Err(e) => {
                            debug!(
                                "[PeerManager::start] Failed to connect to peer {}: {}",
                                peer_addr, e
                            );
                        }
                    }
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
                    if let Err(e) = peer_manager_assignment
                        .assign_piece_to_peer(request.clone())
                        .await
                    {
                        debug!(
                            "[PeerManager] Failed to assign piece {}: {}",
                            request.piece_index, e
                        );
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
    ) -> Result<ConnectedPeer> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| anyhow!("PeerManager not initialized"))?;

        let (mut peer_conn, download_request_tx) =
            PeerConnection::new(peer.clone(), completion_tx, failure_tx);

        peer_conn
            .handshake(config.client_peer_id, config.info_hash)
            .await?;

        let peer_id = peer_conn
            .get_peer_id()
            .ok_or_else(|| anyhow!("Failed to get peer ID after handshake"))?;

        let bitfield = peer_conn.get_bitfield().to_vec();

        peer_conn.start(config.num_pieces).await?;

        Ok(ConnectedPeer {
            peer: peer.clone(),
            peer_id,
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
            .ok_or_else(|| anyhow!("No available peer has piece {}", piece_index))?;

        best_peer
            .download_request_tx
            .send(request)
            .await
            .map_err(|e| anyhow!("Failed to send download request: {}", e))?;

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
        let mut last_error = None;

        loop {
            attempt += 1;
            let elapsed = start_time.elapsed();

            if elapsed >= max_duration {
                return Err(anyhow!(
                    "Failed to get peers from tracker after {} attempts over {} seconds: {}",
                    attempt - 1,
                    elapsed.as_secs(),
                    last_error.unwrap_or_else(|| anyhow!("Unknown error"))
                ));
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
                    let error_msg = e.to_string();

                    if error_msg.contains("Tracker rejected request") {
                        debug!(
                            "[get_peers] Tracker rejection (attempt {}): {}",
                            attempt, error_msg
                        );
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
                        return Err(anyhow!(
                            "Failed to get peers from tracker after {} attempts over {} seconds: {}",
                            attempt,
                            elapsed.as_secs(),
                            last_error.unwrap()
                        ));
                    }

                    debug!(
                        "[get_peers] Attempt {} failed (elapsed: {}s), retrying in {}s: {}",
                        attempt,
                        elapsed.as_secs(),
                        delay.as_secs_f32(),
                        error_msg
                    );

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

        let res = self.http_client.execute(req).await;
        let val: BencodeTypes = match res {
            Ok(res) => {
                if !res.status().is_success() {
                    return Err(anyhow!(
                        "the request to the tracker server failed, status: {:?}",
                        res.status()
                    ));
                }
                let body = res.bytes().await?;
                debug!("[try_get_peers] body: {}", String::from_utf8_lossy(&body));
                let (_, val) = self.decoder.from_bytes(&body)?;
                val
            }
            Err(e) => {
                return Err(anyhow!("the request to the tracker server failed: {}", e));
            }
        };

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
                    return Err(anyhow!("Tracker rejected request: {}", reason));
                }

                let peers_bencode = dict
                    .get("peers")
                    .ok_or(anyhow!("peers field not found in response"))?;

                match peers_bencode {
                    BencodeTypes::List(peers_list) => {
                        let mut peers = Vec::new();
                        for peer_bencode in peers_list {
                            match peer_bencode {
                                BencodeTypes::Dictionary(peer_dict) => {
                                    let ip = peer_dict
                                        .get("ip")
                                        .ok_or(anyhow!("ip field not found in peer"))?;

                                    let port = peer_dict
                                        .get("port")
                                        .ok_or(anyhow!("port field not found in peer"))?;

                                    let ip_string = match ip {
                                        BencodeTypes::String(s) => s.clone(),
                                        _ => return Err(anyhow!("ip field is not a string")),
                                    };

                                    let port_u16 = match port {
                                        BencodeTypes::Integer(i) => {
                                            if *i < 0 || *i > u16::MAX as isize {
                                                return Err(anyhow!("port value out of range"));
                                            }
                                            *i as u16
                                        }
                                        _ => return Err(anyhow!("port field is not an integer")),
                                    };

                                    peers.push(Peer::new(ip_string, port_u16));
                                }
                                _ => return Err(anyhow!("peer is not a dictionary")),
                            }
                        }
                        Ok(peers)
                    }
                    _ => Err(anyhow!("peers field is not a list")),
                }
            }
            _ => Err(anyhow!("response is not a dictionary")),
        }
    }

    // TODO: Implement connect_with_peers() - connect to available peers up to limit
    // Create a channel to send and receive messages from the peer in a high level way
    // Let the peer connection store the bitfield, download the blocks for each piece etc.
    // Will need channels for PeerConnection to read/write.
    // For the peer manager to request and receive pieces once they are downloaded.
    // The Bitfield should be accessible whenever needed without using messages, just checking the struct.

    // TODO: Implement download_piece() - request a piece from available peers
    // receive the piece idx
    // seek if there are ready peers for the piece
    // when found, download the blocks of the piece
    // callback it to the file manager to be stored and updated
}
