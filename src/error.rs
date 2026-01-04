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

    #[error("Peer write error: {0}")]
    PeerWriteError(String),

    #[error("Peer read error: {0}")]
    PeerReadError(String),

    #[error("Peer stream closed")]
    PeerStreamClosed,

    #[error("Peer inbound channel closed")]
    PeerInboundChannelClosed,

    #[error("Peer download request channel closed")]
    PeerDownloadRequestChannelClosed,

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

impl Clone for AppError {
    fn clone(&self) -> Self {
        match self {
            // Cloneable variants
            AppError::InvalidBencode(s) => AppError::InvalidBencode(s.clone()),
            AppError::MissingField(s) => AppError::MissingField(s.clone()),
            AppError::InvalidFieldType { field, expected } => AppError::InvalidFieldType {
                field: field.clone(),
                expected: expected.clone(),
            },
            AppError::HexDecoding(s) => AppError::HexDecoding(s.clone()),
            AppError::InvalidPieceHash => AppError::InvalidPieceHash,
            AppError::TrackerRejected(s) => AppError::TrackerRejected(s.clone()),
            AppError::NoPeersAvailable => AppError::NoPeersAvailable,
            AppError::HandshakeFailed(s) => AppError::HandshakeFailed(s.clone()),
            AppError::PeerWriteError(s) => AppError::PeerWriteError(s.clone()),
            AppError::PeerReadError(s) => AppError::PeerReadError(s.clone()),
            AppError::PeerStreamClosed => AppError::PeerStreamClosed,
            AppError::PeerInboundChannelClosed => AppError::PeerInboundChannelClosed,
            AppError::PeerDownloadRequestChannelClosed => {
                AppError::PeerDownloadRequestChannelClosed
            }
            AppError::HashVerificationFailed { piece_index } => AppError::HashVerificationFailed {
                piece_index: *piece_index,
            },
            AppError::IncompletePiece(idx) => AppError::IncompletePiece(*idx),
            AppError::PieceDownloadFailed {
                piece_index,
                attempts,
            } => AppError::PieceDownloadFailed {
                piece_index: *piece_index,
                attempts: *attempts,
            },
            AppError::PieceAlreadyDownloading => AppError::PieceAlreadyDownloading,
            AppError::PeerQueueFull => AppError::PeerQueueFull,
            AppError::PeerDoesNotHavePiece => AppError::PeerDoesNotHavePiece,
            AppError::PeerNotReady {
                choking,
                bitfield_empty,
            } => AppError::PeerNotReady {
                choking: *choking,
                bitfield_empty: *bitfield_empty,
            },
            AppError::HashMismatch => AppError::HashMismatch,
            AppError::DownloadTimeout => AppError::DownloadTimeout,
            AppError::ChannelSend(s) => AppError::ChannelSend(s.clone()),
            AppError::ArrayConversion(s) => AppError::ArrayConversion(s.clone()),

            // Non-cloneable variants - convert to string
            AppError::Codec(e) => AppError::Other(anyhow::anyhow!("{}", e)),
            AppError::Io(e) => AppError::Other(anyhow::anyhow!("{}", e)),
            AppError::HttpRequest(e) => AppError::Other(anyhow::anyhow!("{}", e)),
            AppError::Other(e) => AppError::Other(anyhow::anyhow!("{}", e)),
        }
    }
}

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
