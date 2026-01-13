use anyhow::Result;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId([u8; 20]);

impl NodeId {
    /// Creates a new NodeId from a 20-byte array.
    pub fn new(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    /// Creates a NodeId from a byte slice, returning an error if not exactly 20 bytes.
    pub fn from_slice(slice: &[u8]) -> Result<Self> {
        if slice.len() != 20 {
            anyhow::bail!("NodeId must be exactly 20 bytes, got {}", slice.len());
        }
        let mut bytes = [0u8; 20];
        bytes.copy_from_slice(slice);
        Ok(Self(bytes))
    }

    /// Returns a reference to the underlying 20-byte array.
    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// Calculates the XOR distance between two NodeIds.
    ///
    /// The Kademlia DHT uses XOR distance as a metric for node proximity.
    /// XOR has important properties for distributed systems:
    /// - Symmetric: distance(A, B) = distance(B, A)
    /// - Triangle inequality: distance(A, C) <= distance(A, B) + distance(B, C)
    /// - Distance to self is always zero: distance(A, A) = 0
    ///
    /// # Example
    /// ```
    /// # use bittorrent_from_scratch::dht::NodeId;
    /// let id1 = NodeId::new([0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    /// let id2 = NodeId::new([0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    ///
    /// let distance = id1.distance(&id2);
    /// // 0x01 XOR 0x03 = 0x02
    /// assert_eq!(distance.as_bytes()[3], 0x02);
    /// ```
    ///
    /// XOR each byte: if bits are the same, result is 0; if different, result is 1.
    /// This creates a "distance" where more similar IDs have smaller XOR values.
    pub fn distance(&self, other: &NodeId) -> NodeId {
        let mut result = [0u8; 20];

        for (i, item) in result.iter_mut().enumerate() {
            *item = self.0[i] ^ other.0[i];
        }

        NodeId(result)
    }

    /// Returns the number of leading zero bits in the NodeId (used for bucket selection).
    pub fn leading_zeros(&self) -> usize {
        for (i, &byte) in self.0.iter().enumerate() {
            if byte != 0 {
                return i * 8 + byte.leading_zeros() as usize;
            }
        }
        160
    }

    /// Generates a random NodeId for this DHT node.
    pub fn random() -> Self {
        let mut bytes = [0u8; 20];
        for byte in &mut bytes {
            *byte = rand::random();
        }
        Self(bytes)
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({})", hex::encode(&self.0[..4]))
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

#[derive(Clone, Debug)]
pub struct DhtNode {
    pub id: NodeId,
    pub addr: SocketAddr,
    pub last_seen: Instant,
}

impl DhtNode {
    /// Creates a new DHT node with the current time as last_seen.
    pub fn new(id: NodeId, addr: SocketAddr) -> Self {
        Self {
            id,
            addr,
            last_seen: Instant::now(),
        }
    }

    /// Updates the last_seen timestamp to the current time.
    pub fn update_last_seen(&mut self) {
        self.last_seen = Instant::now();
    }
}

#[derive(Clone, Debug)]
pub struct CompactNodeInfo {
    pub id: NodeId,
    pub addr: SocketAddrV4,
}

impl CompactNodeInfo {
    /// Serializes the node info to 26 bytes (20-byte ID + 4-byte IPv4 + 2-byte port).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(26);
        bytes.extend_from_slice(self.id.as_bytes());
        bytes.extend_from_slice(&self.addr.ip().octets());
        bytes.extend_from_slice(&self.addr.port().to_be_bytes());
        bytes
    }

    /// Deserializes node info from 26 bytes, returning an error if length is incorrect.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 26 {
            anyhow::bail!(
                "CompactNodeInfo must be exactly 26 bytes, got {}",
                bytes.len()
            );
        }

        let id = NodeId::from_slice(&bytes[0..20])?;
        let ip = Ipv4Addr::new(bytes[20], bytes[21], bytes[22], bytes[23]);
        let port = u16::from_be_bytes([bytes[24], bytes[25]]);
        let addr = SocketAddrV4::new(ip, port);

        Ok(Self { id, addr })
    }

    /// Parses a list of compact node infos from bytes (length must be multiple of 26).
    pub fn parse_multiple(bytes: &[u8]) -> Result<Vec<Self>> {
        if !bytes.len().is_multiple_of(26) {
            anyhow::bail!(
                "Compact node list length must be multiple of 26, got {}",
                bytes.len()
            );
        }

        bytes
            .chunks(26)
            .map(Self::from_bytes)
            .collect::<Result<Vec<_>>>()
    }
}

#[derive(Debug, Clone)]
pub enum Query {
    Ping {
        id: NodeId,
    },
    FindNode {
        id: NodeId,
        target: NodeId,
    },
    GetPeers {
        id: NodeId,
        info_hash: [u8; 20],
    },
    AnnouncePeer {
        id: NodeId,
        info_hash: [u8; 20],
        port: u16,
        token: Vec<u8>,
    },
}

impl Query {
    /// Returns the KRPC query type string (ping, find_node, get_peers, announce_peer).
    pub fn query_type(&self) -> &str {
        match self {
            Query::Ping { .. } => "ping",
            Query::FindNode { .. } => "find_node",
            Query::GetPeers { .. } => "get_peers",
            Query::AnnouncePeer { .. } => "announce_peer",
        }
    }

    /// Returns the NodeId of the sender from any query variant.
    pub fn sender_id(&self) -> &NodeId {
        match self {
            Query::Ping { id } => id,
            Query::FindNode { id, .. } => id,
            Query::GetPeers { id, .. } => id,
            Query::AnnouncePeer { id, .. } => id,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Response {
    Ping {
        id: NodeId,
    },
    FindNode {
        id: NodeId,
        nodes: Vec<CompactNodeInfo>,
    },
    GetPeers {
        id: NodeId,
        token: Vec<u8>,
        values: Option<Vec<SocketAddrV4>>,
        nodes: Option<Vec<CompactNodeInfo>>,
    },
    AnnouncePeer {
        id: NodeId,
    },
}

impl Response {
    /// Returns the NodeId of the sender from any response variant.
    pub fn sender_id(&self) -> &NodeId {
        match self {
            Response::Ping { id } => id,
            Response::FindNode { id, .. } => id,
            Response::GetPeers { id, .. } => id,
            Response::AnnouncePeer { id } => id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ErrorMessage {
    pub code: i64,
    pub message: String,
}

impl ErrorMessage {
    /// Creates a new error message with the given code and message string.
    pub fn new(code: i64, message: String) -> Self {
        Self { code, message }
    }
}

#[derive(Debug, Clone)]
pub enum KrpcMessage {
    Query {
        transaction_id: Vec<u8>,
        query: Query,
    },
    Response {
        transaction_id: Vec<u8>,
        response: Response,
    },
    Error {
        transaction_id: Vec<u8>,
        error: ErrorMessage,
    },
}

impl KrpcMessage {
    /// Returns the transaction ID from any KRPC message variant.
    pub fn transaction_id(&self) -> &[u8] {
        match self {
            KrpcMessage::Query { transaction_id, .. } => transaction_id,
            KrpcMessage::Response { transaction_id, .. } => transaction_id,
            KrpcMessage::Error { transaction_id, .. } => transaction_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id_distance() {
        let id1 = NodeId::new([0x01; 20]);
        let id2 = NodeId::new([0x02; 20]);
        let distance = id1.distance(&id2);
        assert_eq!(distance.as_bytes()[0], 0x03);
    }

    #[test]
    fn test_node_id_leading_zeros() {
        let id = NodeId::new([
            0x00, 0x00, 0x00, 0x80, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        ]);
        assert_eq!(id.leading_zeros(), 24);
    }

    #[test]
    fn test_node_id_all_zeros() {
        let id = NodeId::new([0x00; 20]);
        assert_eq!(id.leading_zeros(), 160);
    }

    #[test]
    fn test_compact_node_info_roundtrip() {
        let id = NodeId::new([0x42; 20]);
        let addr = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 6881);
        let info = CompactNodeInfo { id, addr };

        let bytes = info.to_bytes();
        assert_eq!(bytes.len(), 26);

        let parsed = CompactNodeInfo::from_bytes(&bytes).unwrap();
        assert_eq!(parsed.id, id);
        assert_eq!(parsed.addr, addr);
    }

    #[test]
    fn test_compact_node_info_parse_multiple() {
        let id1 = NodeId::new([0x01; 20]);
        let addr1 = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 6881);
        let info1 = CompactNodeInfo {
            id: id1,
            addr: addr1,
        };

        let id2 = NodeId::new([0x02; 20]);
        let addr2 = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 2), 6882);
        let info2 = CompactNodeInfo {
            id: id2,
            addr: addr2,
        };

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&info1.to_bytes());
        bytes.extend_from_slice(&info2.to_bytes());

        let parsed = CompactNodeInfo::parse_multiple(&bytes).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, id1);
        assert_eq!(parsed[1].id, id2);
    }
}
