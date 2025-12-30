use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CodecError {
    #[error("Incomplete message: need {needed} bytes, got {available}")]
    IncompleteMessage { needed: usize, available: usize },

    #[error("Message too short: {0} bytes")]
    MessageTooShort(usize),

    #[error("Unknown message type: {0}")]
    UnknownMessageType(u8),

    #[error("Invalid message format: {0}")]
    InvalidFormat(String),
}

#[derive(Error, Debug)]
pub enum AppError {
    // Bencode/Encoding errors
    #[error("Invalid bencode format: {0}")]
    InvalidBencode(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid field type: {field}, expected {expected}")]
    InvalidFieldType { field: String, expected: String },

    #[error("Hex decoding error: {0}")]
    HexDecoding(String),

    // Codec/Protocol errors (can wrap CodecError)
    #[error(transparent)]
    Codec(#[from] CodecError),

    #[error("Invalid piece hash format")]
    InvalidPieceHash,

    // Network/Tracker errors
    #[error("Tracker rejected request: {0}")]
    TrackerRejected(String),

    #[error("No peers available")]
    NoPeersAvailable,

    #[error("Handshake failed: {0}")]
    HandshakeFailed(String),

    #[error("Peer disconnected")]
    PeerDisconnected,

    // File I/O errors
    #[error("File I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Hash verification failed for piece {piece_index}")]
    HashVerificationFailed { piece_index: usize },

    // Download errors
    #[error("Cannot assemble incomplete piece {0}")]
    IncompletePiece(u32),

    #[error("Failed to download piece {piece_index} after {attempts} attempts")]
    PieceDownloadFailed { piece_index: u32, attempts: usize },

    #[error("Piece already downloading")]
    PieceAlreadyDownloading,

    #[error("Peer queue full")]
    PeerQueueFull,

    #[error("Peer does not have piece")]
    PeerDoesNotHavePiece,

    #[error("Peer not ready: choking={choking}, bitfield_empty={bitfield_empty}")]
    PeerNotReady { choking: bool, bitfield_empty: bool },

    #[error("Hash mismatch")]
    HashMismatch,

    #[error("Download timeout exceeded")]
    DownloadTimeout,

    // Channel/async errors
    #[error("Channel send error: {0}")]
    ChannelSend(String),

    // Array conversion errors
    #[error("Array conversion error: {0}")]
    ArrayConversion(String),

    // External errors (wrapped)
    #[error("HTTP request failed: {0}")]
    HttpRequest(#[from] reqwest::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = anyhow::Result<T>;

impl AppError {
    pub fn missing_field(field: &str) -> Self {
        AppError::MissingField(field.to_string())
    }

    pub fn invalid_bencode(msg: impl Into<String>) -> Self {
        AppError::InvalidBencode(msg.into())
    }

    pub fn hex_decoding(msg: impl Into<String>) -> Self {
        AppError::HexDecoding(msg.into())
    }

    pub fn handshake_failed(msg: impl Into<String>) -> Self {
        AppError::HandshakeFailed(msg.into())
    }

    pub fn tracker_rejected(msg: impl Into<String>) -> Self {
        AppError::TrackerRejected(msg.into())
    }

    pub fn channel_send(msg: impl Into<String>) -> Self {
        AppError::ChannelSend(msg.into())
    }
}
