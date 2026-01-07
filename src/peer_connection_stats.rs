use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Instant;

/// Peer connection statistics (tracks connected peers and choke state)
#[derive(Debug)]
pub struct PeerConnectionStats {
    total_peers: AtomicUsize,
    choking_peers: AtomicUsize,
    last_choke_time: Mutex<Option<Instant>>,
    last_unchoke_time: Mutex<Option<Instant>>,
}

impl Default for PeerConnectionStats {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerConnectionStats {
    pub fn new() -> Self {
        Self {
            total_peers: AtomicUsize::new(0),
            choking_peers: AtomicUsize::new(0),
            last_choke_time: Mutex::new(None),
            last_unchoke_time: Mutex::new(None),
        }
    }

    pub fn increment_peers(&self) {
        self.total_peers.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_peers(&self) {
        self.total_peers.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn increment_choking(&self) {
        self.choking_peers.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut time) = self.last_choke_time.try_lock() {
            *time = Some(Instant::now());
        }
    }

    pub fn decrement_choking(&self) {
        self.choking_peers.fetch_sub(1, Ordering::Relaxed);
        if let Ok(mut time) = self.last_unchoke_time.try_lock() {
            *time = Some(Instant::now());
        }
    }

    pub fn get_stats(&self) -> (usize, usize) {
        (
            self.total_peers.load(Ordering::Relaxed),
            self.choking_peers.load(Ordering::Relaxed),
        )
    }

    pub fn get_last_choke_time(&self) -> Option<Instant> {
        self.last_choke_time.try_lock().ok().and_then(|t| *t)
    }

    pub fn get_last_unchoke_time(&self) -> Option<Instant> {
        self.last_unchoke_time.try_lock().ok().and_then(|t| *t)
    }
}
