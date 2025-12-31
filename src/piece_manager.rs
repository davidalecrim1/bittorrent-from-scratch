use anyhow::{Result, anyhow};
use async_trait::async_trait;
use log::{debug, error};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::RwLock;

use crate::types::PieceDownloadRequest;

/// Maximum time a piece can be in-flight before being considered stale
pub const PIECE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Trait for piece management (state + file I/O)
#[async_trait]
pub trait PieceManager: Send + Sync {
    /// Initialize with file path and metadata
    async fn initialize(
        &self,
        total_pieces: usize,
        file_path: PathBuf,
        file_size: u64,
        piece_length: usize,
    ) -> Result<()>;

    /// Queue a piece for download
    async fn queue_piece(&self, request: PieceDownloadRequest) -> Result<()>;

    /// Complete a piece - writes to disk AND updates state atomically
    async fn complete_piece(&self, piece_index: u32, data: Vec<u8>) -> Result<()>;

    /// Read a block for upload/seeding
    async fn read_block(
        &self,
        piece_index: u32,
        begin: u32,
        length: u32,
    ) -> Result<Option<Vec<u8>>>;

    /// Check if piece is completed
    async fn has_piece(&self, piece_index: u32) -> bool;

    /// Pop next pending piece
    async fn pop_pending(&self) -> Option<u32>;

    /// Start download
    async fn start_download(
        &self,
        piece_index: u32,
        peer_addr: String,
    ) -> Option<PieceDownloadRequest>;

    /// Retry piece
    async fn retry_piece(&self, piece_index: u32, push_front: bool) -> Option<u32>;

    /// Get statistics snapshot
    async fn get_snapshot(&self) -> PieceStats;

    /// Check if piece is in pending state
    async fn is_pending(&self, piece_index: u32) -> bool;

    /// Check if piece is in in-flight state
    async fn is_in_flight(&self, piece_index: u32) -> bool;

    /// Check if piece is completed
    async fn is_completed(&self, piece_index: u32) -> bool;

    /// Get pieces assigned to a peer
    async fn get_peer_pieces(&self, peer_addr: &str) -> Vec<u32>;

    /// Get request for a piece
    async fn get_request(&self, piece_index: u32) -> Option<PieceDownloadRequest>;

    /// Get peer address for a piece
    async fn get_peer_for_piece(&self, piece_index: u32) -> Option<String>;

    /// Get retry count for a piece
    async fn get_retry_count(&self, piece_index: u32) -> Option<u32>;

    /// Get stale downloads
    async fn get_stale_downloads(&self) -> Vec<(u32, String)>;

    /// Re-queue a piece
    async fn requeue_piece(&self, piece_index: u32);

    /// Verify completed pieces
    async fn verify_completed_pieces(&self, expected_hashes: &[[u8; 20]]) -> Result<Vec<u32>>;

    /// Get bitfield representing which pieces we have
    async fn get_bitfield(&self) -> Option<Vec<bool>>;
}

/// Represents the lifecycle state of a single piece
#[derive(Debug, Clone)]
pub enum PieceState {
    /// Piece is queued for download, not yet assigned to a peer
    Pending {
        request: PieceDownloadRequest,
        retry_count: u32,
    },

    /// Piece is currently being downloaded by a specific peer
    InFlight {
        peer_addr: String,
        request: PieceDownloadRequest,
        retry_count: u32,
        started_at: Instant,
    },

    /// Piece has been successfully downloaded and verified
    Completed,
}

/// Statistics snapshot for atomic progress reporting
#[derive(Debug, Clone, Default)]
pub struct PieceStats {
    pub total_pieces: usize,
    pub pending_count: usize,
    pub in_flight_count: usize,
    pub completed_count: usize,
    pub total_retry_attempts: usize,
}

struct PieceStateInner {
    /// Authoritative state for all pieces (key = piece_index)
    states: HashMap<u32, PieceState>,

    /// FIFO queue of pending piece indices (optimization for pop_front)
    pending_queue: VecDeque<u32>,

    /// Fast lookup for completed pieces (prevents re-queuing)
    completed_set: HashSet<u32>,

    /// Metrics for atomic snapshots
    stats: PieceStats,

    /// File I/O components
    file: Option<File>,
    file_path: Option<PathBuf>,
    file_size: u64,
    piece_length: usize,
}

/// Manages all piece state with atomic transitions
///
/// Safe to share via Arc<FilePieceManager> without exposing lock
#[derive(Clone)]
pub struct FilePieceManager {
    inner: Arc<RwLock<PieceStateInner>>,
}

impl Default for FilePieceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl FilePieceManager {
    /// Create new empty manager (to be initialized later)
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(PieceStateInner {
                states: HashMap::new(),
                pending_queue: VecDeque::new(),
                completed_set: HashSet::new(),
                stats: PieceStats::default(),
                file: None,
                file_path: None,
                file_size: 0,
                piece_length: 0,
            })),
        }
    }

    /// Initialize the manager with total pieces and file (call once after construction)
    pub async fn initialize(
        &self,
        total_pieces: usize,
        file_path: PathBuf,
        file_size: u64,
        piece_length: usize,
    ) -> Result<()> {
        let mut inner = self.inner.write().await;

        // Create and pre-allocate file
        let file = File::create(&file_path).await?;
        file.set_len(file_size).await?; // Prevents fragmentation

        // Store metadata
        inner.file = Some(file);
        inner.file_path = Some(file_path);
        inner.file_size = file_size;
        inner.piece_length = piece_length;
        inner.stats.total_pieces = total_pieces;

        // Pre-allocate capacity for efficiency
        inner.states.reserve(total_pieces);
        inner.pending_queue.reserve(total_pieces);
        inner.completed_set.reserve(total_pieces);

        Ok(())
    }

    /// Add piece to pending queue
    pub async fn queue_piece(&self, request: PieceDownloadRequest) -> Result<()> {
        let mut inner = self.inner.write().await;

        let piece_index = request.piece_index;
        if inner.states.contains_key(&piece_index) {
            return Err(anyhow!("Piece {} already exists", piece_index));
        }

        inner.states.insert(
            piece_index,
            PieceState::Pending {
                request,
                retry_count: 0,
            },
        );
        inner.pending_queue.push_back(piece_index);
        inner.stats.pending_count += 1;

        Ok(())
    }

    /// Pop next pending piece index
    pub async fn pop_pending(&self) -> Option<u32> {
        let mut inner = self.inner.write().await;
        inner.pending_queue.pop_front()
    }

    /// Updates the piece state to downloading (from in flight to pending).
    /// Returns the download request if successful, None if piece is not in Pending state.
    pub async fn start_download(
        &self,
        piece_index: u32,
        peer_addr: String,
    ) -> Option<PieceDownloadRequest> {
        let mut inner = self.inner.write().await;

        let state = inner.states.get_mut(&piece_index)?;

        match state {
            PieceState::Pending {
                request,
                retry_count,
            } => {
                let request_clone = request.clone();
                let retry_count_val = *retry_count;

                *state = PieceState::InFlight {
                    peer_addr,
                    request: request_clone.clone(),
                    retry_count: retry_count_val,
                    started_at: Instant::now(),
                };

                inner.stats.pending_count -= 1;
                inner.stats.in_flight_count += 1;

                Some(request_clone)
            }
            _ => None,
        }
    }

    /// Complete a piece - writes to disk AND updates state atomically
    pub async fn complete_piece(&self, piece_index: u32, data: Vec<u8>) -> Result<()> {
        let mut inner = self.inner.write().await;

        // Check state first
        let is_in_flight = matches!(
            inner.states.get(&piece_index),
            Some(PieceState::InFlight { .. })
        );
        let is_completed = matches!(inner.states.get(&piece_index), Some(PieceState::Completed));

        if is_completed {
            debug!(
                "Piece {} already completed (race condition avoided)",
                piece_index
            );
            return Ok(());
        }

        if !is_in_flight {
            return Err(anyhow!("Piece {} not in InFlight state", piece_index));
        }

        // Calculate offset
        let offset = piece_index as u64 * inner.piece_length as u64;

        // Write to file
        let file = inner
            .file
            .as_mut()
            .ok_or_else(|| anyhow!("File not initialized"))?;

        file.seek(SeekFrom::Start(offset)).await?;
        file.write_all(&data).await?;
        file.flush().await?;

        // Update state AFTER successful write
        if let Some(state) = inner.states.get_mut(&piece_index) {
            *state = PieceState::Completed;
            inner.stats.in_flight_count -= 1;
            inner.stats.completed_count += 1;
            inner.completed_set.insert(piece_index);
        }

        debug!("Piece {} written to disk at offset {}", piece_index, offset);
        Ok(())
    }

    /// Atomic transition: InFlight → Pending (for retry)
    /// Returns retry count if successful, None if piece is not in InFlight state
    pub async fn retry_piece(&self, piece_index: u32, push_front: bool) -> Option<u32> {
        let mut inner = self.inner.write().await;

        let state = inner.states.get_mut(&piece_index)?;

        match state {
            PieceState::InFlight {
                request,
                retry_count,
                ..
            } => {
                let request_clone = request.clone();
                let new_retry_count = *retry_count + 1;

                *state = PieceState::Pending {
                    request: request_clone,
                    retry_count: new_retry_count,
                };

                if push_front {
                    inner.pending_queue.push_front(piece_index);
                } else {
                    inner.pending_queue.push_back(piece_index);
                }

                inner.stats.in_flight_count -= 1;
                inner.stats.pending_count += 1;
                inner.stats.total_retry_attempts += 1;

                debug!(
                    "Piece {} marked for retry (attempt {})",
                    piece_index, new_retry_count
                );
                Some(new_retry_count)
            }
            PieceState::Completed => {
                debug!(
                    "Ignoring retry for piece {} - already completed",
                    piece_index
                );
                None
            }
            _ => {
                debug!(
                    "Ignoring retry for piece {} - not in InFlight state",
                    piece_index
                );
                None
            }
        }
    }

    pub async fn get_snapshot(&self) -> PieceStats {
        let inner = self.inner.read().await;
        inner.stats.clone()
    }

    pub async fn is_completed(&self, piece_index: u32) -> bool {
        let inner = self.inner.read().await;
        inner.completed_set.contains(&piece_index)
    }

    pub async fn is_pending(&self, piece_index: u32) -> bool {
        let inner = self.inner.read().await;
        matches!(
            inner.states.get(&piece_index),
            Some(PieceState::Pending { .. })
        )
    }

    pub async fn is_in_flight(&self, piece_index: u32) -> bool {
        let inner = self.inner.read().await;
        matches!(
            inner.states.get(&piece_index),
            Some(PieceState::InFlight { .. })
        )
    }

    pub async fn get_peer_pieces(&self, peer_addr: &str) -> Vec<u32> {
        let inner = self.inner.read().await;

        inner
            .states
            .iter()
            .filter_map(|(piece_index, state)| match state {
                PieceState::InFlight {
                    peer_addr: addr, ..
                } if addr == peer_addr => Some(*piece_index),
                _ => None,
            })
            .collect()
    }

    /// Get request for a piece
    pub async fn get_request(&self, piece_index: u32) -> Option<PieceDownloadRequest> {
        let inner = self.inner.read().await;

        match inner.states.get(&piece_index)? {
            PieceState::Pending { request, .. } => Some(request.clone()),
            PieceState::InFlight { request, .. } => Some(request.clone()),
            PieceState::Completed => None,
        }
    }

    /// Get the peer address for a piece (if in-flight)
    pub async fn get_peer_for_piece(&self, piece_index: u32) -> Option<String> {
        let inner = self.inner.read().await;

        match inner.states.get(&piece_index)? {
            PieceState::InFlight { peer_addr, .. } => Some(peer_addr.clone()),
            _ => None,
        }
    }

    /// Get the retry count for a piece
    pub async fn get_retry_count(&self, piece_index: u32) -> Option<u32> {
        let inner = self.inner.read().await;

        match inner.states.get(&piece_index)? {
            PieceState::Pending { retry_count, .. } => Some(*retry_count),
            PieceState::InFlight { retry_count, .. } => Some(*retry_count),
            PieceState::Completed => None,
        }
    }

    /// Check for stale downloads that have exceeded the timeout
    /// Returns list of (piece_index, peer_addr) for pieces that should be retried
    pub async fn get_stale_downloads(&self) -> Vec<(u32, String)> {
        let inner = self.inner.read().await;
        let now = Instant::now();

        inner
            .states
            .iter()
            .filter_map(|(piece_index, state)| match state {
                PieceState::InFlight {
                    peer_addr,
                    started_at,
                    ..
                } if now.duration_since(*started_at) > PIECE_DOWNLOAD_TIMEOUT => {
                    Some((*piece_index, peer_addr.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// Re-queue a piece when for some reason the popped piece was not assigned.
    pub async fn requeue_piece(&self, piece_index: u32) {
        let mut inner = self.inner.write().await;

        // Only requeue if piece still exists and not in pending queue already
        if inner.states.contains_key(&piece_index) {
            inner.pending_queue.push_back(piece_index);
            inner.stats.pending_count += 1;
        }
    }

    /// Read a block for upload/seeding
    pub async fn read_block(
        &self,
        piece_index: u32,
        begin: u32,
        length: u32,
    ) -> Result<Option<Vec<u8>>> {
        let inner = self.inner.read().await;

        // Only read completed pieces
        if !inner.completed_set.contains(&piece_index) {
            return Ok(None);
        }

        let file_path = inner
            .file_path
            .as_ref()
            .ok_or_else(|| anyhow!("File not initialized"))?
            .clone();
        let piece_length = inner.piece_length;
        drop(inner); // Release lock before file I/O

        // Read without holding lock
        let mut file = File::open(&file_path).await?;
        let piece_offset = piece_index as u64 * piece_length as u64;
        let block_offset = piece_offset + begin as u64;

        file.seek(SeekFrom::Start(block_offset)).await?;
        let mut buffer = vec![0u8; length as usize];
        file.read_exact(&mut buffer).await?;

        Ok(Some(buffer))
    }

    /// Check if piece is completed
    pub async fn has_piece(&self, piece_index: u32) -> bool {
        let inner = self.inner.read().await;
        inner.completed_set.contains(&piece_index)
    }

    /// Get bitfield representing which pieces we have
    /// Returns None if we don't have any pieces
    pub async fn get_bitfield(&self) -> Option<Vec<bool>> {
        let inner = self.inner.read().await;
        let total_pieces = inner.stats.total_pieces;
        if total_pieces == 0 {
            return None;
        }

        let mut bitfield = Vec::with_capacity(total_pieces);
        for piece_idx in 0..total_pieces {
            let has_piece = inner.completed_set.contains(&(piece_idx as u32));
            bitfield.push(has_piece);
        }

        let has_any = bitfield.iter().any(|&b| b);
        if has_any { Some(bitfield) } else { None }
    }

    /// Verify all completed pieces by reading from file and checking hashes
    /// Returns indices of pieces that failed verification
    pub async fn verify_completed_pieces(&self, expected_hashes: &[[u8; 20]]) -> Result<Vec<u32>> {
        let inner = self.inner.read().await;
        let file_path = inner
            .file_path
            .as_ref()
            .ok_or_else(|| anyhow!("File not initialized"))?
            .clone();
        let piece_length = inner.piece_length;
        let file_size = inner.file_size;

        drop(inner); // Release lock

        let mut failed_pieces = Vec::new();
        let mut file = File::open(&file_path).await?;

        for (piece_index, expected_hash) in expected_hashes.iter().enumerate() {
            // Calculate piece length (last piece may be smaller)
            let offset = piece_index as u64 * piece_length as u64;
            let remaining = file_size - offset;
            let current_piece_length = std::cmp::min(piece_length as u64, remaining) as usize;

            // Read piece
            file.seek(SeekFrom::Start(offset)).await?;
            let mut piece_data = vec![0u8; current_piece_length];
            file.read_exact(&mut piece_data).await?;

            // Verify hash
            let actual_hash = Sha1::new().chain_update(&piece_data).finalize();

            if &actual_hash[..] != expected_hash {
                error!(
                    "Piece {} hash verification failed! Expected: {:?}, Got: {:?}",
                    piece_index,
                    expected_hash,
                    &actual_hash[..]
                );
                failed_pieces.push(piece_index as u32);
            }
        }

        // Re-queue failed pieces
        if !failed_pieces.is_empty() {
            let mut inner = self.inner.write().await;
            for &piece_index in &failed_pieces {
                // Move from Completed back to Pending
                if let Some(PieceState::Completed) = inner.states.get(&piece_index) {
                    // Create a new request
                    let request = PieceDownloadRequest {
                        piece_index,
                        piece_length,
                        expected_hash: expected_hashes[piece_index as usize],
                    };

                    inner.states.insert(
                        piece_index,
                        PieceState::Pending {
                            request,
                            retry_count: 0, // Reset retry count
                        },
                    );
                    inner.pending_queue.push_front(piece_index); // High priority
                    inner.completed_set.remove(&piece_index);

                    // Update stats
                    inner.stats.completed_count -= 1;
                    inner.stats.pending_count += 1;
                }
            }
        }

        Ok(failed_pieces)
    }
}

// Implement PieceManager trait for PieceManager
#[async_trait]
impl PieceManager for FilePieceManager {
    async fn initialize(
        &self,
        total_pieces: usize,
        file_path: PathBuf,
        file_size: u64,
        piece_length: usize,
    ) -> Result<()> {
        Self::initialize(self, total_pieces, file_path, file_size, piece_length).await
    }

    async fn queue_piece(&self, request: PieceDownloadRequest) -> Result<()> {
        Self::queue_piece(self, request).await
    }

    async fn complete_piece(&self, piece_index: u32, data: Vec<u8>) -> Result<()> {
        Self::complete_piece(self, piece_index, data).await
    }

    async fn read_block(
        &self,
        piece_index: u32,
        begin: u32,
        length: u32,
    ) -> Result<Option<Vec<u8>>> {
        Self::read_block(self, piece_index, begin, length).await
    }

    async fn has_piece(&self, piece_index: u32) -> bool {
        Self::has_piece(self, piece_index).await
    }

    async fn pop_pending(&self) -> Option<u32> {
        Self::pop_pending(self).await
    }

    async fn start_download(
        &self,
        piece_index: u32,
        peer_addr: String,
    ) -> Option<PieceDownloadRequest> {
        Self::start_download(self, piece_index, peer_addr).await
    }

    async fn retry_piece(&self, piece_index: u32, push_front: bool) -> Option<u32> {
        Self::retry_piece(self, piece_index, push_front).await
    }

    async fn get_snapshot(&self) -> PieceStats {
        Self::get_snapshot(self).await
    }

    async fn is_pending(&self, piece_index: u32) -> bool {
        Self::is_pending(self, piece_index).await
    }

    async fn is_in_flight(&self, piece_index: u32) -> bool {
        Self::is_in_flight(self, piece_index).await
    }

    async fn is_completed(&self, piece_index: u32) -> bool {
        Self::is_completed(self, piece_index).await
    }

    async fn get_peer_pieces(&self, peer_addr: &str) -> Vec<u32> {
        Self::get_peer_pieces(self, peer_addr).await
    }

    async fn get_request(&self, piece_index: u32) -> Option<PieceDownloadRequest> {
        Self::get_request(self, piece_index).await
    }

    async fn get_peer_for_piece(&self, piece_index: u32) -> Option<String> {
        Self::get_peer_for_piece(self, piece_index).await
    }

    async fn get_retry_count(&self, piece_index: u32) -> Option<u32> {
        Self::get_retry_count(self, piece_index).await
    }

    async fn get_stale_downloads(&self) -> Vec<(u32, String)> {
        Self::get_stale_downloads(self).await
    }

    async fn requeue_piece(&self, piece_index: u32) {
        Self::requeue_piece(self, piece_index).await
    }

    async fn verify_completed_pieces(&self, expected_hashes: &[[u8; 20]]) -> Result<Vec<u32>> {
        Self::verify_completed_pieces(self, expected_hashes).await
    }

    async fn get_bitfield(&self) -> Option<Vec<bool>> {
        FilePieceManager::get_bitfield(self).await
    }
}

/// In-memory implementation of PieceManager for testing.
/// Stores pieces in memory instead of writing to disk.
pub struct InMemoryPieceManager {
    state: FilePieceManager,
    pieces: Arc<RwLock<HashMap<u32, Vec<u8>>>>,
}

impl Default for InMemoryPieceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryPieceManager {
    pub fn new() -> Self {
        Self {
            state: FilePieceManager::new(),
            pieces: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl PieceManager for InMemoryPieceManager {
    async fn initialize(
        &self,
        total_pieces: usize,
        _file_path: PathBuf,
        _file_size: u64,
        piece_length: usize,
    ) -> Result<()> {
        let mut inner = self.state.inner.write().await;
        inner.stats.total_pieces = total_pieces;
        inner.piece_length = piece_length;
        inner.states.reserve(total_pieces);
        inner.pending_queue.reserve(total_pieces);
        inner.completed_set.reserve(total_pieces);
        Ok(())
    }

    async fn complete_piece(&self, piece_index: u32, data: Vec<u8>) -> Result<()> {
        self.pieces.write().await.insert(piece_index, data);

        let mut inner = self.state.inner.write().await;
        let state = inner
            .states
            .get_mut(&piece_index)
            .ok_or_else(|| anyhow!("Piece {} not found", piece_index))?;

        match state {
            PieceState::InFlight { .. } => {
                *state = PieceState::Completed;
                inner.stats.in_flight_count -= 1;
                inner.stats.completed_count += 1;
                inner.completed_set.insert(piece_index);
                Ok(())
            }
            PieceState::Completed => Ok(()),
            _ => Err(anyhow!("Piece {} not in InFlight state", piece_index)),
        }
    }

    async fn read_block(
        &self,
        piece_index: u32,
        begin: u32,
        length: u32,
    ) -> Result<Option<Vec<u8>>> {
        let inner = self.state.inner.read().await;

        if !inner.completed_set.contains(&piece_index) {
            return Ok(None);
        }

        drop(inner);

        let pieces = self.pieces.read().await;
        let piece_data = pieces
            .get(&piece_index)
            .ok_or_else(|| anyhow!("Piece {} not found in storage", piece_index))?;

        let start = begin as usize;
        let end = std::cmp::min(start + length as usize, piece_data.len());

        if start >= piece_data.len() {
            return Ok(None);
        }

        Ok(Some(piece_data[start..end].to_vec()))
    }

    async fn has_piece(&self, piece_index: u32) -> bool {
        self.state.has_piece(piece_index).await
    }

    async fn verify_completed_pieces(&self, expected_hashes: &[[u8; 20]]) -> Result<Vec<u32>> {
        let mut failed_pieces = Vec::new();

        for (piece_index, expected_hash) in expected_hashes.iter().enumerate() {
            let pieces = self.pieces.read().await;

            if let Some(piece_data) = pieces.get(&(piece_index as u32)) {
                let actual_hash = Sha1::new().chain_update(piece_data).finalize();

                if &actual_hash[..] != expected_hash {
                    error!("Piece {} hash verification failed!", piece_index);
                    failed_pieces.push(piece_index as u32);
                }
            }
        }

        if !failed_pieces.is_empty() {
            let mut inner = self.state.inner.write().await;
            for &piece_index in &failed_pieces {
                if let Some(PieceState::Completed) = inner.states.get(&piece_index) {
                    let request = PieceDownloadRequest {
                        piece_index,
                        piece_length: inner.piece_length,
                        expected_hash: expected_hashes[piece_index as usize],
                    };

                    inner.states.insert(
                        piece_index,
                        PieceState::Pending {
                            request,
                            retry_count: 0,
                        },
                    );
                    inner.pending_queue.push_front(piece_index);
                    inner.completed_set.remove(&piece_index);

                    inner.stats.completed_count -= 1;
                    inner.stats.pending_count += 1;
                }
            }
        }

        Ok(failed_pieces)
    }

    async fn queue_piece(&self, request: PieceDownloadRequest) -> Result<()> {
        self.state.queue_piece(request).await
    }

    async fn pop_pending(&self) -> Option<u32> {
        self.state.pop_pending().await
    }

    async fn start_download(
        &self,
        piece_index: u32,
        peer_addr: String,
    ) -> Option<PieceDownloadRequest> {
        self.state.start_download(piece_index, peer_addr).await
    }

    async fn retry_piece(&self, piece_index: u32, push_front: bool) -> Option<u32> {
        self.state.retry_piece(piece_index, push_front).await
    }

    async fn get_snapshot(&self) -> PieceStats {
        self.state.get_snapshot().await
    }

    async fn is_pending(&self, piece_index: u32) -> bool {
        self.state.is_pending(piece_index).await
    }

    async fn is_in_flight(&self, piece_index: u32) -> bool {
        self.state.is_in_flight(piece_index).await
    }

    async fn is_completed(&self, piece_index: u32) -> bool {
        self.state.is_completed(piece_index).await
    }

    async fn get_peer_pieces(&self, peer_addr: &str) -> Vec<u32> {
        self.state.get_peer_pieces(peer_addr).await
    }

    async fn get_request(&self, piece_index: u32) -> Option<PieceDownloadRequest> {
        self.state.get_request(piece_index).await
    }

    async fn get_peer_for_piece(&self, piece_index: u32) -> Option<String> {
        self.state.get_peer_for_piece(piece_index).await
    }

    async fn get_retry_count(&self, piece_index: u32) -> Option<u32> {
        self.state.get_retry_count(piece_index).await
    }

    async fn get_stale_downloads(&self) -> Vec<(u32, String)> {
        self.state.get_stale_downloads().await
    }

    async fn requeue_piece(&self, piece_index: u32) {
        self.state.requeue_piece(piece_index).await
    }

    async fn get_bitfield(&self) -> Option<Vec<bool>> {
        self.state.get_bitfield().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn create_test_request(piece_index: u32) -> PieceDownloadRequest {
        PieceDownloadRequest {
            piece_index,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        }
    }

    async fn create_test_manager(total_pieces: usize) -> (FilePieceManager, NamedTempFile) {
        let temp_file = NamedTempFile::new().unwrap();
        let file_path = temp_file.path().to_path_buf();
        let mgr = FilePieceManager::new();
        let file_size = (total_pieces * 16384) as u64;
        mgr.initialize(total_pieces, file_path, file_size, 16384)
            .await
            .unwrap();
        (mgr, temp_file)
    }

    #[tokio::test]
    async fn test_new_manager() {
        let (mgr, _temp_file) = create_test_manager(100).await;
        let snapshot = mgr.get_snapshot().await;
        assert_eq!(snapshot.total_pieces, 100);
        assert_eq!(snapshot.pending_count, 0);
        assert_eq!(snapshot.in_flight_count, 0);
        assert_eq!(snapshot.completed_count, 0);
    }

    #[tokio::test]
    async fn test_queue_piece() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        let request = create_test_request(5);

        mgr.queue_piece(request.clone()).await.unwrap();

        let snapshot = mgr.get_snapshot().await;
        assert_eq!(snapshot.pending_count, 1);
    }

    #[tokio::test]
    async fn test_queue_duplicate_piece_fails() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        let request = create_test_request(5);

        mgr.queue_piece(request.clone()).await.unwrap();
        let result = mgr.queue_piece(request).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pop_pending() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.queue_piece(create_test_request(7)).await.unwrap();

        let popped1 = mgr.pop_pending().await;
        let popped2 = mgr.pop_pending().await;
        let popped3 = mgr.pop_pending().await;

        assert_eq!(popped1, Some(5));
        assert_eq!(popped2, Some(7));
        assert_eq!(popped3, None);
    }

    #[tokio::test]
    async fn test_start_download_success() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;

        let result = mgr.start_download(5, "192.168.1.1:6881".to_string()).await;

        assert!(result.is_some());
        let snapshot = mgr.get_snapshot().await;
        assert_eq!(snapshot.pending_count, 0);
        assert_eq!(snapshot.in_flight_count, 1);
    }

    #[tokio::test]
    async fn test_start_download_already_in_flight() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(5, "peer1".to_string()).await.unwrap();

        let result = mgr.start_download(5, "peer2".to_string()).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_complete_piece() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(5, "peer1".to_string()).await.unwrap();

        mgr.complete_piece(5, vec![0u8; 16384]).await.unwrap();
        let snapshot = mgr.get_snapshot().await;
        assert_eq!(snapshot.in_flight_count, 0);
        assert_eq!(snapshot.completed_count, 1);
        assert!(mgr.is_completed(5).await);
    }

    #[tokio::test]
    async fn test_complete_already_completed_idempotent() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(5, "peer1".to_string()).await.unwrap();
        mgr.complete_piece(5, vec![0u8; 16384]).await.unwrap();

        let result = mgr.complete_piece(5, vec![0u8; 16384]).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_retry_piece_increments_retry() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(5, "peer1".to_string()).await.unwrap();

        let retry_count = mgr.retry_piece(5, false).await;

        assert_eq!(retry_count, Some(1));
        let snapshot = mgr.get_snapshot().await;
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.in_flight_count, 0);
        assert_eq!(snapshot.total_retry_attempts, 1);
    }

    #[tokio::test]
    async fn test_retry_completed_piece_error() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(5, "peer1".to_string()).await.unwrap();
        mgr.complete_piece(5, vec![0u8; 16384]).await.unwrap();

        let result = mgr.retry_piece(5, false).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_peer_pieces() {
        let (mgr, _temp_file) = create_test_manager(10).await;

        for i in 0..3 {
            mgr.queue_piece(create_test_request(i)).await.unwrap();
            mgr.pop_pending().await;
        }

        mgr.start_download(0, "peer1".to_string()).await.unwrap();
        mgr.start_download(1, "peer1".to_string()).await.unwrap();
        mgr.start_download(2, "peer2".to_string()).await.unwrap();

        let peer1_pieces = mgr.get_peer_pieces("peer1").await;

        assert_eq!(peer1_pieces.len(), 2);
        assert!(peer1_pieces.contains(&0));
        assert!(peer1_pieces.contains(&1));
    }

    #[tokio::test]
    async fn test_atomic_snapshot() {
        let (mgr, _temp_file) = create_test_manager(100).await;

        for i in 0..10 {
            mgr.queue_piece(create_test_request(i)).await.unwrap();
        }

        let snapshot = mgr.get_snapshot().await;

        assert_eq!(snapshot.total_pieces, 100);
        assert_eq!(snapshot.pending_count, 10);
        assert_eq!(snapshot.in_flight_count, 0);
        assert_eq!(snapshot.completed_count, 0);
    }

    #[tokio::test]
    async fn test_retry_piece_push_front() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(1)).await.unwrap();
        mgr.queue_piece(create_test_request(2)).await.unwrap();

        mgr.pop_pending().await;
        mgr.start_download(1, "peer1".to_string()).await.unwrap();
        mgr.retry_piece(1, true).await;

        let next = mgr.pop_pending().await;
        assert_eq!(next, Some(1));
    }

    #[tokio::test]
    async fn test_get_request() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        let request = create_test_request(5);
        mgr.queue_piece(request.clone()).await.unwrap();

        let retrieved = mgr.get_request(5).await;

        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().piece_index, 5);
    }

    #[tokio::test]
    async fn test_requeue_piece() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();

        // Pop the piece index from queue
        let piece_index = mgr.pop_pending().await.unwrap();

        let snapshot1 = mgr.get_snapshot().await;
        // Piece still exists in states but not in queue
        // pending_count was decremented when we transitioned away from Pending state
        // But pop_pending doesn't change state, so count is still 1
        assert_eq!(snapshot1.pending_count, 1);

        // Requeue it (used in rollback scenarios)
        mgr.requeue_piece(piece_index).await;

        let snapshot2 = mgr.get_snapshot().await;
        // Now pending_count should be 2 (piece is back in queue)
        assert_eq!(snapshot2.pending_count, 2);

        // Verify we can pop it again
        let popped_again = mgr.pop_pending().await;
        assert_eq!(popped_again, Some(5));
    }

    #[tokio::test]
    async fn test_stale_downloads_detection() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.queue_piece(create_test_request(7)).await.unwrap();
        mgr.pop_pending().await;
        mgr.pop_pending().await;

        // Manually create old in-flight states by directly modifying internal state
        let old_instant = Instant::now() - PIECE_DOWNLOAD_TIMEOUT - Duration::from_secs(1);
        {
            let mut inner = mgr.inner.write().await;
            inner.states.insert(
                5,
                PieceState::InFlight {
                    peer_addr: "peer1".to_string(),
                    request: create_test_request(5),
                    retry_count: 0,
                    started_at: old_instant,
                },
            );
            inner.states.insert(
                7,
                PieceState::InFlight {
                    peer_addr: "peer2".to_string(),
                    request: create_test_request(7),
                    retry_count: 0,
                    started_at: old_instant,
                },
            );
            inner.stats.in_flight_count = 2;
        }

        // Both downloads should be stale
        let stale = mgr.get_stale_downloads().await;
        assert_eq!(stale.len(), 2);
        assert!(stale.contains(&(5, "peer1".to_string())));
        assert!(stale.contains(&(7, "peer2".to_string())));
    }

    #[tokio::test]
    async fn test_completed_pieces_not_stale() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;

        // Manually create old in-flight state
        let old_instant = Instant::now() - PIECE_DOWNLOAD_TIMEOUT - Duration::from_secs(1);
        {
            let mut inner = mgr.inner.write().await;
            inner.states.insert(
                5,
                PieceState::InFlight {
                    peer_addr: "peer1".to_string(),
                    request: create_test_request(5),
                    retry_count: 0,
                    started_at: old_instant,
                },
            );
            inner.stats.pending_count = 0;
            inner.stats.in_flight_count = 1;
        }

        // Complete the piece
        mgr.complete_piece(5, vec![0u8; 16384]).await.unwrap();

        // Completed piece should not be stale
        let stale = mgr.get_stale_downloads().await;
        assert_eq!(stale.len(), 0);
    }

    #[tokio::test]
    async fn test_read_block_from_completed_piece() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(0)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(0, "peer1".to_string()).await;

        let piece_data = vec![42u8; 16384];
        mgr.complete_piece(0, piece_data.clone()).await.unwrap();

        let block = mgr.read_block(0, 0, 1024).await.unwrap();
        assert!(block.is_some());
        let block_data = block.unwrap();
        assert_eq!(block_data.len(), 1024);
        assert_eq!(block_data, vec![42u8; 1024]);
    }

    #[tokio::test]
    async fn test_read_block_from_incomplete_piece_returns_none() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(0)).await.unwrap();

        let block = mgr.read_block(0, 0, 1024).await.unwrap();
        assert!(block.is_none());
    }

    #[tokio::test]
    async fn test_read_block_with_offset() {
        let (mgr, _temp_file) = create_test_manager(10).await;
        mgr.queue_piece(create_test_request(0)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(0, "peer1".to_string()).await;

        let mut piece_data = vec![0u8; 16384];
        for i in 0..16384 {
            piece_data[i] = (i % 256) as u8;
        }
        mgr.complete_piece(0, piece_data.clone()).await.unwrap();

        let block = mgr.read_block(0, 1000, 500).await.unwrap();
        assert!(block.is_some());
        let block_data = block.unwrap();
        assert_eq!(block_data.len(), 500);
        assert_eq!(block_data, &piece_data[1000..1500]);
    }

    #[tokio::test]
    async fn test_verify_completed_pieces_with_correct_hash() {
        use sha1::{Digest, Sha1};

        let (mgr, _temp_file) = create_test_manager(3).await;

        let mut expected_hashes = Vec::new();
        for i in 0..3 {
            let piece_data = vec![(i * 10) as u8; 16384];
            let hash = Sha1::new().chain_update(&piece_data).finalize();
            let hash_array: [u8; 20] = hash.into();
            expected_hashes.push(hash_array);

            mgr.queue_piece(PieceDownloadRequest {
                piece_index: i,
                piece_length: 16384,
                expected_hash: hash_array,
            })
            .await
            .unwrap();

            mgr.pop_pending().await;
            mgr.start_download(i, "peer1".to_string()).await;
            mgr.complete_piece(i, piece_data).await.unwrap();
        }

        let failed = mgr.verify_completed_pieces(&expected_hashes).await.unwrap();
        assert_eq!(failed.len(), 0, "All pieces should verify correctly");
    }

    #[tokio::test]
    async fn test_verify_completed_pieces_with_wrong_hash() {
        use sha1::{Digest, Sha1};

        let (mgr, _temp_file) = create_test_manager(2).await;

        let piece_data_0 = vec![10u8; 16384];
        let hash_0 = Sha1::new().chain_update(&piece_data_0).finalize();
        let hash_array_0: [u8; 20] = hash_0.into();

        mgr.queue_piece(PieceDownloadRequest {
            piece_index: 0,
            piece_length: 16384,
            expected_hash: hash_array_0,
        })
        .await
        .unwrap();

        mgr.pop_pending().await;
        mgr.start_download(0, "peer1".to_string()).await;
        mgr.complete_piece(0, piece_data_0).await.unwrap();

        let wrong_piece_data = vec![99u8; 16384];
        let wrong_hash = [0u8; 20];

        mgr.queue_piece(PieceDownloadRequest {
            piece_index: 1,
            piece_length: 16384,
            expected_hash: wrong_hash,
        })
        .await
        .unwrap();

        mgr.pop_pending().await;
        mgr.start_download(1, "peer1".to_string()).await;
        mgr.complete_piece(1, wrong_piece_data).await.unwrap();

        let expected_hashes = vec![hash_array_0, wrong_hash];
        let failed = mgr.verify_completed_pieces(&expected_hashes).await.unwrap();

        assert_eq!(failed.len(), 1, "One piece should fail verification");
        assert!(failed.contains(&1));

        assert!(mgr.is_pending(1).await, "Failed piece should be requeued");
    }

    #[tokio::test]
    async fn test_in_memory_piece_manager_read_block() {
        let mgr = InMemoryPieceManager::new();
        mgr.initialize(10, PathBuf::from("/tmp/test"), 163840, 16384)
            .await
            .unwrap();

        mgr.queue_piece(create_test_request(0)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(0, "peer1".to_string()).await;

        let piece_data = vec![42u8; 16384];
        mgr.complete_piece(0, piece_data.clone()).await.unwrap();

        let block = mgr.read_block(0, 100, 500).await.unwrap();
        assert!(block.is_some());
        assert_eq!(block.unwrap().len(), 500);
    }

    #[tokio::test]
    async fn test_in_memory_piece_manager_read_block_incomplete_returns_none() {
        let mgr = InMemoryPieceManager::new();
        mgr.initialize(10, PathBuf::from("/tmp/test"), 163840, 16384)
            .await
            .unwrap();

        mgr.queue_piece(create_test_request(0)).await.unwrap();

        let block = mgr.read_block(0, 0, 500).await.unwrap();
        assert!(block.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_piece_manager_verify_failed_pieces() {
        use sha1::{Digest, Sha1};

        let mgr = InMemoryPieceManager::new();
        mgr.initialize(2, PathBuf::from("/tmp/test"), 32768, 16384)
            .await
            .unwrap();

        let good_data = vec![10u8; 16384];
        let good_hash = Sha1::new().chain_update(&good_data).finalize();
        let good_hash_array: [u8; 20] = good_hash.into();

        mgr.queue_piece(PieceDownloadRequest {
            piece_index: 0,
            piece_length: 16384,
            expected_hash: good_hash_array,
        })
        .await
        .unwrap();
        mgr.pop_pending().await;
        mgr.start_download(0, "peer1".to_string()).await;
        mgr.complete_piece(0, good_data).await.unwrap();

        let bad_data = vec![99u8; 16384];
        let wrong_hash = [0u8; 20];

        mgr.queue_piece(PieceDownloadRequest {
            piece_index: 1,
            piece_length: 16384,
            expected_hash: wrong_hash,
        })
        .await
        .unwrap();
        mgr.pop_pending().await;
        mgr.start_download(1, "peer1".to_string()).await;
        mgr.complete_piece(1, bad_data).await.unwrap();

        let expected_hashes = vec![good_hash_array, wrong_hash];
        let failed = mgr.verify_completed_pieces(&expected_hashes).await.unwrap();

        assert_eq!(failed.len(), 1);
        assert!(failed.contains(&1));
        assert!(mgr.is_pending(1).await);
    }
}
