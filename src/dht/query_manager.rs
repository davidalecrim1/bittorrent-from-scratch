use super::types::{KrpcMessage, NodeId};
use anyhow::Result;
use log::debug;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryType {
    Ping,
    FindNode,
    GetPeers,
    AnnouncePeer,
}

pub struct PendingQuery {
    pub transaction_id: Vec<u8>,
    pub query_type: QueryType,
    pub target: Option<NodeId>,
    pub sent_to: SocketAddr,
    pub sent_at: Instant,
    pub response_tx: oneshot::Sender<KrpcMessage>,
}

pub struct QueryManager {
    pending_queries: HashMap<Vec<u8>, PendingQuery>,
    transaction_counter: AtomicU32,
    timeout: Duration,
}

impl Default for QueryManager {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryManager {
    /// Creates a new QueryManager with default 5-second timeout.
    pub fn new() -> Self {
        Self {
            pending_queries: HashMap::new(),
            transaction_counter: AtomicU32::new(0),
            timeout: DEFAULT_QUERY_TIMEOUT,
        }
    }

    /// Creates a new QueryManager with a custom timeout duration.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            pending_queries: HashMap::new(),
            transaction_counter: AtomicU32::new(0),
            timeout,
        }
    }

    /// Generates a unique 4-byte transaction ID for tracking queries.
    pub fn generate_transaction_id(&self) -> Vec<u8> {
        let counter = self.transaction_counter.fetch_add(1, Ordering::SeqCst);
        counter.to_be_bytes().to_vec()
    }

    /// Registers a new query and returns a transaction ID with a receiver for the response.
    pub fn register_query(
        &mut self,
        query_type: QueryType,
        target: Option<NodeId>,
        addr: SocketAddr,
    ) -> (Vec<u8>, oneshot::Receiver<KrpcMessage>) {
        let transaction_id = self.generate_transaction_id();
        let (tx, rx) = oneshot::channel();

        let pending = PendingQuery {
            transaction_id: transaction_id.clone(),
            query_type,
            target,
            sent_to: addr,
            sent_at: Instant::now(),
            response_tx: tx,
        };

        self.pending_queries.insert(transaction_id.clone(), pending);
        debug!(
            "Registered query {:?} with transaction_id {:?} to {}",
            query_type,
            hex::encode(&transaction_id),
            addr
        );

        (transaction_id, rx)
    }

    /// Matches an incoming response to a pending query by transaction ID and delivers it via channel.
    pub fn handle_response(&mut self, transaction_id: &[u8], response: KrpcMessage) -> Result<()> {
        if let Some(pending) = self.pending_queries.remove(transaction_id) {
            debug!(
                "Received response for transaction_id {:?} from {}",
                hex::encode(transaction_id),
                pending.sent_to
            );
            let _ = pending.response_tx.send(response);
            Ok(())
        } else {
            debug!(
                "Received response for unknown transaction_id {:?}",
                hex::encode(transaction_id)
            );
            Ok(())
        }
    }

    /// Removes queries that have exceeded the timeout duration and returns the count removed.
    pub fn cleanup_expired(&mut self) -> usize {
        let now = Instant::now();
        let timeout = self.timeout;

        let expired: Vec<Vec<u8>> = self
            .pending_queries
            .iter()
            .filter(|(_, query)| now.duration_since(query.sent_at) > timeout)
            .map(|(id, _)| id.clone())
            .collect();

        let count = expired.len();
        for id in expired {
            if let Some(query) = self.pending_queries.remove(&id) {
                debug!(
                    "Query {:?} to {} timed out after {:?}",
                    query.query_type, query.sent_to, timeout
                );
            }
        }

        count
    }

    /// Returns the number of currently pending queries.
    pub fn pending_count(&self) -> usize {
        self.pending_queries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_transaction_id() {
        let manager = QueryManager::new();
        let id1 = manager.generate_transaction_id();
        let id2 = manager.generate_transaction_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_register_query() {
        let mut manager = QueryManager::new();
        let addr = "127.0.0.1:6881".parse().unwrap();

        let (txn_id, _rx) = manager.register_query(QueryType::Ping, None, addr);

        assert_eq!(manager.pending_count(), 1);
        assert_eq!(txn_id.len(), 4);
    }

    #[test]
    fn test_cleanup_expired() {
        let mut manager = QueryManager::with_timeout(Duration::from_millis(10));
        let addr = "127.0.0.1:6881".parse().unwrap();

        let (_txn_id, _rx) = manager.register_query(QueryType::Ping, None, addr);
        assert_eq!(manager.pending_count(), 1);

        std::thread::sleep(Duration::from_millis(20));

        let expired = manager.cleanup_expired();
        assert_eq!(expired, 1);
        assert_eq!(manager.pending_count(), 0);
    }

    #[test]
    fn test_handle_response() {
        use super::super::types::Response;

        let mut manager = QueryManager::new();
        let addr = "127.0.0.1:6881".parse().unwrap();

        let (txn_id, mut rx) = manager.register_query(QueryType::Ping, None, addr);

        let response = KrpcMessage::Response {
            transaction_id: txn_id.clone(),
            response: Response::Ping {
                id: NodeId::new([0x42; 20]),
            },
        };

        manager.handle_response(&txn_id, response).unwrap();
        assert_eq!(manager.pending_count(), 0);

        assert!(rx.try_recv().is_ok());
    }
}
