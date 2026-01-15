// Designed to be small and simple types used in the codebase,
// anything more complex deserves it's own module.
pub use crate::peer_connection::{PeerConnection, PieceDownloadTask};

use anyhow::{Result, anyhow};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Debug;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

use crate::error::AppError;
use crate::peer_messages::PeerMessage;

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
    pub piece_length: usize,
}

#[derive(Debug)]
pub struct ConnectedPeer {
    peer: Peer,
    download_request_tx: mpsc::Sender<PieceDownloadRequest>,
    message_tx: mpsc::Sender<PeerMessage>,
    bitfield: Arc<RwLock<HashSet<u32>>>,
}

impl ConnectedPeer {
    pub fn new(
        peer: Peer,
        download_request_tx: mpsc::Sender<PieceDownloadRequest>,
        message_tx: mpsc::Sender<crate::peer_messages::PeerMessage>,
        bitfield: Arc<RwLock<HashSet<u32>>>, // Shared with PeerConnection
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

    pub async fn request_piece(&self, request: PieceDownloadRequest) -> Result<()> {
        self.download_request_tx
            .send(request)
            .await
            .map_err(|e| anyhow!("Failed to send download request: {}", e))
    }

    pub async fn send_message(&self, msg: crate::peer_messages::PeerMessage) -> Result<()> {
        self.message_tx
            .send(msg)
            .await
            .map_err(|e| anyhow!("Failed to send message: {}", e))
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
        // Send shutdown signal to all subscribers
        let _ = self.shutdown_tx.send(());
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSource {
    Tracker,
    Dht,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Peer {
    pub ip: IpAddr,
    pub port: u16,
    pub source: PeerSource,
}

pub type PeerId = [u8; 20];
pub type PeerAddr = String;

impl Peer {
    pub fn new(ip: String, port: u16, source: PeerSource) -> Self {
        Self {
            ip: IpAddr::from_str(&ip).unwrap(),
            port,
            source,
        }
    }

    pub fn get_addr(&self) -> PeerAddr {
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
    use super::{ConnectedPeer, Peer, PeerSource, PieceDownloadRequest};
    use crate::peer_messages::BitfieldMessage;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::RwLock;

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
        let peer = Peer::new("192.168.1.100".to_string(), 6881, PeerSource::Tracker);
        assert_eq!(peer.get_addr(), "192.168.1.100:6881");
    }

    #[test]
    fn test_peer_ipv6_address_formatting() {
        // IPv6 addresses require bracket notation for socket addresses
        let peer = Peer::new("2001:db8::1".to_string(), 6881, PeerSource::Tracker);
        assert_eq!(peer.get_addr(), "[2001:db8::1]:6881");

        let peer2 = Peer::new("fe80::1".to_string(), 8080, PeerSource::Tracker);
        assert_eq!(peer2.get_addr(), "[fe80::1]:8080");

        let peer3 = Peer::new("::1".to_string(), 6882, PeerSource::Tracker);
        assert_eq!(peer3.get_addr(), "[::1]:6882");
    }

    #[test]
    fn test_peer_ipv6_full_address_formatting() {
        // Full IPv6 address (not compressed)
        let peer = Peer::new(
            "2001:0db8:0000:0000:0000:0000:0000:0001".to_string(),
            6881,
            PeerSource::Tracker,
        );
        assert_eq!(peer.get_addr(), "[2001:db8::1]:6881");
    }

    #[tokio::test]
    async fn test_connected_peer_has_piece() {
        use tokio::sync::mpsc;

        let peer = Peer::new("127.0.0.1".to_string(), 6881, PeerSource::Tracker);
        let (tx, _rx) = mpsc::channel(1);
        let (message_tx, _message_rx) = mpsc::channel(1);

        let mut bitfield_set = HashSet::new();
        bitfield_set.insert(0);
        bitfield_set.insert(2);
        let bitfield = Arc::new(RwLock::new(bitfield_set));

        let connected_peer = ConnectedPeer::new(peer, tx, message_tx, bitfield);

        assert!(connected_peer.has_piece(0).await);
        assert!(!connected_peer.has_piece(1).await);
        assert!(connected_peer.has_piece(2).await);
        assert!(!connected_peer.has_piece(3).await);
    }

    #[tokio::test]
    async fn test_connected_peer_piece_count() {
        use tokio::sync::mpsc;

        let peer = Peer::new("127.0.0.1".to_string(), 6881, PeerSource::Tracker);
        let (tx, _rx) = mpsc::channel(1);
        let (message_tx, _message_rx) = mpsc::channel(1);

        let mut bitfield_set = HashSet::new();
        bitfield_set.insert(0);
        bitfield_set.insert(2);
        bitfield_set.insert(3);
        let bitfield = Arc::new(RwLock::new(bitfield_set));

        let connected_peer = ConnectedPeer::new(peer, tx, message_tx, bitfield);

        assert_eq!(connected_peer.piece_count().await, 3);
    }

    #[tokio::test]
    async fn test_connected_peer_bitfield_len() {
        use tokio::sync::mpsc;

        let peer = Peer::new("127.0.0.1".to_string(), 6881, PeerSource::Tracker);
        let (tx, _rx) = mpsc::channel(1);
        let (message_tx, _message_rx) = mpsc::channel(1);

        let mut bitfield_set = HashSet::new();
        for i in 0..100 {
            bitfield_set.insert(i);
        }
        let bitfield = Arc::new(RwLock::new(bitfield_set));

        let connected_peer = ConnectedPeer::new(peer, tx, message_tx, bitfield);

        assert_eq!(connected_peer.bitfield_len().await, 100);
    }

    #[tokio::test]
    async fn test_connected_peer_request_piece() {
        use tokio::sync::mpsc;

        let peer = Peer::new("127.0.0.1".to_string(), 6881, PeerSource::Tracker);
        let (tx, mut rx) = mpsc::channel(1);
        let (message_tx, _message_rx) = mpsc::channel(1);

        let mut bitfield_set = HashSet::new();
        bitfield_set.insert(0);
        let bitfield = Arc::new(RwLock::new(bitfield_set));

        let connected_peer = ConnectedPeer::new(peer, tx, message_tx, bitfield);

        let request = PieceDownloadRequest {
            piece_index: 0,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        };

        connected_peer.request_piece(request.clone()).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.piece_index, 0);
    }

    #[tokio::test]
    async fn test_connected_peer_get_sender() {
        use tokio::sync::mpsc;

        let peer = Peer::new("127.0.0.1".to_string(), 6881, PeerSource::Tracker);
        let (tx, _rx) = mpsc::channel(1);
        let (message_tx, _message_rx) = mpsc::channel(1);
        let bitfield = Arc::new(RwLock::new(HashSet::new()));

        let connected_peer = ConnectedPeer::new(peer, tx.clone(), message_tx, bitfield);

        let sender = connected_peer.get_sender();
        assert_eq!(sender.capacity(), tx.capacity());
    }

    #[test]
    fn test_connected_peer_peer_addr() {
        use tokio::sync::mpsc;

        let peer = Peer::new("192.168.1.100".to_string(), 6881, PeerSource::Tracker);
        let (tx, _rx) = mpsc::channel(1);
        let (message_tx, _message_rx) = mpsc::channel(1);
        let bitfield = Arc::new(RwLock::new(HashSet::new()));

        let connected_peer = ConnectedPeer::new(peer, tx, message_tx, bitfield);

        assert_eq!(connected_peer.peer_addr(), "192.168.1.100:6881");
    }

    #[test]
    fn test_connected_peer_peer() {
        use tokio::sync::mpsc;

        let peer = Peer::new("192.168.1.100".to_string(), 6881, PeerSource::Tracker);
        let (tx, _rx) = mpsc::channel(1);
        let (message_tx, _message_rx) = mpsc::channel(1);
        let bitfield = Arc::new(RwLock::new(HashSet::new()));

        let connected_peer = ConnectedPeer::new(peer.clone(), tx, message_tx, bitfield);

        assert_eq!(connected_peer.peer(), &peer);
    }
}
