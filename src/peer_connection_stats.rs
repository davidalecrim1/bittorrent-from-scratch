use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::peer::PeerAddr;

#[derive(Debug, Clone)]
struct PeerChokeState {
    is_choking: bool,
    last_choke_time: Option<Instant>,
    last_unchoke_time: Option<Instant>,
}

/// Peer connection statistics (tracks connected peers and choke state)
#[derive(Debug)]
pub struct PeerConnectionStats {
    total_peers: AtomicUsize,
    choke_states: Mutex<HashMap<PeerAddr, PeerChokeState>>,
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
            choke_states: Mutex::new(HashMap::new()),
        }
    }

    pub fn increment_peers(&self) {
        self.total_peers.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_peers(&self) {
        self.total_peers.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn set_choking(&self, peer_addr: &str) {
        let mut states = self.choke_states.lock().unwrap();
        states
            .entry(peer_addr.to_string())
            .and_modify(|state| {
                state.is_choking = true;
                state.last_choke_time = Some(Instant::now());
            })
            .or_insert(PeerChokeState {
                is_choking: true,
                last_choke_time: Some(Instant::now()),
                last_unchoke_time: None,
            });
    }

    pub fn set_unchoked(&self, peer_addr: &str) {
        let mut states = self.choke_states.lock().unwrap();
        states
            .entry(peer_addr.to_string())
            .and_modify(|state| {
                state.is_choking = false;
                state.last_unchoke_time = Some(Instant::now());
            })
            .or_insert(PeerChokeState {
                is_choking: false,
                last_choke_time: None,
                last_unchoke_time: Some(Instant::now()),
            });
    }

    pub fn get_stats(&self) -> (usize, usize) {
        let total = self.total_peers.load(Ordering::Relaxed);
        let states = self.choke_states.lock().unwrap();
        let choking = states.values().filter(|s| s.is_choking).count();
        (total, choking)
    }

    pub fn is_choking(&self, peer_addr: &str) -> bool {
        self.choke_states
            .lock()
            .unwrap()
            .get(peer_addr)
            .map(|s| s.is_choking)
            .unwrap_or(false)
    }

    pub fn get_choke_time(&self, peer_addr: &str) -> Option<Instant> {
        self.choke_states
            .lock()
            .unwrap()
            .get(peer_addr)
            .and_then(|s| s.last_choke_time)
    }

    pub fn get_unchoke_time(&self, peer_addr: &str) -> Option<Instant> {
        self.choke_states
            .lock()
            .unwrap()
            .get(peer_addr)
            .and_then(|s| s.last_unchoke_time)
    }

    pub fn get_choking_peers(&self) -> Vec<PeerAddr> {
        self.choke_states
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(addr, state)| {
                if state.is_choking {
                    Some(addr.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn remove_peer(&self, peer_addr: &str) {
        self.choke_states.lock().unwrap().remove(peer_addr);
    }
}
