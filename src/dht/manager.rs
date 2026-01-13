use super::message_io::DhtMessageIO;
use super::query_manager::{QueryManager, QueryType};
use super::routing_table::RoutingTable;
use super::types::{CompactNodeInfo, DhtNode, KrpcMessage, NodeId, Query, Response};
use crate::types::Peer;
use anyhow::Result;
use log::{debug, info, warn};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio::time;

const BOOTSTRAP_NODES: &[&str] = &[
    "dht.libtorrent.org:25401",
    "dht.transmissionbt.com:6881",
    "router.bittorrent.com:6881",
    "router.utorrent.com:6881",
    "dht.aelitis.com:6881",
    "router.bittorrent.com:8991",
];

const K: usize = 8;
const ALPHA: usize = 3;
const MAX_ITERATIONS: usize = 8;
const BOOTSTRAP_REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
const QUERY_CLEANUP_INTERVAL: Duration = Duration::from_secs(1);

pub struct DhtManager {
    routing_table: Arc<RwLock<RoutingTable>>,
    query_manager: Arc<Mutex<QueryManager>>,
    message_io: Arc<dyn DhtMessageIO>,
    self_id: NodeId,
    bootstrap_nodes: Vec<SocketAddr>,
}

impl DhtManager {
    /// Creates a new DhtManager with a random node ID and initializes the routing table.
    pub fn new(message_io: Arc<dyn DhtMessageIO>) -> Self {
        let self_id = NodeId::random();
        let routing_table = Arc::new(RwLock::new(RoutingTable::new(self_id, K)));
        let query_manager = Arc::new(Mutex::new(QueryManager::new()));

        Self {
            routing_table,
            query_manager,
            message_io,
            self_id,
            bootstrap_nodes: Vec::new(),
        }
    }

    /// Resolves bootstrap node hostnames to IP addresses via DNS lookup.
    async fn resolve_bootstrap_nodes() -> Vec<SocketAddr> {
        let mut resolved = Vec::new();

        for hostname in BOOTSTRAP_NODES {
            match tokio::net::lookup_host(hostname).await {
                Ok(addrs) => {
                    for addr in addrs {
                        if addr.is_ipv4() {
                            debug!("Resolved bootstrap node {} to {}", hostname, addr);
                            resolved.push(addr);
                            break;
                        } else {
                            debug!("Skipping IPv6 bootstrap node {}: {}", hostname, addr);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to resolve bootstrap node {}: {}", hostname, e);
                }
            }
        }

        resolved
    }

    /// Bootstraps the DHT by querying hardcoded bootstrap nodes to populate the routing table.
    pub async fn bootstrap(&self) -> Result<()> {
        let bootstrap_nodes = Self::resolve_bootstrap_nodes().await;

        info!(
            "Bootstrapping DHT with {} bootstrap nodes",
            bootstrap_nodes.len()
        );

        let mut nodes_added = 0;

        for addr in &bootstrap_nodes {
            let query = Query::FindNode {
                id: self.self_id,
                target: self.self_id,
            };

            match self.send_query(*addr, query).await {
                Ok(Response::FindNode { nodes, .. }) => {
                    for node_info in nodes {
                        let node = DhtNode::new(node_info.id, SocketAddr::V4(node_info.addr));
                        self.routing_table.write().await.insert(node);
                        nodes_added += 1;
                    }
                }
                Ok(_) => {
                    warn!("Bootstrap from {} returned unexpected response type", addr);
                }
                Err(e) => {
                    warn!("Failed to bootstrap from {}: {}", addr, e);
                }
            }
        }

        info!(
            "DHT bootstrap complete: added {} nodes to routing table",
            nodes_added
        );
        Ok(())
    }

    /// Performs an iterative DHT lookup to find peers for the given info_hash.
    pub async fn get_peers(&self, info_hash: [u8; 20]) -> Result<Vec<Peer>> {
        self.get_peers_internal(info_hash).await
    }

    async fn get_peers_internal(&self, info_hash: [u8; 20]) -> Result<Vec<Peer>> {
        let target = NodeId::from_slice(&info_hash)?;
        let mut peers = Vec::new();

        let closest_nodes = self.routing_table.read().await.find_closest(&target, K);

        if closest_nodes.is_empty() {
            self.bootstrap().await?;
            tokio::time::sleep(Duration::from_secs(2)).await;
            // Box::pin is required for recursive async calls to avoid infinite type size.
            // Box allocates the future on the heap (fixed-size pointer instead of infinite nesting).
            // Pin guarantees the future won't move in memory (required for self-referential futures).
            return Box::pin(self.get_peers_internal(info_hash)).await;
        }

        let mut queried: HashSet<NodeId> = HashSet::new();
        let mut candidates: Vec<DhtNode> = closest_nodes;
        let mut iteration = 0;
        let mut last_closest_distance = u8::MAX;

        while iteration < MAX_ITERATIONS && !candidates.is_empty() {
            let to_query: Vec<DhtNode> = candidates
                .iter()
                .filter(|node| !queried.contains(&node.id))
                .take(ALPHA)
                .cloned()
                .collect();

            if to_query.is_empty() {
                break;
            }

            for node in &to_query {
                queried.insert(node.id);
            }

            let mut tasks = Vec::new();
            for node in to_query {
                let manager = self.clone_arc();
                let task =
                    tokio::spawn(
                        async move { manager.query_get_peers(node.addr, info_hash).await },
                    );
                tasks.push(task);
            }

            for task in tasks {
                if let Ok(Ok(result)) = task.await {
                    match result {
                        GetPeersResult::Peers(new_peers) => {
                            peers.extend(new_peers);
                        }
                        GetPeersResult::Nodes(nodes) => {
                            for node_info in nodes {
                                let node =
                                    { DhtNode::new(node_info.id, SocketAddr::V4(node_info.addr)) };

                                self.routing_table.write().await.insert(node.clone());
                                candidates.push(node);
                            }
                        }
                        GetPeersResult::Both(new_peers, nodes) => {
                            peers.extend(new_peers);
                            for node_info in nodes {
                                let node =
                                    { DhtNode::new(node_info.id, SocketAddr::V4(node_info.addr)) };

                                self.routing_table.write().await.insert(node.clone());
                                candidates.push(node);
                            }
                        }
                    }
                }
            }

            candidates.sort_by_key(|node| {
                let distance = node.id.distance(&target);
                *distance.as_bytes()
            });
            candidates.truncate(K);

            if let Some(closest) = candidates.first() {
                let current_distance = closest.id.distance(&target);
                let dist_byte = current_distance.as_bytes()[0];

                if dist_byte >= last_closest_distance {
                    debug!(
                        "DHT lookup converged at iteration {} with {} peers",
                        iteration,
                        peers.len()
                    );
                    break;
                }
                last_closest_distance = dist_byte;
            }

            iteration += 1;
        }

        info!(
            "DHT found {} peers for info_hash after {} iterations",
            peers.len(),
            iteration
        );
        Ok(peers)
    }

    /// Announces that this node has the torrent on the given port to the k-closest DHT nodes.
    pub async fn announce(&self, info_hash: [u8; 20], port: u16) -> Result<()> {
        let target = NodeId::from_slice(&info_hash)?;
        let closest_nodes = self.routing_table.read().await.find_closest(&target, K);

        for node in closest_nodes.iter().take(ALPHA) {
            let query = Query::AnnouncePeer {
                id: self.self_id,
                info_hash,
                port,
                token: vec![],
            };

            if let Err(e) = self.send_query(node.addr, query).await {
                warn!("Failed to announce to {}: {}", node.addr, e);
            }
        }

        Ok(())
    }

    async fn send_query(&self, addr: SocketAddr, query: Query) -> Result<Response> {
        let (transaction_id, rx) = {
            let mut qm = self.query_manager.lock().await;
            let query_type = match &query {
                Query::Ping { .. } => QueryType::Ping,
                Query::FindNode { .. } => QueryType::FindNode,
                Query::GetPeers { .. } => QueryType::GetPeers,
                Query::AnnouncePeer { .. } => QueryType::AnnouncePeer,
            };
            qm.register_query(query_type, None, addr)
        };

        let msg = KrpcMessage::Query {
            transaction_id,
            query,
        };

        self.message_io.send_message(&msg, addr).await?;

        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(KrpcMessage::Response { response, .. })) => Ok(response),
            Ok(Ok(KrpcMessage::Error { error, .. })) => {
                anyhow::bail!("DHT error: {} (code {})", error.message, error.code)
            }
            Ok(Ok(_)) => anyhow::bail!("Unexpected message type"),
            Ok(Err(_)) => anyhow::bail!("Response channel closed"),
            Err(_) => anyhow::bail!("Query timeout"),
        }
    }

    async fn query_get_peers(
        &self,
        addr: SocketAddr,
        info_hash: [u8; 20],
    ) -> Result<GetPeersResult> {
        let query = Query::GetPeers {
            id: self.self_id,
            info_hash,
        };

        let response = self.send_query(addr, query).await?;

        match response {
            Response::GetPeers {
                values: Some(peer_addrs),
                nodes: Some(node_list),
                ..
            } => {
                let peers: Vec<Peer> = peer_addrs
                    .into_iter()
                    .map(|addr| Peer::new(addr.ip().to_string(), addr.port()))
                    .collect();
                Ok(GetPeersResult::Both(peers, node_list))
            }
            Response::GetPeers {
                values: Some(peer_addrs),
                ..
            } => {
                let peers: Vec<Peer> = peer_addrs
                    .into_iter()
                    .map(|addr| Peer::new(addr.ip().to_string(), addr.port()))
                    .collect();
                Ok(GetPeersResult::Peers(peers))
            }
            Response::GetPeers {
                nodes: Some(node_list),
                ..
            } => Ok(GetPeersResult::Nodes(node_list)),
            _ => Ok(GetPeersResult::Nodes(vec![])),
        }
    }

    /// Spawns a background task that listens for incoming DHT messages and dispatches them.
    pub fn spawn_message_handler(
        self: Arc<Self>,
        mut shutdown_rx: broadcast::Receiver<()>,
        ready_tx: Option<tokio::sync::oneshot::Sender<()>>,
    ) {
        tokio::spawn(async move {
            debug!("DHT message handler task starting");

            if let Some(tx) = ready_tx {
                let _ = tx.send(());
                debug!("DHT message handler signaled ready");
            }

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        debug!("DHT message handler shutting down");
                        break;
                    }
                    result = self.message_io.recv_message() => {
                        match result {
                            Ok((msg, addr)) => {
                                if let Err(e) = self.handle_message(msg, addr).await {
                                    warn!("Error handling DHT message from {}: {}", addr, e);
                                }
                            }
                            Err(e) => {
                                warn!("Error receiving DHT message: {}", e);
                            }
                        }
                    }
                }
            }
            debug!("DHT message handler task exited");
        });
    }

    /// Spawns a background task that periodically bootstraps the DHT to refresh the routing table.
    pub fn spawn_bootstrap_refresh(self: Arc<Self>, mut shutdown_rx: broadcast::Receiver<()>) {
        tokio::spawn(async move {
            let mut interval = time::interval(BOOTSTRAP_REFRESH_INTERVAL);

            interval.tick().await;

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        debug!("DHT bootstrap refresh shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        debug!("DHT periodic bootstrap refresh");
                        if let Err(e) = self.bootstrap().await {
                            warn!("Failed to refresh bootstrap: {}", e);
                        }
                    }
                }
            }
        });
    }

    /// Spawns a background task that periodically removes expired queries from the query manager.
    pub fn spawn_query_cleanup(self: Arc<Self>, mut shutdown_rx: broadcast::Receiver<()>) {
        tokio::spawn(async move {
            let mut interval = time::interval(QUERY_CLEANUP_INTERVAL);

            interval.tick().await;

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        debug!("DHT query cleanup shutting down");
                        break;
                    }
                    _ = interval.tick() => {
                        let mut qm = self.query_manager.lock().await;
                        let expired = qm.cleanup_expired();
                        if expired > 0 {
                            debug!("Cleaned up {} expired DHT queries", expired);
                        }
                    }
                }
            }
        });
    }

    async fn handle_message(&self, msg: KrpcMessage, addr: SocketAddr) -> Result<()> {
        match msg {
            KrpcMessage::Query {
                transaction_id,
                query,
            } => self.handle_query(transaction_id, query, addr).await,
            KrpcMessage::Response {
                transaction_id,
                response,
            } => {
                let response_msg = KrpcMessage::Response {
                    transaction_id: transaction_id.clone(),
                    response,
                };
                let mut qm = self.query_manager.lock().await;
                qm.handle_response(&transaction_id, response_msg)
            }
            KrpcMessage::Error {
                transaction_id,
                error,
            } => {
                debug!(
                    "Received DHT error from {}: {} (code {})",
                    addr, error.message, error.code
                );
                let error_msg = KrpcMessage::Error {
                    transaction_id: transaction_id.clone(),
                    error,
                };
                let mut qm = self.query_manager.lock().await;
                qm.handle_response(&transaction_id, error_msg)
            }
        }
    }

    async fn handle_query(
        &self,
        transaction_id: Vec<u8>,
        query: Query,
        addr: SocketAddr,
    ) -> Result<()> {
        let node = DhtNode::new(*query.sender_id(), addr);
        self.routing_table.write().await.insert(node);

        let response = match query {
            Query::Ping { .. } => Response::Ping { id: self.self_id },
            Query::FindNode { target, .. } => {
                let nodes = self.routing_table.read().await.find_closest(&target, K);
                let compact_nodes: Vec<CompactNodeInfo> = nodes
                    .iter()
                    .filter_map(|node| {
                        if let SocketAddr::V4(addr_v4) = node.addr {
                            Some(CompactNodeInfo {
                                id: node.id,
                                addr: addr_v4,
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                Response::FindNode {
                    id: self.self_id,
                    nodes: compact_nodes,
                }
            }
            Query::GetPeers { info_hash, .. } => {
                let target = NodeId::from_slice(&info_hash)?;
                let nodes = self.routing_table.read().await.find_closest(&target, K);
                let compact_nodes: Vec<CompactNodeInfo> = nodes
                    .iter()
                    .filter_map(|node| {
                        if let SocketAddr::V4(addr_v4) = node.addr {
                            Some(CompactNodeInfo {
                                id: node.id,
                                addr: addr_v4,
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                Response::GetPeers {
                    id: self.self_id,
                    token: vec![0x00, 0x01],
                    values: None,
                    nodes: Some(compact_nodes),
                }
            }
            Query::AnnouncePeer { .. } => Response::AnnouncePeer { id: self.self_id },
        };

        let response_msg = KrpcMessage::Response {
            transaction_id,
            response,
        };

        self.message_io.send_message(&response_msg, addr).await?;
        Ok(())
    }

    fn clone_arc(&self) -> Arc<Self> {
        Arc::new(Self {
            routing_table: self.routing_table.clone(),
            query_manager: self.query_manager.clone(),
            message_io: self.message_io.clone(),
            self_id: self.self_id,
            bootstrap_nodes: self.bootstrap_nodes.clone(),
        })
    }
}

enum GetPeersResult {
    Peers(Vec<Peer>),
    Nodes(Vec<CompactNodeInfo>),
    Both(Vec<Peer>, Vec<CompactNodeInfo>),
}
