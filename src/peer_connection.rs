use anyhow::{Result, anyhow};
use log::debug;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{RwLock, mpsc};
use tokio::time::sleep;

use crate::download_state::DownloadState;
use crate::messages::{InterestedMessage, PeerMessage, PieceMessage, RequestMessage};
use crate::types::{
    CompletedPiece, FailedPiece, Peer, PeerDisconnected, PeerHandshake, PeerId,
    PieceDownloadRequest,
};

const DEFAULT_BLOCK_SIZE: usize = 16 * 1024; // 16 KiB per BitTorrent spec
const BLOCK_PIPELINE_SIZE: usize = 5; // Request 5 blocks ahead

const OUTBOUND_CHANNEL_SIZE: usize = 32;
const INBOUND_CHANNEL_SIZE: usize = 64;
const DOWNLOAD_REQUEST_CHANNEL_SIZE: usize = 200;

const MAX_RETRIES_HANDSHAKE: usize = 3;

#[derive(Debug)]
pub struct PeerConnection {
    peer: Peer,
    peer_id: Option<PeerId>,

    // Stream is only present after handshake
    stream: Option<TcpStream>,

    // TCP connector for establishing connections
    tcp_connector: Arc<dyn crate::traits::TcpConnector>,

    // Outbound messages the actor will write to the wire
    outbound_tx: mpsc::Sender<PeerMessage>,
    outbound_rx: Option<mpsc::Receiver<PeerMessage>>,

    // Inbound messages read from the wire and sent out
    inbound_tx: mpsc::Sender<PeerMessage>,
    inbound_rx: Option<mpsc::Receiver<PeerMessage>>,

    // Piece download channels
    download_request_rx: Option<mpsc::Receiver<PieceDownloadRequest>>,
    piece_completion_tx: mpsc::UnboundedSender<CompletedPiece>,
    piece_failure_tx: mpsc::UnboundedSender<FailedPiece>,

    // Peer disconnection notification
    peer_disconnect_tx: mpsc::UnboundedSender<PeerDisconnected>,

    // Peer Status
    is_choking: bool,
    is_interested: bool,

    // Peer Information (shared with PeerManager)
    bitfield: Arc<RwLock<Vec<bool>>>,

    // Download state
    current_download: Option<DownloadState>,
}

// Creates a connection with the Peer provided and wraps it within
// It will rely on the inbound_tx and outbound_rx channels to communicate with the peer connection.
impl PeerConnection {
    pub fn new(
        peer: Peer,
        piece_completion_tx: mpsc::UnboundedSender<CompletedPiece>,
        piece_failure_tx: mpsc::UnboundedSender<FailedPiece>,
        peer_disconnect_tx: mpsc::UnboundedSender<PeerDisconnected>,
        tcp_connector: Arc<dyn crate::traits::TcpConnector>,
    ) -> (
        Self,
        mpsc::Sender<PieceDownloadRequest>,
        Arc<RwLock<Vec<bool>>>,
    ) {
        let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_CHANNEL_SIZE);
        let (inbound_tx, inbound_rx) = mpsc::channel(INBOUND_CHANNEL_SIZE);
        let (download_request_tx, download_request_rx) =
            mpsc::channel(DOWNLOAD_REQUEST_CHANNEL_SIZE);

        let bitfield = Arc::new(RwLock::new(Vec::new()));

        let peer_conn = Self {
            peer,
            peer_id: None,
            stream: None,
            tcp_connector,
            outbound_tx,
            outbound_rx: Some(outbound_rx),
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            download_request_rx: Some(download_request_rx),
            piece_completion_tx,
            piece_failure_tx,
            peer_disconnect_tx,
            is_choking: false,
            is_interested: false,
            bitfield: bitfield.clone(),
            current_download: None,
        };

        (peer_conn, download_request_tx, bitfield)
    }

    pub fn get_peer_id(&self) -> Option<[u8; 20]> {
        self.peer_id
    }

    // Handshake with the peer to initialize the connection.
    pub async fn handshake(
        &mut self,
        client_peer_id: [u8; 20],
        info_hash: [u8; 20],
    ) -> Result<PeerHandshake> {
        let handshake = PeerHandshake::new(info_hash, client_peer_id);
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
    async fn try_handshake(&mut self, handshake_bytes: &[u8]) -> Result<PeerHandshake> {
        let mut stream = self.tcp_connector.connect(self.peer.get_addr()).await?;
        stream.write_all(handshake_bytes).await?;

        let mut buffer = vec![0u8; 68];
        let n = stream.read(&mut buffer).await?;

        let response = PeerHandshake::from_bytes(&buffer[..n])?;
        self.peer_id = Some(response.get_peer_id());
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
        let message_io = crate::io::TcpMessageIO::from_stream(stream, num_pieces);
        self.start_with_io(Box::new(message_io)).await
    }

    // Starts with a custom MessageIO implementation.
    // Usually done for testing purposes.
    pub async fn start_with_io(
        mut self,
        mut message_io: Box<dyn crate::traits::MessageIO>,
    ) -> Result<()> {
        // Take the receivers out of self
        let mut outbound_rx = self.outbound_rx.take().unwrap();
        let inbound_tx = self.inbound_tx.clone();
        let disconnect_tx = self.peer_disconnect_tx.clone();
        let peer = self.peer.clone();

        // Spawn single I/O task that handles both reading and writing
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Handle outbound messages (write to network)
                    Some(msg) = outbound_rx.recv() => {
                        if let Err(e) = message_io.write_message(&msg).await {
                            let _ = disconnect_tx.send(PeerDisconnected {
                                peer: peer.clone(),
                                reason: format!("write_error: {}", e),
                            });
                            break;
                        }
                    }

                    // Handle inbound messages (read from network)
                    result = message_io.read_message() => {
                        match result {
                            Ok(Some(msg)) => {
                                if inbound_tx.send(msg).await.is_err() {
                                    // Channel closed, stop I/O task
                                    break;
                                }
                            }
                            Ok(None) => {
                                // Stream ended gracefully
                                let _ = disconnect_tx.send(PeerDisconnected {
                                    peer: peer.clone(),
                                    reason: "stream_closed".to_string(),
                                });
                                break;
                            }
                            Err(e) => {
                                let _ = disconnect_tx.send(PeerDisconnected {
                                    peer: peer.clone(),
                                    reason: format!("read_error: {}", e),
                                });
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Clone outbound_tx before handle_incoming_messages consumes self
        let outbound_tx = self.outbound_tx.clone();

        // Spawn message handler first (ensures it's ready before we trigger responses)
        self.handle_incoming_messages();

        // Now send interested message (peer will respond with bitfield)
        let _ = outbound_tx
            .send(PeerMessage::Interested(InterestedMessage {}))
            .await;

        Ok(())
    }

    fn handle_incoming_messages(mut self) {
        let mut inbound_rx = self.inbound_rx.take().unwrap();
        let mut download_request_rx = self.download_request_rx.take().unwrap();
        let disconnect_tx = self.peer_disconnect_tx.clone();
        let peer = self.peer.clone();
        let peer_addr = peer.get_addr();

        tokio::task::spawn(async move {
            loop {
                tokio::select! {
                    // Use biased to ensure fair scheduling between branches
                    biased;

                    result = inbound_rx.recv() => {
                        match result {
                            Some(msg) => {
                                if let Err(_e) = self.handle_peer_message(peer_addr.clone(), msg).await {
                                    break;
                                }
                            }
                            None => {
                                // Reader channel closed (peer disconnected)
                                let _ = disconnect_tx.send(PeerDisconnected {
                                    peer: peer.clone(),
                                    reason: "inbound_channel_closed".to_string(),
                                });
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
                                    let _ = self.piece_failure_tx.send(FailedPiece {
                                        piece_index,
                                        reason: e.to_string(),
                                    });
                                }
                            }
                            None => {
                                // Download request channel closed (shouldn't happen during normal operation)
                                debug!("Peer Connector download_request channel closed unexpectedly from Peer {}", peer_addr);
                                let _ = disconnect_tx.send(PeerDisconnected {
                                    peer: peer.clone(),
                                    reason: "download_request_channel_closed".to_string(),
                                });
                                break;
                            }
                        }
                    }
                }
            }
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
                // Peer is choking us - we should stop requesting pieces
                // Current download can continue receiving already-requested blocks
                // but we won't send new requests until unchoked
                self.is_choking = true;
            }

            PeerMessage::Bitfield(bitfield) => {
                let num_pieces_available = bitfield.bitfield.iter().filter(|&&has| has).count();
                debug!(
                    "Peer Connector received Bitfield with {}/{} pieces available from Peer {}",
                    num_pieces_available,
                    bitfield.bitfield.len(),
                    peer_addr,
                );

                let mut bf = self.bitfield.write().await;
                *bf = bitfield.bitfield;
            }

            PeerMessage::Unchoke(_) => {
                debug!("Peer Connector received Unchoke from Peer {}", peer_addr);
                self.is_choking = false;

                if self.current_download.is_some() {
                    self.handle_piece_download().await?;
                }
            }

            PeerMessage::Interested(_) => {
                debug!("Peer Connector received Interested from Peer {}", peer_addr);
                self.is_interested = true;
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
                let piece_index = have.piece_index as usize;
                let mut bf = self.bitfield.write().await;
                if piece_index < bf.len() {
                    bf[piece_index] = true;
                }
            }

            PeerMessage::Request(request) => {
                debug!(
                    "Peer Connector received Request for piece {} from Peer {} (ignored - upload not supported)",
                    request.piece_index, peer_addr
                );
            }

            PeerMessage::Piece(piece) => {
                debug!(
                    "Peer Connector received Piece for index {}, begin {} from Peer {}",
                    piece.piece_index, piece.begin, peer_addr
                );
                self.handle_piece_block(piece).await?;
            }

            PeerMessage::Cancel(cancel) => {
                // TODO: Check how to support this.
                debug!(
                    "Peer Connector received Cancel for piece {} from Peer {} (ignored - upload not supported)",
                    cancel.piece_index, peer_addr,
                );
            }
        }

        Ok(())
    }

    async fn handle_piece_block(&mut self, piece_msg: PieceMessage) -> Result<()> {
        let download_state = match self.current_download.as_mut() {
            Some(state) => state,
            None => {
                debug!(
                    "Peer {} received piece block for index {} but no active download",
                    self.peer.get_addr(),
                    piece_msg.piece_index
                );
                return Ok(());
            }
        };

        if piece_msg.piece_index != download_state.piece_index {
            debug!(
                "Peer {} received piece block for index {} but expected {}",
                self.peer.get_addr(),
                piece_msg.piece_index,
                download_state.piece_index
            );
            return Ok(());
        }

        download_state.add_block(piece_msg.begin, piece_msg.block)?;

        if download_state.is_complete() {
            debug!(
                "Peer Connector all blocks received for piece {}, finishing download from Peer {}",
                download_state.piece_index,
                self.peer.get_addr(),
            );
            self.finish_piece_download().await?;
        } else {
            self.handle_piece_download().await?;
        }

        Ok(())
    }

    // Starts the download of a piece from the Peer Manager.
    async fn start_piece_download(
        &mut self,
        _peer_addr: String,
        request: PieceDownloadRequest,
    ) -> Result<()> {
        if !self.has_piece(request.piece_index as usize).await {
            return Err(anyhow!("Peer does not have piece {}", request.piece_index));
        }

        if !self.can_download_pieces().await {
            let bf = self.bitfield.read().await;
            return Err(anyhow!(
                "Peer not ready for download (choking={}, bitfield={})",
                self.is_choking,
                bf.is_empty()
            ));
        }

        if self.current_download.is_some() {
            return Err(anyhow!(
                "Already downloading a piece, cannot start new download"
            ));
        }

        self.current_download = Some(DownloadState::new(
            request.piece_index,
            request.piece_length,
            DEFAULT_BLOCK_SIZE,
            request.expected_hash,
        ));

        self.handle_piece_download().await?;

        Ok(())
    }

    // After all the blocks have been downloaded, verify the hash and
    // send the completed piece to the Peer Manager.
    async fn finish_piece_download(&mut self) -> Result<()> {
        let download_state = self
            .current_download
            .take()
            .ok_or_else(|| anyhow!("No active download to finish"))?;

        let piece_data = download_state.assemble_piece()?;

        match download_state.verify_hash(&piece_data)? {
            true => {
                debug!(
                    "Peer {} finished downloading the piece {}",
                    self.peer.get_addr().clone(),
                    download_state.piece_index
                );

                let completed = CompletedPiece {
                    piece_index: download_state.piece_index,
                    data: piece_data,
                };

                self.piece_completion_tx
                    .send(completed)
                    .map_err(|_| anyhow!("Failed to send completed piece to manager"))?;
            }
            false => {
                use sha1::{Digest, Sha1};
                let computed_hash = Sha1::new().chain_update(&piece_data).finalize();
                let computed_hash_bytes: [u8; 20] = computed_hash.into();

                debug!(
                    "Peer {} - Hash mismatch for piece {}: expected {:02x?}, got {:02x?}, piece_length={}, data_length={}",
                    self.peer.get_addr().clone(),
                    download_state.piece_index,
                    download_state.expected_hash(),
                    computed_hash_bytes,
                    download_state.piece_length(),
                    piece_data.len()
                );

                let failed = FailedPiece {
                    piece_index: download_state.piece_index,
                    reason: "hash_mismatch".to_string(),
                };

                self.piece_failure_tx
                    .send(failed)
                    .map_err(|_| anyhow!("Failed to send failed piece to manager"))?;
            }
        }

        Ok(())
    }

    pub async fn has_piece(&self, piece_index: usize) -> bool {
        let bf = self.bitfield.read().await;
        bf.get(piece_index).copied().unwrap_or(false)
    }

    pub async fn can_download_pieces(&self) -> bool {
        let bf = self.bitfield.read().await;
        !self.is_choking && !bf.is_empty()
    }

    // Controls the blocks in the requests messages to download pieces from the peer.
    async fn handle_piece_download(&mut self) -> Result<()> {
        let download_state = match self.current_download.as_mut() {
            Some(state) => state,
            None => {
                return Ok(());
            }
        };

        let mut requests_sent = 0;
        while requests_sent < BLOCK_PIPELINE_SIZE {
            match download_state.get_next_block_to_request() {
                Some((begin, length)) => {
                    debug!(
                        "Peer Connector sending request for piece {} block {} from Peer {}",
                        download_state.piece_index,
                        begin,
                        self.peer.get_addr(),
                    );

                    let request = RequestMessage {
                        piece_index: download_state.piece_index,
                        begin,
                        length,
                    };

                    self.outbound_tx.send(PeerMessage::Request(request)).await?;
                    requests_sent += 1;
                }
                None => {
                    break;
                }
            }
        }

        Ok(())
    }
}
