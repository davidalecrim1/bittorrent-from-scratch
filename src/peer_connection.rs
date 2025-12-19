use anyhow::{Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use log::debug;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{RwLock, mpsc};
use tokio::time::sleep;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::download_state::DownloadState;
use crate::encoding::{PeerMessageDecoder, PeerMessageEncoder};
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

#[derive(Debug)]
pub struct PeerConnection {
    peer: Peer,
    peer_id: Option<PeerId>,

    // Stream is only present after handshake
    stream: Option<TcpStream>,

    // Outbound messages the actor will write to the wire
    outbound_tx: mpsc::Sender<PeerMessage>,
    outbound_rx: Option<mpsc::Receiver<PeerMessage>>,

    // Inbound messages read from the wire and sent out
    inbound_tx: mpsc::Sender<PeerMessage>,
    inbound_rx: Option<mpsc::Receiver<PeerMessage>>,

    // Piece download channels
    download_request_rx: Option<mpsc::Receiver<PieceDownloadRequest>>,
    piece_completion_tx: mpsc::Sender<CompletedPiece>,
    piece_failure_tx: mpsc::Sender<FailedPiece>,

    // Peer disconnection notification
    peer_disconnect_tx: mpsc::Sender<PeerDisconnected>,

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
        piece_completion_tx: mpsc::Sender<CompletedPiece>,
        piece_failure_tx: mpsc::Sender<FailedPiece>,
        peer_disconnect_tx: mpsc::Sender<PeerDisconnected>,
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

    // Opens the connection and performs the handshake.
    // This is the kick off to run the messages loop.
    pub async fn handshake(
        &mut self,
        client_peer_id: [u8; 20],
        info_hash: [u8; 20],
    ) -> Result<PeerHandshake> {
        let handshake = PeerHandshake::new(info_hash, client_peer_id);
        let bytes = handshake.to_bytes();
        let max_retries = 3;

        let mut last_error = None;

        for attempt in 1..=max_retries {
            match self.try_handshake(&bytes).await {
                Ok(response) => {
                    return Ok(response);
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        let delay = Duration::from_millis(200 * 2_u64.pow(attempt - 1)); // 200ms, 400ms, 800ms ...
                        sleep(delay).await;
                    }
                }
            }
        }

        Err(anyhow!(
            "Failed to handshake with peer {} after {} attempts: {}",
            self.peer.get_addr(),
            max_retries,
            last_error.unwrap()
        ))
    }

    async fn try_handshake(&mut self, handshake_bytes: &[u8]) -> Result<PeerHandshake> {
        let mut stream = TcpStream::connect(self.peer.get_addr()).await?;
        stream.write_all(handshake_bytes).await?;

        let mut buffer = vec![0u8; 68];
        let n = stream.read(&mut buffer).await?;

        let response = PeerHandshake::from_bytes(&buffer[..n])?;
        self.peer_id = Some(response.get_peer_id());
        self.stream = Some(stream);

        Ok(response)
    }

    pub async fn start(mut self, num_pieces: usize) -> Result<()> {
        let stream = self
            .stream
            .take()
            .ok_or_else(|| anyhow!("handshake not done"))?;
        let (reader, writer) = stream.into_split();

        let decoder = PeerMessageDecoder::new(num_pieces);
        let encoder = PeerMessageEncoder::new(num_pieces);

        let mut reader = FramedRead::new(reader, decoder);
        let mut writer = FramedWrite::new(writer, encoder);

        // Take the receivers out of self
        let mut outbound_rx = self.outbound_rx.take().unwrap();
        let inbound_tx = self.inbound_tx.clone();
        let disconnect_tx_reader = self.peer_disconnect_tx.clone();
        let disconnect_tx_writer = self.peer_disconnect_tx.clone();
        let peer_reader = self.peer.clone();
        let peer_writer = self.peer.clone();

        // Spawn writer task
        tokio::spawn(async move {
            while let Some(msg) = outbound_rx.recv().await {
                if let Err(e) = writer.send(msg).await {
                    let _ = disconnect_tx_writer
                        .send(PeerDisconnected {
                            peer: peer_writer.clone(),
                            reason: format!("write_error: {}", e),
                        })
                        .await;
                    break;
                }
            }
        });

        // Spawn reader task
        tokio::spawn(async move {
            while let Some(result) = reader.next().await {
                match result {
                    Ok(msg) => {
                        let _ = inbound_tx.send(msg).await;
                    }
                    Err(err) => {
                        let _ = disconnect_tx_reader
                            .send(PeerDisconnected {
                                peer: peer_reader,
                                reason: format!("read_error: {}", err),
                            })
                            .await;
                        break;
                    }
                }
            }
        });

        self.send_interested_message().await;
        self.handle_incoming_messages();

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
                                }).await;
                                break;
                            }
                        }
                    }
                    result = download_request_rx.recv() => {
                        match result {
                            Some(request) => {
                                debug!("Peer {} received download request for piece {}", peer_addr.clone(), request.piece_index);

                                let piece_index = request.piece_index;
                                if let Err(e) = self.start_piece_download(peer_addr.clone(), request).await {
                                    debug!("Peer {} failed to start piece download: {}", peer_addr.clone(), e);
                                    let _ = self.piece_failure_tx.send(FailedPiece {
                                        piece_index,
                                        reason: e.to_string(),
                                    }).await;
                                }
                            }
                            None => {
                                // Download request channel closed (shouldn't happen during normal operation)
                                debug!("Peer {} download_request channel closed unexpectedly", peer_addr);
                                let _ = disconnect_tx.send(PeerDisconnected {
                                    peer: peer.clone(),
                                    reason: "download_request_channel_closed".to_string(),
                                }).await;
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
                // No-op for now.
                // TODO: Consider refreshing the connection.
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
                    "Peer {} received Bitfield message with {}/{} pieces available",
                    peer_addr,
                    num_pieces_available,
                    bitfield.bitfield.len()
                );

                let mut bf = self.bitfield.write().await;
                *bf = bitfield.bitfield;
            }

            PeerMessage::Unchoke(_) => {
                self.is_choking = false;

                if self.current_download.is_some() {
                    self.handle_piece_download().await?;
                }
            }

            PeerMessage::Interested(_) => {
                self.is_interested = true;
            }

            PeerMessage::NotInterested(_) => {
                self.is_interested = false;
            }

            PeerMessage::Have(have) => {
                let piece_index = have.piece_index as usize;
                let mut bf = self.bitfield.write().await;
                if piece_index < bf.len() {
                    bf[piece_index] = true;
                }
            }

            PeerMessage::Request(_request) => {
                // We don't support uploading yet
            }

            PeerMessage::Piece(piece) => {
                self.handle_piece_block(piece).await?;
            }

            PeerMessage::Cancel(_cancel) => {
                // We don't support uploading yet, so no-op
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

        debug!(
            "Peer {} received block at offset {} for piece {} ({} bytes) - progress: {}/{}",
            self.peer.get_addr(),
            piece_msg.begin,
            piece_msg.piece_index,
            piece_msg.block.len(),
            download_state.received_blocks.len() + 1,
            download_state.total_blocks
        );

        download_state.add_block(piece_msg.begin, piece_msg.block)?;

        if download_state.is_complete() {
            debug!(
                "Peer {} all blocks received for piece {}, finishing download",
                self.peer.get_addr(),
                download_state.piece_index
            );
            self.finish_piece_download().await?;
        } else {
            self.handle_piece_download().await?;
        }

        Ok(())
    }

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
                    .await
                    .map_err(|_| anyhow!("Failed to send completed piece to manager"))?;
            }
            false => {
                debug!(
                    "Peer {} failed downloading the piece {}",
                    self.peer.get_addr().clone(),
                    download_state.piece_index
                );

                let failed = FailedPiece {
                    piece_index: download_state.piece_index,
                    reason: "hash_mismatch".to_string(),
                };

                self.piece_failure_tx
                    .send(failed)
                    .await
                    .map_err(|_| anyhow!("Failed to send failed piece to manager"))?;

                debug!("Hash mismatch for piece {}", download_state.piece_index);
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

    async fn send_interested_message(&self) {
        let outbound_tx = self.outbound_tx.clone();

        match outbound_tx
            .send(PeerMessage::Interested(InterestedMessage {}))
            .await
        {
            Ok(_) => {}
            Err(err) => {
                debug!(
                    "Failed to send interested message to peer {:?}: {}",
                    self.peer_id, err
                );
            }
        }
    }

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
                        "Peer {} sending request message for piece {} and block {}",
                        self.peer.get_addr().clone(),
                        download_state.piece_index,
                        begin
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
