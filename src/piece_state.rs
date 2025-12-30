use anyhow::{Result, anyhow};
use log::debug;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::types::PieceDownloadRequest;

/// Maximum time a piece can be in-flight before being considered stale
pub const PIECE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

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
}

/// Manages all piece state with atomic transitions
///
/// Safe to share via Arc<PieceStateManager> without exposing lock
#[derive(Clone)]
pub struct PieceStateManager {
    inner: Arc<RwLock<PieceStateInner>>,
}

impl Default for PieceStateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PieceStateManager {
    /// Create new empty manager (to be initialized later)
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(PieceStateInner {
                states: HashMap::new(),
                pending_queue: VecDeque::new(),
                completed_set: HashSet::new(),
                stats: PieceStats::default(),
            })),
        }
    }

    /// Initialize the manager with total pieces (call once after construction)
    pub async fn initialize(&self, total_pieces: usize) {
        let mut inner = self.inner.write().await;
        inner.stats.total_pieces = total_pieces;

        // Pre-allocate capacity for efficiency
        inner.states.reserve(total_pieces);
        inner.pending_queue.reserve(total_pieces);
        inner.completed_set.reserve(total_pieces);
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

    /// Update the Piece to completed (from in flight to completed state)
    pub async fn complete_piece(&self, piece_index: u32) -> Result<()> {
        let mut inner = self.inner.write().await;

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
            PieceState::Completed => {
                debug!(
                    "Piece {} already completed (race condition avoided)",
                    piece_index
                );
                Ok(())
            }
            _ => Err(anyhow!("Piece {} not in InFlight state", piece_index)),
        }
    }

    /// Atomic transition: InFlight → Pending (for retry)
    /// Returns retry count if successful, None if piece is not in InFlight state
    pub async fn retry_piece(
        &self,
        piece_index: u32,
        reason: &str,
        push_front: bool,
    ) -> Option<u32> {
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
                    "Piece {} failed ({}), retry {}",
                    piece_index, reason, new_retry_count
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_request(piece_index: u32) -> PieceDownloadRequest {
        PieceDownloadRequest {
            piece_index,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        }
    }

    #[tokio::test]
    async fn test_new_manager() {
        let mgr = PieceStateManager::new();
        mgr.initialize(100).await;
        let snapshot = mgr.get_snapshot().await;
        assert_eq!(snapshot.total_pieces, 100);
        assert_eq!(snapshot.pending_count, 0);
        assert_eq!(snapshot.in_flight_count, 0);
        assert_eq!(snapshot.completed_count, 0);
    }

    #[tokio::test]
    async fn test_queue_piece() {
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
        let request = create_test_request(5);

        mgr.queue_piece(request.clone()).await.unwrap();

        let snapshot = mgr.get_snapshot().await;
        assert_eq!(snapshot.pending_count, 1);
    }

    #[tokio::test]
    async fn test_queue_duplicate_piece_fails() {
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
        let request = create_test_request(5);

        mgr.queue_piece(request.clone()).await.unwrap();
        let result = mgr.queue_piece(request).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pop_pending() {
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
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
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
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
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(5, "peer1".to_string()).await.unwrap();

        let result = mgr.start_download(5, "peer2".to_string()).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_complete_piece() {
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(5, "peer1".to_string()).await.unwrap();

        mgr.complete_piece(5).await.unwrap();
        let snapshot = mgr.get_snapshot().await;
        assert_eq!(snapshot.in_flight_count, 0);
        assert_eq!(snapshot.completed_count, 1);
        assert!(mgr.is_completed(5).await);
    }

    #[tokio::test]
    async fn test_complete_already_completed_idempotent() {
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(5, "peer1".to_string()).await.unwrap();
        mgr.complete_piece(5).await.unwrap();

        let result = mgr.complete_piece(5).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_retry_piece_increments_retry() {
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(5, "peer1".to_string()).await.unwrap();

        let retry_count = mgr.retry_piece(5, "timeout", false).await;

        assert_eq!(retry_count, Some(1));
        let snapshot = mgr.get_snapshot().await;
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.in_flight_count, 0);
        assert_eq!(snapshot.total_retry_attempts, 1);
    }

    #[tokio::test]
    async fn test_retry_completed_piece_error() {
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
        mgr.queue_piece(create_test_request(5)).await.unwrap();
        mgr.pop_pending().await;
        mgr.start_download(5, "peer1".to_string()).await.unwrap();
        mgr.complete_piece(5).await.unwrap();

        let result = mgr.retry_piece(5, "timeout", false).await;

        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_peer_pieces() {
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;

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
        let mgr = PieceStateManager::new();
        mgr.initialize(100).await;

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
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
        mgr.queue_piece(create_test_request(1)).await.unwrap();
        mgr.queue_piece(create_test_request(2)).await.unwrap();

        mgr.pop_pending().await;
        mgr.start_download(1, "peer1".to_string()).await.unwrap();
        mgr.retry_piece(1, "queue full", true).await;

        let next = mgr.pop_pending().await;
        assert_eq!(next, Some(1));
    }

    #[tokio::test]
    async fn test_get_request() {
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
        let request = create_test_request(5);
        mgr.queue_piece(request.clone()).await.unwrap();

        let retrieved = mgr.get_request(5).await;

        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().piece_index, 5);
    }

    #[tokio::test]
    async fn test_requeue_piece() {
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
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
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
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
        let mgr = PieceStateManager::new();
        mgr.initialize(10).await;
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
        mgr.complete_piece(5).await.unwrap();

        // Completed piece should not be stale
        let stale = mgr.get_stale_downloads().await;
        assert_eq!(stale.len(), 0);
    }
}
