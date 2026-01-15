use super::types::{DhtNode, NodeId};

#[derive(Debug, Clone, Default)]
pub struct RoutingTableStats {
    pub total_nodes: usize,
    pub buckets_used: usize,
    pub buckets_full: usize,
    pub ipv4_nodes: usize,
    pub ipv6_nodes: usize,
}

pub struct RoutingTable {
    buckets: Vec<Vec<DhtNode>>,
    self_id: NodeId,
    k: usize,
}

impl RoutingTable {
    /// Creates a new routing table with 160 k-buckets for the given node ID and bucket size.
    pub fn new(self_id: NodeId, k: usize) -> Self {
        Self {
            buckets: vec![Vec::new(); 160],
            self_id,
            k,
        }
    }

    /// Inserts a node into the appropriate bucket, updating last_seen if already present or evicting the oldest if full.
    pub fn insert(&mut self, node: DhtNode) {
        if node.id == self.self_id {
            return;
        }

        let bucket_index = self.bucket_index(&node.id);
        let bucket = &mut self.buckets[bucket_index];

        if let Some(pos) = bucket.iter().position(|n| n.id == node.id) {
            bucket[pos].update_last_seen();
            return;
        }

        if bucket.len() < self.k {
            bucket.push(node);
        } else {
            let oldest_idx = bucket
                .iter()
                .enumerate()
                .min_by_key(|(_, n)| n.last_seen)
                .map(|(idx, _)| idx);

            if let Some(idx) = oldest_idx {
                bucket[idx] = node;
            }
        }
    }

    /// Returns up to `count` closest nodes to the target, sorted by XOR distance.
    pub fn find_closest(&self, target: &NodeId, count: usize) -> Vec<DhtNode> {
        let mut nodes: Vec<DhtNode> = self
            .buckets
            .iter()
            .flat_map(|bucket| bucket.iter().cloned())
            .collect();

        nodes.sort_by_key(|node| {
            let distance = node.id.distance(target);
            *distance.as_bytes()
        });

        nodes.into_iter().take(count).collect()
    }

    /// Removes a node from the routing table by its NodeId.
    pub fn remove(&mut self, node_id: &NodeId) {
        let bucket_index = self.bucket_index(node_id);
        let bucket = &mut self.buckets[bucket_index];
        bucket.retain(|n| n.id != *node_id);
    }

    /// Calculates the bucket index for a given node ID based on XOR distance.
    pub fn bucket_index(&self, node_id: &NodeId) -> usize {
        let distance = self.self_id.distance(node_id);
        let leading_zeros = distance.leading_zeros();
        if leading_zeros >= 160 {
            159
        } else {
            leading_zeros
        }
    }

    /// Returns a reference to the bucket at the given index.
    pub fn get_bucket(&self, index: usize) -> &Vec<DhtNode> {
        &self.buckets[index]
    }

    /// Returns the total number of nodes across all buckets.
    pub fn len(&self) -> usize {
        self.buckets.iter().map(|bucket| bucket.len()).sum()
    }

    /// Returns true if the routing table contains no nodes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns statistics about the routing table.
    pub fn get_stats(&self) -> RoutingTableStats {
        let mut stats = RoutingTableStats::default();

        for bucket in &self.buckets {
            if !bucket.is_empty() {
                stats.buckets_used += 1;
                if bucket.len() >= self.k {
                    stats.buckets_full += 1;
                }
                stats.total_nodes += bucket.len();

                for node in bucket {
                    if node.addr.is_ipv4() {
                        stats.ipv4_nodes += 1;
                    } else {
                        stats.ipv6_nodes += 1;
                    }
                }
            }
        }

        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn create_test_node(id_byte: u8, addr: SocketAddr) -> DhtNode {
        let id = NodeId::new([id_byte; 20]);
        DhtNode::new(id, addr)
    }

    #[test]
    fn test_insert_new_node() {
        let self_id = NodeId::new([0x00; 20]);
        let mut table = RoutingTable::new(self_id, 8);

        let node = create_test_node(0x01, "127.0.0.1:6881".parse().unwrap());
        table.insert(node.clone());

        assert_eq!(table.len(), 1);
    }

    #[test]
    fn test_insert_duplicate_node_updates_last_seen() {
        let self_id = NodeId::new([0x00; 20]);
        let mut table = RoutingTable::new(self_id, 8);

        let node = create_test_node(0x01, "127.0.0.1:6881".parse().unwrap());
        table.insert(node.clone());

        std::thread::sleep(std::time::Duration::from_millis(10));

        let node2 = create_test_node(0x01, "127.0.0.1:6881".parse().unwrap());
        table.insert(node2);

        assert_eq!(table.len(), 1);
    }

    #[test]
    fn test_insert_does_not_add_self() {
        let self_id = NodeId::new([0x00; 20]);
        let mut table = RoutingTable::new(self_id, 8);

        let node = create_test_node(0x00, "127.0.0.1:6881".parse().unwrap());
        table.insert(node);

        assert_eq!(table.len(), 0);
    }

    #[test]
    fn test_bucket_full_evicts_oldest() {
        let self_id = NodeId::new([0x00; 20]);
        let mut table = RoutingTable::new(self_id, 2);

        let mut id1 = [0x00; 20];
        id1[0] = 0x80;
        id1[19] = 0x01;
        let node1 = DhtNode::new(NodeId::new(id1), "127.0.0.1:6881".parse().unwrap());
        table.insert(node1.clone());

        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut id2 = [0x00; 20];
        id2[0] = 0x80;
        id2[19] = 0x02;
        let node2 = DhtNode::new(NodeId::new(id2), "127.0.0.1:6882".parse().unwrap());
        table.insert(node2.clone());

        std::thread::sleep(std::time::Duration::from_millis(10));

        let mut id3 = [0x00; 20];
        id3[0] = 0x80;
        id3[19] = 0x03;
        let node3 = DhtNode::new(NodeId::new(id3), "127.0.0.1:6883".parse().unwrap());
        table.insert(node3.clone());

        let bucket_idx = table.bucket_index(&node1.id);
        let bucket = table.get_bucket(bucket_idx);

        assert_eq!(bucket.len(), 2);
    }

    #[test]
    fn test_find_closest() {
        let self_id = NodeId::new([0x00; 20]);
        let mut table = RoutingTable::new(self_id, 8);

        table.insert(create_test_node(0x01, "127.0.0.1:6881".parse().unwrap()));
        table.insert(create_test_node(0x02, "127.0.0.1:6882".parse().unwrap()));
        table.insert(create_test_node(0x04, "127.0.0.1:6883".parse().unwrap()));
        table.insert(create_test_node(0x08, "127.0.0.1:6884".parse().unwrap()));

        let target = NodeId::new([0x03; 20]);
        let closest = table.find_closest(&target, 2);

        assert_eq!(closest.len(), 2);
        assert_eq!(closest[0].id, NodeId::new([0x02; 20]));
    }

    #[test]
    fn test_remove() {
        let self_id = NodeId::new([0x00; 20]);
        let mut table = RoutingTable::new(self_id, 8);

        let node = create_test_node(0x01, "127.0.0.1:6881".parse().unwrap());
        table.insert(node.clone());

        assert_eq!(table.len(), 1);

        table.remove(&node.id);

        assert_eq!(table.len(), 0);
    }

    #[test]
    fn test_bucket_index() {
        let self_id = NodeId::new([0x00; 20]);
        let table = RoutingTable::new(self_id, 8);

        let node1_id = NodeId::new([
            0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        assert_eq!(table.bucket_index(&node1_id), 0);

        let node2_id = NodeId::new([
            0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ]);
        assert_eq!(table.bucket_index(&node2_id), 8);
    }
}
