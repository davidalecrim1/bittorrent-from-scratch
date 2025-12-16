// Re-export modules
pub use crate::download_state::*;
pub use crate::messages::*;

use anyhow::{Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use log::debug;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Debug;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::codec::{FramedRead, FramedWrite};

use crate::encoding::{PeerMessageDecoder, PeerMessageEncoder};

const DEFAULT_BLOCK_SIZE: usize = 16 * 1024; // 16 KiB per BitTorrent spec
const BLOCK_PIPELINE_SIZE: usize = 5; // Request 5 blocks ahead

#[derive(Debug, Clone)]
pub struct PieceDownloadRequest {
    pub piece_index: u32,
    pub piece_length: usize,
    pub expected_hash: [u8; 20], // SHA1 hash for verification
}

#[derive(Debug, Clone)]
pub struct PeerManagerConfig {
    pub info_hash: [u8; 20],
    pub client_peer_id: [u8; 20],
    pub file_size: usize,
    pub num_pieces: usize,
    pub max_peers: usize,
}

#[derive(Debug)]
pub struct ConnectedPeer {
    pub peer: Peer,
    pub download_request_tx: mpsc::Sender<PieceDownloadRequest>,
    pub bitfield: Vec<bool>,
    pub active_downloads: HashSet<u32>,
}

#[derive(Debug, Clone)]
pub struct CompletedPiece {
    pub piece_index: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FailedPiece {
    pub piece_index: u32,
    pub reason: String, // "hash_mismatch" or "peer_disconnected"
}

#[derive(Debug, Clone)]
pub struct PeerDisconnected {
    pub peer_addr: String,
    pub reason: String,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum BencodeTypes {
    String(String),
    Integer(isize),
    List(Vec<BencodeTypes>),
    Dictionary(BTreeMap<String, BencodeTypes>),
    #[allow(dead_code)]
    Raw(Vec<u8>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Peer {
    pub ip: IpAddr,
    pub port: u16,
}

impl Peer {
    pub fn new(ip: String, port: u16) -> Self {
        Self {
            ip: IpAddr::from_str(&ip).unwrap(),
            port,
        }
    }

    pub fn get_addr(&self) -> String {
        format!("{}:{}", &self.ip, &self.port)
    }
}

pub struct PeerHandshake {
    info_hash: [u8; 20], // NOT the hexadecimal string, but the actual bytes
    peer_id: [u8; 20],
    reserved: [u8; 8],
}

impl PeerHandshake {
    pub fn new(info_hash: [u8; 20], peer_id: [u8; 20]) -> Self {
        Self {
            info_hash,
            peer_id,
            reserved: [0; 8],
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(68);
        // pstrlen (1 byte)
        bytes.push(19);
        // pstr (19 bytes)
        bytes.extend_from_slice(b"BitTorrent protocol");
        // reserved (8 bytes)
        bytes.extend_from_slice(&self.reserved);
        // info_hash (20 bytes)
        bytes.extend_from_slice(&self.info_hash);
        // peer_id (20 bytes)
        bytes.extend_from_slice(&self.peer_id);

        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 68 {
            return Err(anyhow!("the provided data is not a valid handshake"));
        }

        let info_hash = &bytes[19..39];
        let peer_id = &bytes[39..59];
        let reserved = &bytes[59..67];

        Ok(Self {
            info_hash: info_hash.try_into()?,
            peer_id: peer_id.try_into()?,
            reserved: reserved.try_into()?,
        })
    }

    pub fn get_peer_id(&self) -> [u8; 20] {
        self.peer_id
    }
}

impl Debug for PeerHandshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let info_hash = hex::encode(self.info_hash);
        let peer_id = hex::encode(self.peer_id);
        write!(
            f,
            "PeerHandshake {{ info_hash: {:?}, peer_id: {:?} }}",
            info_hash, peer_id
        )
    }
}

pub type PeerId = [u8; 20];

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
    #[allow(dead_code)]
    download_request_tx: mpsc::Sender<PieceDownloadRequest>,
    download_request_rx: Option<mpsc::Receiver<PieceDownloadRequest>>,
    piece_completion_tx: mpsc::Sender<CompletedPiece>,
    piece_failure_tx: mpsc::Sender<FailedPiece>,

    // Peer disconnection notification
    peer_disconnect_tx: mpsc::Sender<PeerDisconnected>,

    // Peer Status
    is_choking: bool,
    is_interested: bool,

    // Peer Information
    bitfield: Vec<bool>,

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
    ) -> (Self, mpsc::Sender<PieceDownloadRequest>) {
        let (outbound_tx, outbound_rx) = mpsc::channel(10);
        let (inbound_tx, inbound_rx) = mpsc::channel(10);
        let (download_request_tx, download_request_rx) = mpsc::channel(10);

        let peer_conn = Self {
            peer,
            peer_id: None,
            stream: None,
            outbound_tx,
            outbound_rx: Some(outbound_rx),
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            download_request_tx: download_request_tx.clone(),
            download_request_rx: Some(download_request_rx),
            piece_completion_tx,
            piece_failure_tx,
            peer_disconnect_tx,
            is_choking: false,
            is_interested: false,
            bitfield: Vec::new(),
            current_download: None,
        };

        (peer_conn, download_request_tx)
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
        let peer_addr = self.peer.get_addr();
        let peer_addr_reader = peer_addr.clone();
        let peer_addr_writer = peer_addr.clone();

        // Spawn writer task
        tokio::spawn(async move {
            while let Some(msg) = outbound_rx.recv().await {
                if let Err(e) = writer.send(msg).await {
                    debug!("[PeerConnection] Write error: {}", e);
                    let _ = disconnect_tx_writer
                        .send(PeerDisconnected {
                            peer_addr: peer_addr_writer.clone(),
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
                        debug!("[PeerConnection] Read error: {}", err);
                        let _ = disconnect_tx_reader
                            .send(PeerDisconnected {
                                peer_addr: peer_addr_reader,
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
        let peer_addr = self.peer.get_addr();

        tokio::task::spawn(async move {
            loop {
                tokio::select! {
                    Some(msg) = inbound_rx.recv() => {
                        if let Err(e) = self.handle_peer_message(msg).await {
                            debug!("[handle_incoming_messages] Error handling peer message: {}", e);
                            break;
                        }
                    }
                    Some(request) = download_request_rx.recv() => {
                        let piece_index = request.piece_index;
                        if let Err(e) = self.start_piece_download(request).await {
                            debug!("[handle_incoming_messages] Error starting download: {}", e);
                            // Send failure to manager but continue
                            let _ = self.piece_failure_tx.send(FailedPiece {
                                piece_index,
                                reason: e.to_string(),
                            }).await;
                        }
                    }
                    else => {
                        debug!("[handle_incoming_messages] Both channels closed, peer disconnected");
                        let _ = disconnect_tx.send(PeerDisconnected {
                            peer_addr: peer_addr.clone(),
                            reason: "channels_closed".to_string(),
                        }).await;
                        break;
                    }
                }
            }
            debug!("[handle_incoming_messages] Message handler exiting");
        });
    }

    async fn handle_peer_message(&mut self, msg: PeerMessage) -> Result<()> {
        match msg {
            PeerMessage::Bitfield(bitfield) => {
                self.bitfield = bitfield.bitfield;
                debug!(
                    "[handle_peer_message] Received bitfield with {} pieces",
                    self.bitfield.len()
                );
            }

            PeerMessage::Unchoke(_) => {
                self.is_choking = false;
                debug!("[handle_peer_message] Peer unchoked us");

                // If we have a pending download, start requesting blocks
                if self.current_download.is_some() {
                    self.handle_piece_download().await?;
                }
            }

            PeerMessage::Interested(_) => {
                self.is_interested = true;
            }

            PeerMessage::Request(_request) => {
                // We don't support uploading yet
                debug!("[handle_peer_message] Ignoring request message (upload not implemented)");
            }

            PeerMessage::Piece(piece) => {
                self.handle_piece_block(piece).await?;
            }
        }

        Ok(())
    }

    async fn handle_piece_block(&mut self, piece_msg: PieceMessage) -> Result<()> {
        let download_state = match self.current_download.as_mut() {
            Some(state) => state,
            None => {
                debug!(
                    "[handle_piece_block] Received unexpected piece block for piece {}, no active download",
                    piece_msg.piece_index
                );
                return Ok(());
            }
        };

        // Verify this block belongs to our current download
        if piece_msg.piece_index != download_state.piece_index {
            debug!(
                "[handle_piece_block] Received block for piece {}, but downloading piece {}",
                piece_msg.piece_index, download_state.piece_index
            );
            return Ok(());
        }

        // Add block to our state
        download_state.add_block(piece_msg.begin, piece_msg.block)?;

        debug!(
            "[handle_piece_block] Received block: piece={}, begin={}, blocks={}/{}",
            piece_msg.piece_index,
            piece_msg.begin,
            download_state.received_blocks.len(),
            download_state.total_blocks
        );

        if download_state.is_complete() {
            self.finish_piece_download().await?;
        } else {
            self.handle_piece_download().await?;
        }

        Ok(())
    }

    async fn start_piece_download(&mut self, request: PieceDownloadRequest) -> Result<()> {
        if !self.has_piece(request.piece_index as usize) {
            return Err(anyhow!("Peer does not have piece {}", request.piece_index));
        }

        if !self.can_download_pieces() {
            return Err(anyhow!(
                "Peer not ready for download (choking={}, interested={}, bitfield={})",
                self.is_choking,
                self.is_interested,
                self.bitfield.is_empty()
            ));
        }

        if self.current_download.is_some() {
            return Err(anyhow!(
                "Already downloading a piece, cannot start new download"
            ));
        }

        debug!(
            "[start_piece_download] Starting download of piece {}, length={}",
            request.piece_index, request.piece_length
        );

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

        debug!(
            "[finish_piece_download] Piece {} complete, assembling data",
            download_state.piece_index
        );

        let piece_data = download_state.assemble_piece()?;

        match download_state.verify_hash(&piece_data)? {
            true => {
                let completed = CompletedPiece {
                    piece_index: download_state.piece_index,
                    data: piece_data,
                };

                self.piece_completion_tx
                    .send(completed)
                    .await
                    .map_err(|_| anyhow!("Failed to send completed piece to manager"))?;

                debug!(
                    "[finish_piece_download] Sent completed piece {} to manager",
                    download_state.piece_index
                );
            }
            false => {
                let failed = FailedPiece {
                    piece_index: download_state.piece_index,
                    reason: "hash_mismatch".to_string(),
                };

                self.piece_failure_tx
                    .send(failed)
                    .await
                    .map_err(|_| anyhow!("Failed to send failed piece to manager"))?;

                debug!(
                    "[finish_piece_download] Hash mismatch for piece {}, notified manager",
                    download_state.piece_index
                );
            }
        }

        Ok(())
    }

    pub fn has_piece(&self, piece_index: usize) -> bool {
        self.bitfield.get(piece_index).copied().unwrap_or(false)
    }

    pub fn can_download_pieces(&self) -> bool {
        !self.is_choking && self.is_interested && !self.bitfield.is_empty()
    }

    pub fn get_bitfield(&self) -> &[bool] {
        &self.bitfield
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
        // Check if we have an active download
        let download_state = match self.current_download.as_mut() {
            Some(state) => state,
            None => {
                debug!("[handle_piece_download] No active download");
                return Ok(());
            }
        };

        // Request blocks in pipeline fashion
        let mut requests_sent = 0;
        while requests_sent < BLOCK_PIPELINE_SIZE {
            match download_state.get_next_block_to_request() {
                Some((begin, length)) => {
                    let request = RequestMessage {
                        piece_index: download_state.piece_index,
                        begin,
                        length,
                    };

                    debug!(
                        "[handle_piece_download] Requesting block: piece={}, begin={}, length={}",
                        download_state.piece_index, begin, length
                    );

                    self.outbound_tx.send(PeerMessage::Request(request)).await?;

                    requests_sent += 1;
                }
                None => {
                    // No more blocks to request
                    break;
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct Piece {
    #[allow(dead_code)]
    index: usize,
    #[allow(dead_code)]
    status: PieceStatus,
}

impl Piece {
    pub fn new(index: usize, status: PieceStatus) -> Self {
        Self { index, status }
    }
}

#[derive(Debug)]
pub enum PieceStatus {
    Pending,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitfield_message_from_bytes_single_byte() {
        let bytes = vec![0b10101010];
        let num_pieces = 8;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        assert!(bitfield.has_piece(0));
        assert!(!bitfield.has_piece(1));
        assert!(bitfield.has_piece(2));
        assert!(!bitfield.has_piece(3));
        assert!(bitfield.has_piece(4));
        assert!(!bitfield.has_piece(5));
        assert!(bitfield.has_piece(6));
        assert!(!bitfield.has_piece(7));
    }

    #[test]
    fn test_bitfield_message_from_bytes_multiple_bytes() {
        let bytes = vec![0b11110000, 0b00001111];
        let num_pieces = 16;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        for i in 0..4 {
            assert!(bitfield.has_piece(i), "Piece {} should be available", i);
        }
        for i in 4..8 {
            assert!(
                !bitfield.has_piece(i),
                "Piece {} should not be available",
                i
            );
        }
        for i in 8..12 {
            assert!(
                !bitfield.has_piece(i),
                "Piece {} should not be available",
                i
            );
        }
        for i in 12..16 {
            assert!(bitfield.has_piece(i), "Piece {} should be available", i);
        }
    }

    #[test]
    fn test_bitfield_message_fewer_pieces_than_bits() {
        let bytes = vec![0b11111111];
        let num_pieces = 5;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        for i in 0..5 {
            assert!(bitfield.has_piece(i), "Piece {} should be available", i);
        }

        assert!(!bitfield.has_piece(5));
        assert!(!bitfield.has_piece(6));
        assert!(!bitfield.has_piece(7));
    }

    #[test]
    fn test_bitfield_message_all_pieces_available() {
        let bytes = vec![0b11111111, 0b11111111];
        let num_pieces = 16;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        for i in 0..16 {
            assert!(bitfield.has_piece(i), "Piece {} should be available", i);
        }
    }

    #[test]
    fn test_bitfield_message_no_pieces_available() {
        let bytes = vec![0b00000000, 0b00000000];
        let num_pieces = 16;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        for i in 0..16 {
            assert!(
                !bitfield.has_piece(i),
                "Piece {} should not be available",
                i
            );
        }
    }

    #[test]
    fn test_bitfield_message_has_piece_out_of_bounds() {
        let bytes = vec![0b10101010];
        let num_pieces = 4;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        assert!(!bitfield.has_piece(100));
        assert!(!bitfield.has_piece(8));
    }

    #[test]
    fn test_bitfield_message_empty_bytes() {
        let bytes = vec![];
        let num_pieces = 0;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        assert!(!bitfield.has_piece(0));
    }

    #[test]
    fn test_bitfield_message_msb_ordering() {
        let bytes = vec![0b10000000];
        let num_pieces = 8;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        assert!(bitfield.has_piece(0));
        for i in 1..8 {
            assert!(
                !bitfield.has_piece(i),
                "Piece {} should not be available",
                i
            );
        }
    }

    #[test]
    fn test_bitfield_message_realistic_scenario() {
        let bytes = vec![0b11010100, 0b10110000, 0b00000001];
        let num_pieces = 20;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        let expected = [
            true, true, false, true, false, true, false, false, true, false, true, true, false,
            false, false, false, false, false, false, false,
        ];

        for (i, &expected_value) in expected.iter().enumerate() {
            assert_eq!(
                bitfield.has_piece(i),
                expected_value,
                "Piece {} should be {}",
                i,
                if expected_value {
                    "available"
                } else {
                    "not available"
                }
            );
        }
    }

    #[test]
    fn test_bitfield_message_insufficient_bytes() {
        let bytes = vec![0b11110000];
        let num_pieces = 12;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        for i in 0..4 {
            assert!(bitfield.has_piece(i), "Piece {} should be available", i);
        }
        for i in 4..12 {
            assert!(
                !bitfield.has_piece(i),
                "Piece {} should not be available",
                i
            );
        }
    }
}
