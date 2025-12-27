// Designed to be small and simple types used in the codebase,
// anything more complex deserves it's own module.
pub use crate::peer_connection::PeerConnection;

use anyhow::{Result, anyhow};
use log::debug;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Debug;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

use crate::download_state::DownloadState;
use crate::messages::{PeerMessage, PieceMessage, RequestMessage};

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
}

#[derive(Debug)]
pub struct ConnectedPeer {
    peer: Peer,
    download_request_tx: mpsc::Sender<PieceDownloadRequest>,
    bitfield: Arc<RwLock<Vec<bool>>>,
    active_downloads: HashSet<u32>,
}

impl ConnectedPeer {
    pub fn new(
        peer: Peer,
        download_request_tx: mpsc::Sender<PieceDownloadRequest>,
        bitfield: Arc<RwLock<Vec<bool>>>, // Shared with PeerConnection
    ) -> Self {
        Self {
            peer,
            download_request_tx,
            bitfield,
            active_downloads: HashSet::new(),
        }
    }

    pub async fn has_piece(&self, piece_index: usize) -> bool {
        let bf = self.bitfield.read().await;
        bf.get(piece_index).copied().unwrap_or(false)
    }

    pub async fn piece_count(&self) -> usize {
        let bf = self.bitfield.read().await;
        bf.iter().filter(|&&has| has).count()
    }

    pub async fn bitfield_len(&self) -> usize {
        self.bitfield.read().await.len()
    }

    pub async fn request_piece(&self, request: PieceDownloadRequest) -> Result<()> {
        self.download_request_tx
            .send(request)
            .await
            .map_err(|e| anyhow!("Failed to send download request: {}", e))
    }

    pub fn get_sender(&self) -> mpsc::Sender<PieceDownloadRequest> {
        self.download_request_tx.clone()
    }

    pub fn peer_addr(&self) -> String {
        self.peer.get_addr()
    }

    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    pub fn mark_downloading(&mut self, piece_index: u32) {
        self.active_downloads.insert(piece_index);
    }

    pub fn mark_complete(&mut self, piece_index: u32) -> bool {
        self.active_downloads.remove(&piece_index)
    }

    pub fn is_downloading(&self, piece_index: u32) -> bool {
        self.active_downloads.contains(&piece_index)
    }

    pub fn active_download_count(&self) -> usize {
        self.active_downloads.len()
    }

    pub fn active_downloads(&self) -> impl Iterator<Item = &u32> {
        self.active_downloads.iter()
    }

    pub fn take_active_downloads(&mut self) -> HashSet<u32> {
        std::mem::take(&mut self.active_downloads)
    }
}

#[derive(Debug, Clone)]
pub struct CompletedPiece {
    pub piece_index: u32,
    pub peer_addr: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FailedPiece {
    pub piece_index: u32,
    pub peer_addr: String,
    pub reason: String, // "hash_mismatch" or "peer_disconnected"
}

#[derive(Debug, Clone)]
pub struct PeerDisconnected {
    pub peer: Peer,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct DownloadComplete;

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
        // Send shutdown signal to all subscribers
        let _ = self.shutdown_tx.send(());
    }
}

pub struct PieceDownloadTask {
    pub piece_index: u32,
    pub download_state: DownloadState,
    pub block_semaphore: Arc<tokio::sync::Semaphore>,
    pub outbound_tx: mpsc::Sender<PeerMessage>,
    pub inbound_rx: mpsc::Receiver<PieceMessage>,
    pub completion_tx: mpsc::UnboundedSender<CompletedPiece>,
    pub failure_tx: mpsc::UnboundedSender<FailedPiece>,
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

                let request = RequestMessage {
                    piece_index: self.piece_index,
                    begin,
                    length,
                };

                self.outbound_tx.send(PeerMessage::Request(request)).await?;

                let piece_msg = match self.inbound_rx.recv().await {
                    Some(msg) => msg,
                    None => {
                        debug!(
                            "Piece {}: Channel closed, peer disconnected",
                            self.piece_index
                        );
                        self.send_failure("Peer disconnected".to_string()).await?;
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
                        self.send_failure("Peer disconnected".to_string()).await?;
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
            debug!(
                "Piece {}: Hash verified, sending completion",
                self.piece_index
            );
            self.completion_tx.send(CompletedPiece {
                piece_index: self.piece_index,
                data: piece_data,
                peer_addr: self.peer_addr.clone(),
            })?;
        } else {
            debug!("Piece {}: Hash mismatch, sending failure", self.piece_index);
            self.send_failure("Hash mismatch".to_string()).await?;
        }

        Ok(())
    }

    async fn send_failure(&self, reason: String) -> Result<()> {
        self.failure_tx.send(FailedPiece {
            piece_index: self.piece_index,
            peer_addr: self.peer_addr.clone(),
            reason,
        })?;
        Ok(())
    }
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

pub type PeerId = [u8; 20];

impl Peer {
    pub fn new(ip: String, port: u16) -> Self {
        Self {
            ip: IpAddr::from_str(&ip).unwrap(),
            port,
        }
    }

    pub fn get_addr(&self) -> String {
        if self.ip.is_ipv6() {
            // IPv6 requires bracket notation for socket addresses
            format!("[{}]:{}", &self.ip, &self.port)
        } else {
            // IPv4 standard notation
            format!("{}:{}", &self.ip, &self.port)
        }
    }
}

/// Tracker announce request parameters
#[derive(Debug, Clone)]
pub struct AnnounceRequest {
    pub endpoint: String,
    pub info_hash: Vec<u8>,
    pub peer_id: String,
    pub port: u16,
    pub uploaded: usize,
    pub downloaded: usize,
    pub left: usize,
}

/// Tracker announce response
#[derive(Debug, Clone)]
pub struct AnnounceResponse {
    pub interval: Option<u64>,
    pub peers: Vec<Peer>,
}

#[cfg(test)]
mod tests {
    use super::Peer;
    use crate::messages::BitfieldMessage;

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

    #[test]
    fn test_peer_ipv4_address_formatting() {
        let peer = Peer::new("192.168.1.100".to_string(), 6881);
        assert_eq!(peer.get_addr(), "192.168.1.100:6881");
    }

    #[test]
    fn test_peer_ipv6_address_formatting() {
        // IPv6 addresses require bracket notation for socket addresses
        let peer = Peer::new("2001:db8::1".to_string(), 6881);
        assert_eq!(peer.get_addr(), "[2001:db8::1]:6881");

        let peer2 = Peer::new("fe80::1".to_string(), 8080);
        assert_eq!(peer2.get_addr(), "[fe80::1]:8080");

        let peer3 = Peer::new("::1".to_string(), 6882);
        assert_eq!(peer3.get_addr(), "[::1]:6882");
    }

    #[test]
    fn test_peer_ipv6_full_address_formatting() {
        // Full IPv6 address (not compressed)
        let peer = Peer::new("2001:0db8:0000:0000:0000:0000:0000:0001".to_string(), 6881);
        assert_eq!(peer.get_addr(), "[2001:db8::1]:6881");
    }
}
