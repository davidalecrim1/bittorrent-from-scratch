use super::RoutingTableStats;
use super::manager::DhtManager;
use crate::types::Peer;
use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait DhtClient: Send + Sync {
    /// Performs an iterative DHT lookup to find peers for the given info_hash.
    async fn get_peers(&self, info_hash: [u8; 20]) -> Result<Vec<Peer>>;
    /// Announces that this node has the torrent identified by info_hash on the given port.
    async fn announce(&self, info_hash: [u8; 20], port: u16) -> Result<()>;
    /// Returns statistics about the DHT routing table.
    async fn get_stats(&self) -> Result<RoutingTableStats>;
}

#[async_trait]
impl DhtClient for DhtManager {
    /// Performs an iterative DHT lookup to find peers for the given info_hash.
    async fn get_peers(&self, info_hash: [u8; 20]) -> Result<Vec<Peer>> {
        self.get_peers(info_hash).await
    }

    /// Announces that this node has the torrent identified by info_hash on the given port.
    async fn announce(&self, info_hash: [u8; 20], port: u16) -> Result<()> {
        self.announce(info_hash, port).await
    }

    /// Returns statistics about the DHT routing table.
    async fn get_stats(&self) -> Result<RoutingTableStats> {
        Ok(self.get_stats().await)
    }
}
