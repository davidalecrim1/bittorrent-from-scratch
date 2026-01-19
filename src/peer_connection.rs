use anyhow::{Result, anyhow};
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{RwLock, mpsc};
use tokio::time::sleep;

use crate::bandwidth_limiter::BandwidthLimiter;
use crate::download_state::DownloadState;
use crate::error::AppError;
use crate::io::{MessageIO, ThrottledMessageIO};
use crate::peer_messages::{self, PeerHandshakeMessage};
use crate::peer_messages::{
    BitfieldMessage, InterestedMessage, PeerMessage, PieceMessage, UnchokeMessage,
};
use crate::piece_manager::PieceManager;
use crate::types::{
    FailedPiece, MAX_PIECES_PER_PEER, Peer, PeerDisconnected, PeerEvent, PieceDownloadRequest,
    StorePieceRequest,
};

const DEFAULT_BLOCK_SIZE: usize = 16 * 1024; // 16 KiB per BitTorrent spec

// Limits the amount of concurrent pieces blocks (chunks) that can be
// downloaded concurrently.
const SEMAPHORE_BLOCK_CONCURRENCY: usize = 25;

const MAX_RETRIES_HANDSHAKE: usize = 3;

pub struct PieceDownloadTask {
    pub piece_index: u32,
    pub download_state: DownloadState,
    pub block_semaphore: Arc<tokio::sync::Semaphore>,
    pub outbound_tx: mpsc::UnboundedSender<PeerMessage>,
    pub inbound_rx: mpsc::UnboundedReceiver<PieceMessage>,
    pub event_tx: mpsc::UnboundedSender<PeerEvent>,
    pub peer_addr: String,
}

impl PieceDownloadTask {
    pub async fn run(mut self) -> Result<()> {
        debug!(
            "Piece {}: Starting download task for peer {}",
            self.piece_index, self.peer_addr
        );

        loop {
            if let Some((begin, length)) = self.download_state.get_next_block_to_request() {
                let _permit = self.block_semaphore.acquire().await?;

                debug!(
                    "Piece {}: Requesting block at offset {} (length {}) from peer {} (available permits: {})",
                    self.piece_index,
                    begin,
                    length,
                    self.peer_addr,
                    self.block_semaphore.available_permits()
                );

                let request = peer_messages::RequestMessage {
                    piece_index: self.piece_index,
                    begin,
                    length,
                };

                self.outbound_tx.send(PeerMessage::Request(request))?;

                let piece_msg = match self.inbound_rx.recv().await {
                    Some(msg) => msg,
                    None => {
                        debug!(
                            "Piece {}: Channel closed, peer disconnected",
                            self.piece_index
                        );
                        self.send_failure(AppError::PeerStreamClosed).await?;
                        return Ok(());
                    }
                };

                self.download_state
                    .add_block(piece_msg.begin, piece_msg.block)?;

                if self.download_state.is_complete() {
                    self.finalize_piece().await?;
                    return Ok(());
                }
            } else {
                if self.download_state.is_complete() {
                    self.finalize_piece().await?;
                    return Ok(());
                }

                let piece_msg = match self.inbound_rx.recv().await {
                    Some(msg) => msg,
                    None => {
                        debug!(
                            "Piece {}: Channel closed, peer disconnected",
                            self.piece_index
                        );
                        self.send_failure(AppError::PeerStreamClosed).await?;
                        return Ok(());
                    }
                };

                self.download_state
                    .add_block(piece_msg.begin, piece_msg.block)?;
            }
        }
    }

    async fn finalize_piece(&self) -> Result<()> {
        let piece_data = self.download_state.assemble_piece()?;

        if self.download_state.verify_hash(&piece_data)? {
            info!(
                "Piece {} completed from peer {} (size: {} bytes)",
                self.piece_index,
                self.peer_addr,
                piece_data.len()
            );
            self.event_tx
                .send(PeerEvent::StorePiece(StorePieceRequest {
                    piece_index: self.piece_index,
                    data: piece_data,
                    peer_addr: self.peer_addr.clone(),
                }))?;
        } else {
            debug!("Piece {}: Hash mismatch, sending failure", self.piece_index);
            self.send_failure(AppError::HashMismatch).await?;
        }

        Ok(())
    }

    async fn send_failure(&self, error: AppError) -> Result<()> {
        self.event_tx.send(PeerEvent::Failure(FailedPiece {
            piece_index: self.piece_index,
            peer_addr: self.peer_addr.clone(),
            error,
            push_front: false,
        }))?;
        Ok(())
    }
}

pub struct PeerConnection {
    // Peer basic information
    peer: Peer,

    // Stream is only present after handshake
    stream: Option<TcpStream>,

    // TCP connector for establishing connections
    tcp_connector: Arc<dyn crate::tcp_connector::TcpStreamFactory>,

    // Outbound messages the actor will write to the wire
    outbound_tx: mpsc::UnboundedSender<PeerMessage>,
    outbound_rx: Option<mpsc::UnboundedReceiver<PeerMessage>>,

    // Inbound messages read from the wire and sent out
    inbound_tx: mpsc::UnboundedSender<PeerMessage>,
    inbound_rx: Option<mpsc::UnboundedReceiver<PeerMessage>>,

    // Receives broadcast messages from PeerManager to send to all peers simultaneously.
    // Used for efficiently broadcasting Have messages when we complete pieces.
    broadcast_rx: Option<tokio::sync::broadcast::Receiver<PeerMessage>>,

    // Piece download channels
    download_request_rx: Option<mpsc::UnboundedReceiver<PieceDownloadRequest>>,

    // Unified event channel with Peer Manager
    event_tx: mpsc::UnboundedSender<PeerEvent>,

    // Peer Status
    is_choking: Arc<AtomicBool>,
    // Means if the peer is interested in us to request pieces to be uploaded to them.
    is_interested: bool,

    // Peer Information (shared with PeerManager) - tracks which pieces the peer has
    bitfield: Arc<RwLock<HashSet<u32>>>,

    // Multi-piece download state
    active_downloads: HashMap<u32, tokio::task::JoinHandle<()>>,
    download_queue: VecDeque<PieceDownloadRequest>,
    max_concurrent_pieces: usize,

    // Senders for delivering received blocks to their respective piece download tasks
    piece_block_txs: HashMap<u32, mpsc::UnboundedSender<peer_messages::PieceMessage>>,

    // Manager for reading and writing pieces to disk
    piece_manager: Arc<dyn PieceManager>,

    // Limiter for the upload/download from the client.
    bandwidth_limiter: Option<BandwidthLimiter>,

    // Statistics about upload/download with peers.
    bandwidth_stats: Arc<crate::BandwidthStats>,

    // Block semaphore to limit the concurrent downloads of blocks (chunks of pieces) at the same time.
    block_semaphore: Arc<tokio::sync::Semaphore>,
}

// Creates a connection with the Peer provided and wraps it within
// It will rely on the inbound_tx and outbound_rx channels to communicate with the peer connection.
impl PeerConnection {
    #[allow(clippy::type_complexity)]
    pub fn new(
        peer: Peer,
        event_tx: mpsc::UnboundedSender<PeerEvent>,
        tcp_connector: Arc<dyn crate::tcp_connector::TcpStreamFactory>,
        piece_manager: Arc<dyn PieceManager>,
        bandwidth_limiter: Option<BandwidthLimiter>,
        bandwidth_stats: Arc<crate::BandwidthStats>,
        broadcast_rx: tokio::sync::broadcast::Receiver<PeerMessage>,
    ) -> (
        Self,
        mpsc::UnboundedSender<PieceDownloadRequest>,
        Arc<RwLock<HashSet<u32>>>,
        mpsc::UnboundedSender<PeerMessage>,
    ) {
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let (download_request_tx, download_request_rx) = mpsc::unbounded_channel();

        let bitfield = Arc::new(RwLock::new(HashSet::new()));
        let block_semaphore = Arc::new(tokio::sync::Semaphore::new(SEMAPHORE_BLOCK_CONCURRENCY));
        let outbound_tx_clone = outbound_tx.clone();
        let is_choking = Arc::new(AtomicBool::new(false));

        let peer_conn = Self {
            peer,
            stream: None,
            tcp_connector,
            outbound_tx,
            outbound_rx: Some(outbound_rx),
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            broadcast_rx: Some(broadcast_rx),
            download_request_rx: Some(download_request_rx),
            event_tx,
            is_choking,
            is_interested: false,
            bitfield: bitfield.clone(),
            active_downloads: HashMap::new(),
            download_queue: VecDeque::new(),
            max_concurrent_pieces: MAX_PIECES_PER_PEER,
            block_semaphore,
            piece_block_txs: HashMap::new(),
            piece_manager,
            bandwidth_limiter,
            bandwidth_stats,
        };

        (peer_conn, download_request_tx, bitfield, outbound_tx_clone)
    }

    // Handshake with the peer to initialize the connection.
    pub async fn handshake(
        &mut self,
        client_peer_id: [u8; 20],
        info_hash: [u8; 20],
    ) -> Result<PeerHandshakeMessage> {
        let handshake = PeerHandshakeMessage::new(info_hash, client_peer_id);
        let bytes = handshake.to_bytes();

        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES_HANDSHAKE {
            match self.try_handshake(&bytes).await {
                Ok(response) => {
                    return Ok(response);
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_RETRIES_HANDSHAKE {
                        let delay = Duration::from_millis(200 * 2_u64.pow(attempt as u32 - 1)); // 200ms, 400ms, 800ms ...
                        sleep(delay).await;
                    }
                }
            }
        }

        Err(anyhow!(
            "Failed to handshake with peer {} after {} attempts: {}",
            self.peer.get_addr(),
            MAX_RETRIES_HANDSHAKE,
            last_error.unwrap()
        ))
    }

    // Attemps to perform the handshake. This can be unstable and may need retries.
    async fn try_handshake(&mut self, handshake_bytes: &[u8]) -> Result<PeerHandshakeMessage> {
        let mut stream = self.tcp_connector.connect(self.peer.get_addr()).await?;
        stream.write_all(handshake_bytes).await?;

        let mut buffer = vec![0u8; 68];
        let n = stream.read(&mut buffer).await?;

        let response = PeerHandshakeMessage::from_bytes(&buffer[..n])?;
        self.stream = Some(stream);

        Ok(response)
    }

    // Starts the peer connection after the handshake
    // wrapping the TCP connection in the MessageIO trait.
    pub async fn start(mut self, num_pieces: usize) -> Result<()> {
        let stream = self
            .stream
            .take()
            .ok_or_else(|| anyhow!("handshake not done"))?;

        debug!(
            "Starting peer connection for {} with num_pieces={}",
            self.peer.get_addr(),
            num_pieces
        );

        let tcp_message_io = crate::io::TcpMessageIO::from_stream(stream, num_pieces);

        let message_io: Box<dyn crate::io::MessageIO> =
            if let Some(ref limiter) = self.bandwidth_limiter {
                Box::new(ThrottledMessageIO::new(
                    Box::new(tcp_message_io),
                    limiter.download.clone(),
                    limiter.upload.clone(),
                    Some(self.bandwidth_stats.clone()),
                ))
            } else {
                Box::new(ThrottledMessageIO::new(
                    Box::new(tcp_message_io),
                    None,
                    None,
                    Some(self.bandwidth_stats.clone()),
                ))
            };

        self.start_with_io(message_io).await
    }

    // Starts with a custom MessageIO implementation.
    // Usually done for testing purposes.
    pub async fn start_with_io(
        mut self,
        mut message_io: Box<dyn crate::io::MessageIO>,
    ) -> Result<()> {
        // Take the receivers out of self
        let mut outbound_rx = self.outbound_rx.take().unwrap();
        let inbound_tx = self.inbound_tx.clone();
        let event_tx = self.event_tx.clone();
        let peer = self.peer.clone();

        // Spawn single I/O task that handles both reading and writing
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Handle outbound messages (write to network)
                    Some(msg) = outbound_rx.recv() => {
                        if let Err(e) = message_io.write_message(&msg).await {
                            let _ = event_tx.send(PeerEvent::Disconnect(PeerDisconnected {
                                peer: peer.clone(),
                                error: AppError::PeerWriteError(e.to_string()),
                            }));
                            break;
                        }
                    }

                    // Handle inbound messages (read from network)
                    result = message_io.read_message() => {
                        match result {
                            Ok(Some(msg)) => {
                                if inbound_tx.send(msg).is_err() {
                                    // Channel closed, stop I/O task
                                    break;
                                }
                            }
                            Ok(None) => {
                                // Stream ended gracefully
                                let _ = event_tx.send(PeerEvent::Disconnect(PeerDisconnected {
                                    peer: peer.clone(),
                                    error: AppError::PeerStreamClosed,
                                }));
                                break;
                            }
                            Err(e) => {
                                let _ = event_tx.send(PeerEvent::Disconnect(PeerDisconnected {
                                    peer: peer.clone(),
                                    error: AppError::PeerReadError(e.to_string()),
                                }));
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Clone outbound_tx before handle_incoming_messages consumes self
        let outbound_tx = self.outbound_tx.clone();

        // Get our bitfield before handle_incoming_messages consumes self
        let our_bitfield = self.get_our_bitfield().await;

        // Spawn message handler (ensures it's ready before we trigger responses)
        self.handle_incoming_messages();

        // Send bitfield if we have any pieces (bandwidth saving: don't send empty bitfield)
        if let Some(bitfield) = our_bitfield {
            let _ = outbound_tx.send(PeerMessage::Bitfield(bitfield));
        }

        // Send interested message (peer will respond with bitfield)
        let _ = outbound_tx.send(PeerMessage::Interested(InterestedMessage {}));

        Ok(())
    }

    fn handle_incoming_messages(mut self) {
        let mut inbound_rx = self.inbound_rx.take().unwrap();
        let mut broadcast_rx = self.broadcast_rx.take().unwrap();
        let mut download_request_rx = self.download_request_rx.take().unwrap();
        let outbound_tx = self.outbound_tx.clone();
        let event_tx = self.event_tx.clone();
        let peer = self.peer.clone();
        let peer_addr = peer.get_addr();

        tokio::task::spawn(async move {
            let mut cleanup_interval = tokio::time::interval(Duration::from_millis(50));
            cleanup_interval.tick().await;

            loop {
                tokio::select! {
                    // Use biased to ensure fair scheduling between branches
                    biased;

                    _ = cleanup_interval.tick() => {
                        self.cleanup_completed_tasks().await;
                    }

                    result = broadcast_rx.recv() => {
                        match result {
                            Ok(msg) => {
                                // Forward broadcast message to outbound channel
                                if let Err(e) = outbound_tx.send(msg) {
                                    debug!("Failed to send broadcast message to peer {}: {}", peer_addr, e);
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                debug!("Peer {} lagged and missed {} broadcast messages", peer_addr, skipped);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                debug!("Broadcast channel closed for peer {}", peer_addr);
                            }
                        }
                    }

                    result = inbound_rx.recv() => {
                        match result {
                            Some(msg) => {
                                if let Err(_e) = self.handle_peer_message(peer_addr.clone(), msg).await {
                                    break;
                                }
                            }
                            None => {
                                // Reader channel closed (peer disconnected)
                                let _ = event_tx.send(PeerEvent::Disconnect(PeerDisconnected {
                                    peer: peer.clone(),
                                    error: AppError::PeerInboundChannelClosed,
                                }));
                                break;
                            }
                        }
                    }
                    result = download_request_rx.recv() => {
                        match result {
                            Some(request) => {
                                debug!("Peer Connector received a download request for piece {}, sending to Peer {}", request.piece_index,peer_addr.clone());

                                let piece_index = request.piece_index;
                                if let Err(e) = self.start_piece_download(peer_addr.clone(), request).await {
                                    // TODO: Check how to better handle this in the future. Either by queueing or assigning better.
                                    debug!("Peer Connector failed to start piece {} download from Peer {}, with error: {}", piece_index, peer_addr.clone(), e);
                                    let _ = event_tx.send(PeerEvent::Failure(FailedPiece {
                                        piece_index,
                                        peer_addr: peer_addr.clone(),
                                        error: AppError::Other(e),
                                        push_front: false,
                                    }));
                                }
                            }
                            None => {
                                // Download request channel closed (shouldn't happen during normal operation)
                                debug!("Peer Connector download_request channel closed unexpectedly from Peer {}", peer_addr);
                                let _ = event_tx.send(PeerEvent::Disconnect(PeerDisconnected {
                                    peer: peer.clone(),
                                    error: AppError::PeerDownloadRequestChannelClosed,
                                }));
                                break;
                            }
                        }
                    }
                }
            }

            self.shutdown_piece_tasks().await;
        });
    }

    async fn handle_peer_message(&mut self, peer_addr: String, msg: PeerMessage) -> Result<()> {
        match msg {
            PeerMessage::KeepAlive => {
                debug!(
                    // TODO: Check later on how to support this.
                    "Peer Connector received a Keep Alive message from Peer {} - (ignored - keep alive not supported)",
                    peer_addr
                );
            }

            PeerMessage::Choke(_) => {
                let was_choking = self.is_choking.swap(true, Ordering::Relaxed);
                if !was_choking {
                    info!("Peer {} is now choking us", peer_addr);
                    let _ = self.event_tx.send(PeerEvent::PeerChoked(peer_addr.clone()));
                }
            }

            PeerMessage::Bitfield(bitfield) => {
                // Convert Vec<bool> bitfield to HashSet of piece indices
                let piece_indices: HashSet<u32> = bitfield
                    .bitfield
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, &has)| if has { Some(idx as u32) } else { None })
                    .collect();

                let num_pieces_available = piece_indices.len();
                debug!(
                    "Peer Connector received Bitfield with {}/{} pieces available from Peer {}",
                    num_pieces_available,
                    bitfield.bitfield.len(),
                    peer_addr,
                );

                let mut bf = self.bitfield.write().await;
                *bf = piece_indices;
            }

            PeerMessage::Unchoke(_) => {
                let was_choking = self.is_choking.swap(false, Ordering::Relaxed);
                if was_choking {
                    info!("Peer {} unchoked us", peer_addr);
                    let _ = self
                        .event_tx
                        .send(PeerEvent::PeerUnchoked(peer_addr.clone()));
                }
            }

            PeerMessage::Interested(_) => {
                debug!("Peer Connector received Interested from Peer {}", peer_addr);
                self.is_interested = true;
                let _ = self
                    .outbound_tx
                    .send(PeerMessage::Unchoke(UnchokeMessage {}));
            }

            PeerMessage::NotInterested(_) => {
                self.is_interested = false;
                debug!(
                    "Peer Connector received NotInterested from Peer {}",
                    peer_addr
                );
            }

            PeerMessage::Have(have) => {
                debug!(
                    "Peer Connector received Have (now has piece {}) from Peer {}",
                    have.piece_index, peer_addr,
                );
                let mut bf = self.bitfield.write().await;
                bf.insert(have.piece_index);
            }

            PeerMessage::Request(request) => {
                debug!(
                    "Peer Connector received Request for piece {} (begin={}, length={}) from Peer {}",
                    request.piece_index, request.begin, request.length, peer_addr
                );

                let piece_manager = self.piece_manager.clone();
                let outbound_tx = self.outbound_tx.clone();
                let peer_addr_clone = peer_addr.clone();

                tokio::spawn(async move {
                    match piece_manager
                        .read_block(request.piece_index, request.begin, request.length)
                        .await
                    {
                        Ok(Some(block)) => {
                            let piece_msg = PieceMessage {
                                piece_index: request.piece_index,
                                begin: request.begin,
                                block,
                            };

                            if let Err(e) = outbound_tx.send(PeerMessage::Piece(piece_msg)) {
                                debug!("Failed to send piece to peer {}: {}", peer_addr_clone, e);
                            } else {
                                debug!(
                                    "Uploaded block for piece {} (begin={}, length={}) to peer {}",
                                    request.piece_index,
                                    request.begin,
                                    request.length,
                                    peer_addr_clone
                                );
                            }
                        }
                        Ok(None) => {
                            debug!(
                                "Peer {} requested piece {} but we don't have it",
                                peer_addr_clone, request.piece_index
                            );
                        }
                        Err(e) => {
                            debug!(
                                "Failed to read piece {} for upload to peer {}: {}",
                                request.piece_index, peer_addr_clone, e
                            );
                        }
                    }
                });
            }

            PeerMessage::Piece(piece) => {
                let piece_index = piece.piece_index;
                debug!(
                    "Peer Connector received Piece for index {}, begin {} from Peer {}",
                    piece_index, piece.begin, peer_addr
                );

                if let Some(tx) = self.piece_block_txs.get(&piece_index) {
                    if let Err(e) = tx.send(piece) {
                        debug!("Failed to route piece {} block to task: {}", piece_index, e);
                    }
                } else {
                    debug!(
                        "Received block for unknown piece {} from peer {}",
                        piece_index, peer_addr
                    );
                }
            }

            PeerMessage::Cancel(cancel) => {
                // TODO: Check how to support this.
                debug!(
                    "Peer Connector received Cancel for piece {} from Peer {} (ignored - upload not supported)",
                    cancel.piece_index, peer_addr,
                );
            }

            PeerMessage::Extended { .. } => {
                debug!(
                    "Peer Connector received Extended message from Peer {} (ignored - only used during metadata fetch)",
                    peer_addr
                );
            }
        }

        Ok(())
    }

    // Spawns a download task for a piece, creating an unbounded channel to receive blocks
    // and tracking the task handle for cleanup on completion or disconnection.
    fn activate_piece_download(&mut self, request: PieceDownloadRequest) {
        let piece_index = request.piece_index;
        let download_state = DownloadState::new(
            request.piece_index,
            request.piece_length,
            DEFAULT_BLOCK_SIZE,
            request.expected_hash,
        );

        let (piece_tx, piece_rx) = mpsc::unbounded_channel::<PieceMessage>();
        self.piece_block_txs.insert(piece_index, piece_tx);

        let task = PieceDownloadTask {
            piece_index,
            download_state,
            block_semaphore: self.block_semaphore.clone(),
            outbound_tx: self.outbound_tx.clone(),
            inbound_rx: piece_rx,
            event_tx: self.event_tx.clone(),
            peer_addr: self.peer.get_addr(),
        };

        let peer_addr = self.peer.get_addr().clone();

        let handle = tokio::spawn(async move {
            if let Err(e) = task.run().await {
                error!(
                    "Peer {} faied to download Piece {} on task: {}",
                    peer_addr, piece_index, e
                );
            }
        });

        self.active_downloads.insert(piece_index, handle);

        debug!(
            "Peer {}: Activated piece {} from queue (active: {}, queued: {})",
            self.peer.get_addr(),
            piece_index,
            self.active_downloads.len(),
            self.download_queue.len()
        );
    }

    // Starts the download of a piece from the Peer Manager.
    async fn start_piece_download(
        &mut self,
        _peer_addr: String,
        request: PieceDownloadRequest,
    ) -> Result<()> {
        let piece_index = request.piece_index;

        if self.active_downloads.contains_key(&piece_index)
            || self
                .download_queue
                .iter()
                .any(|r| r.piece_index == piece_index)
        {
            self.event_tx.send(PeerEvent::Failure(FailedPiece {
                piece_index,
                peer_addr: self.peer.get_addr(),
                error: AppError::PieceAlreadyDownloading,
                push_front: false,
            }))?;
            return Ok(());
        }

        let total = self.active_downloads.len() + self.download_queue.len();
        if total >= self.max_concurrent_pieces {
            debug!(
                "Peer {}: Queue full, rejecting piece {} (active: {}, queued: {}, max: {})",
                self.peer.get_addr(),
                piece_index,
                self.active_downloads.len(),
                self.download_queue.len(),
                self.max_concurrent_pieces
            );

            self.event_tx.send(PeerEvent::Failure(FailedPiece {
                piece_index,
                peer_addr: self.peer.get_addr(),
                error: AppError::PeerQueueFull,
                push_front: true,
            }))?;
            return Ok(());
        }

        if !self.has_piece(piece_index).await {
            self.event_tx.send(PeerEvent::Failure(FailedPiece {
                piece_index,
                peer_addr: self.peer.get_addr(),
                error: AppError::PeerDoesNotHavePiece,
                push_front: false,
            }))?;
            return Ok(());
        }

        if !self.can_download_pieces().await {
            self.event_tx.send(PeerEvent::Failure(FailedPiece {
                piece_index,
                peer_addr: self.peer.get_addr(),
                error: AppError::PeerNotReady {
                    choking: self.is_choking.load(Ordering::Relaxed),
                    bitfield_empty: self.bitfield.read().await.is_empty(),
                },
                push_front: false,
            }))?;
            return Ok(());
        }

        if self.active_downloads.len() < self.max_concurrent_pieces {
            debug!(
                "Peer {}: Added piece {} to active downloads (active: {}, queued: {}, total: {})",
                self.peer.get_addr(),
                piece_index,
                self.active_downloads.len() + 1,
                self.download_queue.len(),
                self.active_downloads.len() + self.download_queue.len() + 1
            );
            self.activate_piece_download(request);
        } else {
            debug!(
                "Peer {}: Added piece {} to queue (active: {}, queued: {}, total: {})",
                self.peer.get_addr(),
                piece_index,
                self.active_downloads.len(),
                self.download_queue.len() + 1,
                self.active_downloads.len() + self.download_queue.len() + 1
            );
            self.download_queue.push_back(request);
        }

        Ok(())
    }

    async fn cleanup_completed_tasks(&mut self) {
        let mut completed_pieces = Vec::new();

        self.active_downloads.retain(|&piece_index, handle| {
            if handle.is_finished() {
                completed_pieces.push(piece_index);
                false
            } else {
                true
            }
        });

        for piece_index in completed_pieces {
            self.piece_block_txs.remove(&piece_index);
            debug!(
                "Peer {}: Cleaned up completed task for piece {}",
                self.peer.get_addr(),
                piece_index
            );

            if let Some(next_request) = self.download_queue.pop_front() {
                self.activate_piece_download(next_request);
            }
        }
    }

    async fn shutdown_piece_tasks(&mut self) {
        debug!(
            "Peer {}: Shutting down {} active piece tasks",
            self.peer.get_addr(),
            self.active_downloads.len()
        );

        for (piece_index, tx) in self.piece_block_txs.drain() {
            drop(tx);
            debug!(
                "Peer {}: Closed channel for piece {}",
                self.peer.get_addr(),
                piece_index
            );
        }

        for (piece_index, handle) in self.active_downloads.drain() {
            match tokio::time::timeout(Duration::from_secs(2), handle).await {
                Ok(Ok(())) => debug!("Piece {} task exited cleanly", piece_index),
                Ok(Err(e)) => error!("Piece {} task panicked: {:?}", piece_index, e),
                Err(_) => error!("Piece {} task did not exit within timeout", piece_index),
            }
        }

        self.download_queue.clear();
        debug!(
            "Peer {}: All piece tasks shutdown complete",
            self.peer.get_addr()
        );
    }

    pub async fn has_piece(&self, piece_index: u32) -> bool {
        let bf = self.bitfield.read().await;
        bf.contains(&piece_index)
    }

    pub async fn can_download_pieces(&self) -> bool {
        let bf = self.bitfield.read().await;
        !self.is_choking.load(Ordering::Relaxed) && !bf.is_empty()
    }

    async fn get_our_bitfield(&self) -> Option<BitfieldMessage> {
        self.piece_manager
            .get_bitfield()
            .await
            .map(|bitfield| BitfieldMessage { bitfield })
    }

    /// Fetch torrent metadata from peer using BEP 9 extension protocol
    pub async fn fetch_metadata(
        info_hash: [u8; 20],
        peer_addr: std::net::SocketAddr,
        peer_id: [u8; 20],
        tcp_factory: Arc<dyn crate::tcp_connector::TcpStreamFactory>,
    ) -> Result<std::collections::BTreeMap<String, crate::types::BencodeTypes>> {
        use crate::encoding::Decoder;
        use crate::peer_messages::{
            create_extension_handshake, create_metadata_request, parse_extension_handshake,
            parse_metadata_data,
        };
        use crate::types::BencodeTypes;

        info!("Fetching metadata from peer {}", peer_addr);

        let handshake_msg = PeerHandshakeMessage::new_with_extensions(info_hash, peer_id, true);

        debug!("Connecting to peer {}", peer_addr);
        let mut tcp_stream = tcp_factory.connect(peer_addr.to_string()).await?;

        debug!("Sending handshake to peer {}", peer_addr);
        tcp_stream.write_all(&handshake_msg.to_bytes()).await?;

        debug!("Reading handshake response from peer {}", peer_addr);
        let mut buf = [0u8; 68];
        tcp_stream.read_exact(&mut buf).await?;
        let peer_handshake = PeerHandshakeMessage::from_bytes(&buf)?;

        if !peer_handshake.supports_extensions() {
            warn!("Peer {} does not support extensions", peer_addr);
            return Err(anyhow!("Peer does not support extensions"));
        }
        debug!("Peer {} supports extensions", peer_addr);

        let tcp_message_io = crate::io::TcpMessageIO::from_stream(tcp_stream, 0);
        let mut message_io = ThrottledMessageIO::new(Box::new(tcp_message_io), None, None, None);

        debug!("Sending extension handshake to peer {}", peer_addr);
        message_io
            .write_message(&create_extension_handshake())
            .await?;

        debug!(
            "Reading extension handshake response from peer {}",
            peer_addr
        );
        let ext_handshake_msg = message_io
            .read_message()
            .await?
            .ok_or_else(|| anyhow!("Connection closed before extension handshake"))?;
        let payload = match ext_handshake_msg {
            PeerMessage::Extended {
                extension_id: 0,
                payload,
            } => payload,
            other => {
                warn!(
                    "Expected extension handshake from {}, got {:?}",
                    peer_addr, other
                );
                return Err(anyhow!("Expected extension handshake"));
            }
        };

        debug!("Parsing extension handshake from peer {}", peer_addr);
        let extensions = parse_extension_handshake(&payload)?;
        debug!("Extensions supported by {}: {:?}", peer_addr, extensions);

        let ut_metadata_id = extensions.get("ut_metadata").ok_or_else(|| {
            warn!("Peer {} does not support ut_metadata", peer_addr);
            anyhow!("Peer does not support ut_metadata")
        })?;
        debug!(
            "Peer {} ut_metadata extension ID: {}",
            peer_addr, ut_metadata_id
        );

        debug!("Requesting metadata piece 0 from peer {}", peer_addr);
        message_io
            .write_message(&create_metadata_request(*ut_metadata_id as u8, 0))
            .await?;

        // Helper function to read metadata response, skipping other messages
        async fn read_metadata_response<T: MessageIO>(
            message_io: &mut T,
            peer_addr: SocketAddr,
        ) -> Result<Vec<u8>> {
            // NOTE: We check for OUR extension ID (1), not the peer's ID
            // because the peer uses OUR ID when sending messages TO us
            const OUR_UT_METADATA_ID: u8 = 1;
            let metadata_timeout = std::time::Duration::from_secs(10);

            let result: Result<Vec<u8>> = tokio::time::timeout(metadata_timeout, async {
                loop {
                    let msg = message_io
                        .read_message()
                        .await?
                        .ok_or_else(|| anyhow!("Connection closed before metadata response"))?;

                    match msg {
                        PeerMessage::Extended {
                            extension_id,
                            payload,
                        } if extension_id == OUR_UT_METADATA_ID => {
                            debug!("Received metadata response from peer {}", peer_addr);
                            return Ok::<Vec<u8>, anyhow::Error>(payload);
                        }
                        other => {
                            debug!(
                                "Skipping non-metadata message from {}: {:?}",
                                peer_addr, other
                            );
                        }
                    }
                }
            })
            .await
            .map_err(|_| anyhow!("Timeout waiting for metadata response from {}", peer_addr))?;

            result
        }

        debug!("Reading metadata response from peer {}", peer_addr);
        let payload = read_metadata_response(&mut message_io, peer_addr).await?;

        debug!("Parsing metadata data from peer {}", peer_addr);
        let piece0_response = parse_metadata_data(&payload)?;
        let total_size = piece0_response.total_size as usize;
        let piece_size = 16384; // BEP 9 standard metadata piece size
        let num_pieces = total_size.div_ceil(piece_size);

        debug!(
            "Received piece 0: {} bytes from peer {} (total size: {}, {} pieces needed)",
            piece0_response.data.len(),
            peer_addr,
            total_size,
            num_pieces
        );

        // Collect all pieces
        let mut all_pieces = vec![Vec::new(); num_pieces];
        all_pieces[0] = piece0_response.data;

        // Request remaining pieces if needed
        if num_pieces > 1 {
            debug!(
                "Metadata requires {} pieces, fetching remaining pieces from {}",
                num_pieces, peer_addr
            );

            for (idx, piece_slot) in all_pieces.iter_mut().enumerate().skip(1) {
                let piece_index = idx;
                debug!(
                    "Requesting metadata piece {} from peer {}",
                    piece_index, peer_addr
                );

                message_io
                    .write_message(&create_metadata_request(
                        *ut_metadata_id as u8,
                        piece_index as isize,
                    ))
                    .await?;

                let piece_payload = read_metadata_response(&mut message_io, peer_addr).await?;
                let piece_response = parse_metadata_data(&piece_payload)?;

                if piece_response.piece != piece_index as isize {
                    return Err(anyhow!(
                        "Received wrong piece: expected {}, got {}",
                        piece_index,
                        piece_response.piece
                    ));
                }

                debug!(
                    "Received piece {}: {} bytes from peer {}",
                    piece_index,
                    piece_response.data.len(),
                    peer_addr
                );

                *piece_slot = piece_response.data;
            }
        }

        // Concatenate all pieces
        let complete_metadata: Vec<u8> = all_pieces.into_iter().flatten().collect();

        // Verify size
        if complete_metadata.len() != total_size {
            return Err(anyhow!(
                "Metadata size mismatch: expected {} bytes, got {}",
                total_size,
                complete_metadata.len()
            ));
        }

        debug!(
            "Assembled complete metadata: {} bytes from peer {}",
            complete_metadata.len(),
            peer_addr
        );

        // Parse complete metadata
        let decoder = Decoder {};
        let (_n, bencode) = decoder.from_bytes(&complete_metadata)?;

        match bencode {
            BencodeTypes::Dictionary(info_dict) => {
                info!(
                    "Successfully parsed metadata dictionary from peer {}",
                    peer_addr
                );
                Ok(info_dict)
            }
            _ => {
                warn!("Metadata from {} is not a dictionary", peer_addr);
                Err(anyhow!("Metadata is not a dictionary"))
            }
        }
    }
}
