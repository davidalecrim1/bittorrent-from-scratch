use super::types::{CompactNodeInfo, ErrorMessage, KrpcMessage, NodeId, Query, Response};
use crate::encoding::{Decoder as BencodeDecoder, Encoder as BencodeEncoder};
use crate::types::BencodeTypes;
use anyhow::Result;
use async_trait::async_trait;
use log::debug;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::UdpSocket;

const UDP_BUFFER_SIZE: usize = 1472;

#[async_trait]
pub trait DhtMessageIO: Send + Sync + std::fmt::Debug {
    /// Sends a KRPC message to the specified address over UDP.
    async fn send_message(&self, msg: &KrpcMessage, addr: SocketAddr) -> Result<()>;
    /// Receives a KRPC message from the UDP socket and returns it with the sender's address.
    async fn recv_message(&self) -> Result<(KrpcMessage, SocketAddr)>;
}

#[derive(Debug)]
pub struct UdpMessageIO {
    socket: Arc<UdpSocket>,
    encoder: BencodeEncoder,
    decoder: BencodeDecoder,
}

impl UdpMessageIO {
    /// Creates a new UdpMessageIO with the given socket and bencode encoder/decoder.
    pub fn new(socket: Arc<UdpSocket>, encoder: BencodeEncoder, decoder: BencodeDecoder) -> Self {
        Self {
            socket,
            encoder,
            decoder,
        }
    }

    pub fn encode_message(&self, msg: &KrpcMessage) -> Result<Vec<u8>> {
        let bencode = match msg {
            KrpcMessage::Query {
                transaction_id,
                query,
            } => self.encode_query(transaction_id, query)?,
            KrpcMessage::Response {
                transaction_id,
                response,
            } => self.encode_response(transaction_id, response)?,
            KrpcMessage::Error {
                transaction_id,
                error,
            } => self.encode_error(transaction_id, error)?,
        };

        self.encoder.from_bencode_types(bencode)
    }

    fn encode_query(&self, transaction_id: &[u8], query: &Query) -> Result<BencodeTypes> {
        let mut dict = BTreeMap::new();

        dict.insert("t".to_string(), BencodeTypes::Raw(transaction_id.to_vec()));
        dict.insert("y".to_string(), BencodeTypes::String("q".to_string()));
        dict.insert(
            "q".to_string(),
            BencodeTypes::String(query.query_type().to_string()),
        );

        let mut args = BTreeMap::new();
        args.insert(
            "id".to_string(),
            BencodeTypes::Raw(query.sender_id().as_bytes().to_vec()),
        );

        match query {
            Query::Ping { .. } => {}
            Query::FindNode { target, .. } => {
                args.insert(
                    "target".to_string(),
                    BencodeTypes::Raw(target.as_bytes().to_vec()),
                );
            }
            Query::GetPeers { info_hash, .. } => {
                args.insert(
                    "info_hash".to_string(),
                    BencodeTypes::Raw(info_hash.to_vec()),
                );
            }
            Query::AnnouncePeer {
                info_hash,
                port,
                token,
                ..
            } => {
                args.insert(
                    "info_hash".to_string(),
                    BencodeTypes::Raw(info_hash.to_vec()),
                );
                args.insert("port".to_string(), BencodeTypes::Integer(*port as isize));
                args.insert("token".to_string(), BencodeTypes::Raw(token.clone()));
            }
        }

        dict.insert("a".to_string(), BencodeTypes::Dictionary(args));

        Ok(BencodeTypes::Dictionary(dict))
    }

    fn encode_response(&self, transaction_id: &[u8], response: &Response) -> Result<BencodeTypes> {
        let mut dict = BTreeMap::new();

        dict.insert("t".to_string(), BencodeTypes::Raw(transaction_id.to_vec()));
        dict.insert("y".to_string(), BencodeTypes::String("r".to_string()));

        let mut r_dict = BTreeMap::new();
        r_dict.insert(
            "id".to_string(),
            BencodeTypes::Raw(response.sender_id().as_bytes().to_vec()),
        );

        match response {
            Response::Ping { .. } => {}
            Response::FindNode {
                nodes: nodes_ipv4,
                nodes6: nodes_ipv6,
                ..
            } => {
                if !nodes_ipv4.is_empty() {
                    let nodes_bytes: Vec<u8> =
                        nodes_ipv4.iter().flat_map(|node| node.to_bytes()).collect();
                    r_dict.insert("nodes".to_string(), BencodeTypes::Raw(nodes_bytes));
                }
                if !nodes_ipv6.is_empty() {
                    let nodes6_bytes: Vec<u8> =
                        nodes_ipv6.iter().flat_map(|node| node.to_bytes()).collect();
                    r_dict.insert("nodes6".to_string(), BencodeTypes::Raw(nodes6_bytes));
                }
            }
            Response::GetPeers {
                token,
                values,
                nodes: nodes_ipv4,
                nodes6: nodes_ipv6,
                ..
            } => {
                r_dict.insert("token".to_string(), BencodeTypes::Raw(token.clone()));

                if let Some(peers) = values {
                    let peer_list: Vec<BencodeTypes> = peers
                        .iter()
                        .map(|addr| {
                            let mut bytes = Vec::with_capacity(6);
                            bytes.extend_from_slice(&addr.ip().octets());
                            bytes.extend_from_slice(&addr.port().to_be_bytes());
                            BencodeTypes::Raw(bytes)
                        })
                        .collect();
                    r_dict.insert("values".to_string(), BencodeTypes::List(peer_list));
                }

                if let Some(node_list) = nodes_ipv4 {
                    let nodes_bytes: Vec<u8> =
                        node_list.iter().flat_map(|node| node.to_bytes()).collect();
                    r_dict.insert("nodes".to_string(), BencodeTypes::Raw(nodes_bytes));
                }

                if let Some(node_list_v6) = nodes_ipv6 {
                    let nodes6_bytes: Vec<u8> = node_list_v6
                        .iter()
                        .flat_map(|node| node.to_bytes())
                        .collect();
                    r_dict.insert("nodes6".to_string(), BencodeTypes::Raw(nodes6_bytes));
                }
            }
            Response::AnnouncePeer { .. } => {}
        }

        dict.insert("r".to_string(), BencodeTypes::Dictionary(r_dict));

        Ok(BencodeTypes::Dictionary(dict))
    }

    fn encode_error(&self, transaction_id: &[u8], error: &ErrorMessage) -> Result<BencodeTypes> {
        let mut dict = BTreeMap::new();

        dict.insert("t".to_string(), BencodeTypes::Raw(transaction_id.to_vec()));
        dict.insert("y".to_string(), BencodeTypes::String("e".to_string()));

        let error_list = vec![
            BencodeTypes::Integer(error.code as isize),
            BencodeTypes::String(error.message.clone()),
        ];
        dict.insert("e".to_string(), BencodeTypes::List(error_list));

        Ok(BencodeTypes::Dictionary(dict))
    }

    pub fn decode_message(&self, bytes: &[u8]) -> Result<KrpcMessage> {
        let (_bytes_read, bencode) = self.decoder.from_bytes(bytes)?;

        let dict = match bencode {
            BencodeTypes::Dictionary(d) => d,
            _ => anyhow::bail!("KRPC message must be a dictionary"),
        };

        let transaction_id = self.extract_transaction_id(&dict)?;
        let message_type = self.extract_message_type(&dict)?;

        match message_type.as_str() {
            "q" => {
                let query = self.decode_query(&dict)?;
                Ok(KrpcMessage::Query {
                    transaction_id,
                    query,
                })
            }
            "r" => {
                let response = self.decode_response(&dict)?;
                Ok(KrpcMessage::Response {
                    transaction_id,
                    response,
                })
            }
            "e" => {
                let error = self.decode_error(&dict)?;
                Ok(KrpcMessage::Error {
                    transaction_id,
                    error,
                })
            }
            _ => anyhow::bail!("Unknown message type: {}", message_type),
        }
    }

    fn extract_transaction_id(&self, dict: &BTreeMap<String, BencodeTypes>) -> Result<Vec<u8>> {
        match dict.get("t") {
            Some(BencodeTypes::Raw(bytes)) => Ok(bytes.clone()),
            Some(BencodeTypes::String(s)) => Ok(s.as_bytes().to_vec()),
            _ => anyhow::bail!("Missing or invalid transaction_id"),
        }
    }

    fn extract_message_type(&self, dict: &BTreeMap<String, BencodeTypes>) -> Result<String> {
        match dict.get("y") {
            Some(BencodeTypes::String(s)) => Ok(s.clone()),
            _ => anyhow::bail!("Missing or invalid message type"),
        }
    }

    fn decode_query(&self, dict: &BTreeMap<String, BencodeTypes>) -> Result<Query> {
        let query_type = match dict.get("q") {
            Some(BencodeTypes::String(s)) => s.clone(),
            _ => anyhow::bail!("Missing or invalid query type"),
        };

        let args = match dict.get("a") {
            Some(BencodeTypes::Dictionary(d)) => d,
            _ => anyhow::bail!("Missing or invalid query arguments"),
        };

        let id = self.extract_node_id(args, "id")?;

        match query_type.as_str() {
            "ping" => Ok(Query::Ping { id }),
            "find_node" => {
                let target = self.extract_node_id(args, "target")?;
                Ok(Query::FindNode { id, target })
            }
            "get_peers" => {
                let info_hash = self.extract_info_hash(args)?;
                Ok(Query::GetPeers { id, info_hash })
            }
            "announce_peer" => {
                let info_hash = self.extract_info_hash(args)?;
                let port = self.extract_port(args)?;
                let token = self.extract_raw_bytes(args, "token")?;
                Ok(Query::AnnouncePeer {
                    id,
                    info_hash,
                    port,
                    token,
                })
            }
            _ => anyhow::bail!("Unknown query type: {}", query_type),
        }
    }

    fn decode_response(&self, dict: &BTreeMap<String, BencodeTypes>) -> Result<Response> {
        let r_dict = match dict.get("r") {
            Some(BencodeTypes::Dictionary(d)) => d,
            _ => anyhow::bail!("Missing or invalid response dictionary"),
        };

        let id = self.extract_node_id(r_dict, "id")?;

        if r_dict.contains_key("token") {
            let token = self.extract_raw_bytes(r_dict, "token")?;

            let values = if let Some(BencodeTypes::List(list)) = r_dict.get("values") {
                Some(self.parse_peer_values(list)?)
            } else {
                None
            };

            let nodes_ipv4 = if let Some(BencodeTypes::Raw(bytes)) = r_dict.get("nodes") {
                Some(CompactNodeInfo::parse_multiple(bytes)?)
            } else {
                None
            };

            let nodes_ipv6 = if let Some(BencodeTypes::Raw(bytes)) = r_dict.get("nodes6") {
                Some(crate::dht::CompactNodeInfoV6::parse_multiple(bytes)?)
            } else {
                None
            };

            Ok(Response::GetPeers {
                id,
                token,
                values,
                nodes: nodes_ipv4,
                nodes6: nodes_ipv6,
            })
        } else if r_dict.contains_key("nodes") || r_dict.contains_key("nodes6") {
            let nodes_ipv4 = if let Some(BencodeTypes::Raw(bytes)) = r_dict.get("nodes") {
                CompactNodeInfo::parse_multiple(bytes)?
            } else {
                Vec::new()
            };

            let nodes_ipv6 = if let Some(BencodeTypes::Raw(bytes)) = r_dict.get("nodes6") {
                crate::dht::CompactNodeInfoV6::parse_multiple(bytes)?
            } else {
                Vec::new()
            };

            Ok(Response::FindNode {
                id,
                nodes: nodes_ipv4,
                nodes6: nodes_ipv6,
            })
        } else {
            Ok(Response::Ping { id })
        }
    }

    fn decode_error(&self, dict: &BTreeMap<String, BencodeTypes>) -> Result<ErrorMessage> {
        let error_list = match dict.get("e") {
            Some(BencodeTypes::List(list)) => list,
            _ => anyhow::bail!("Missing or invalid error list"),
        };

        if error_list.len() < 2 {
            anyhow::bail!("Error list must have at least 2 elements");
        }

        let code = match &error_list[0] {
            BencodeTypes::Integer(i) => *i as i64,
            _ => anyhow::bail!("Error code must be an integer"),
        };

        let message = match &error_list[1] {
            BencodeTypes::String(s) => s.clone(),
            _ => anyhow::bail!("Error message must be a string"),
        };

        Ok(ErrorMessage { code, message })
    }

    fn extract_node_id(&self, dict: &BTreeMap<String, BencodeTypes>, key: &str) -> Result<NodeId> {
        let bytes = self.extract_raw_bytes(dict, key)?;
        NodeId::from_slice(&bytes)
    }

    fn extract_info_hash(&self, dict: &BTreeMap<String, BencodeTypes>) -> Result<[u8; 20]> {
        let bytes = self.extract_raw_bytes(dict, "info_hash")?;
        if bytes.len() != 20 {
            anyhow::bail!("info_hash must be 20 bytes");
        }
        let mut arr = [0u8; 20];
        arr.copy_from_slice(&bytes);
        Ok(arr)
    }

    fn extract_port(&self, dict: &BTreeMap<String, BencodeTypes>) -> Result<u16> {
        match dict.get("port") {
            Some(BencodeTypes::Integer(i)) => {
                if *i < 0 || *i > 65535 {
                    anyhow::bail!("Port out of range: {}", i);
                }
                Ok(*i as u16)
            }
            _ => anyhow::bail!("Missing or invalid port"),
        }
    }

    fn extract_raw_bytes(
        &self,
        dict: &BTreeMap<String, BencodeTypes>,
        key: &str,
    ) -> Result<Vec<u8>> {
        match dict.get(key) {
            Some(BencodeTypes::Raw(bytes)) => Ok(bytes.clone()),
            Some(BencodeTypes::String(s)) => Ok(s.as_bytes().to_vec()),
            _ => anyhow::bail!("Missing or invalid field: {}", key),
        }
    }

    fn parse_peer_values(&self, list: &[BencodeTypes]) -> Result<Vec<SocketAddrV4>> {
        list.iter()
            .map(|item| {
                let bytes = match item {
                    BencodeTypes::Raw(bytes) => bytes.as_slice(),
                    BencodeTypes::String(s) => s.as_bytes(),
                    _ => anyhow::bail!("Peer value must be raw bytes or string"),
                };

                if bytes.len() != 6 {
                    anyhow::bail!("Peer address must be 6 bytes, got {}", bytes.len());
                }

                let ip = Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]);
                let port = u16::from_be_bytes([bytes[4], bytes[5]]);
                Ok(SocketAddrV4::new(ip, port))
            })
            .collect()
    }
}

#[async_trait]
impl DhtMessageIO for UdpMessageIO {
    /// Sends a KRPC message to the specified address over UDP.
    async fn send_message(&self, msg: &KrpcMessage, addr: SocketAddr) -> Result<()> {
        let bytes = self.encode_message(msg)?;
        self.socket.send_to(&bytes, addr).await?;
        debug!("Sent DHT message to {}: {:?}", addr, msg);
        Ok(())
    }

    /// Receives a KRPC message from the UDP socket and returns it with the sender's address.
    async fn recv_message(&self) -> Result<(KrpcMessage, SocketAddr)> {
        let mut buf = [0u8; UDP_BUFFER_SIZE];
        let (len, addr) = self.socket.recv_from(&mut buf).await?;
        let msg = self.decode_message(&buf[..len])?;
        debug!("Received DHT message from {}: {:?}", addr, msg);
        Ok((msg, addr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dht::{CompactNodeInfo, CompactNodeInfoV6, NodeId, Query, Response};
    use crate::encoding::{Decoder, Encoder};
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
    use std::sync::Arc;
    use tokio::net::UdpSocket;

    async fn create_test_message_io() -> UdpMessageIO {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let encoder = Encoder {};
        let decoder = Decoder {};
        UdpMessageIO::new(Arc::new(socket), encoder, decoder)
    }

    #[tokio::test]
    async fn test_encode_decode_find_node_response_with_ipv6() {
        let io = create_test_message_io().await;

        let ipv4_node = CompactNodeInfo {
            id: NodeId::new([0x01; 20]),
            addr: SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 6881),
        };

        let ipv6_node = CompactNodeInfoV6 {
            id: NodeId::new([0x02; 20]),
            addr: SocketAddrV6::new(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1), 6881, 0, 0),
        };

        let response = Response::FindNode {
            id: NodeId::new([0x42; 20]),
            nodes: vec![ipv4_node.clone()],
            nodes6: vec![ipv6_node.clone()],
        };

        let msg = KrpcMessage::Response {
            transaction_id: vec![0x01, 0x02],
            response: response.clone(),
        };

        let encoded = io.encode_message(&msg).unwrap();
        let decoded = io.decode_message(&encoded).unwrap();

        if let KrpcMessage::Response {
            transaction_id,
            response: decoded_response,
        } = decoded
        {
            assert_eq!(transaction_id, vec![0x01, 0x02]);
            if let Response::FindNode { id, nodes, nodes6 } = decoded_response {
                assert_eq!(id, NodeId::new([0x42; 20]));
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].id, ipv4_node.id);
                assert_eq!(nodes6.len(), 1);
                assert_eq!(nodes6[0].id, ipv6_node.id);
            } else {
                panic!("Expected FindNode response");
            }
        } else {
            panic!("Expected Response message");
        }
    }

    #[tokio::test]
    async fn test_encode_decode_get_peers_response_with_ipv6() {
        let io = create_test_message_io().await;

        let ipv4_node = CompactNodeInfo {
            id: NodeId::new([0x03; 20]),
            addr: SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 6882),
        };

        let ipv6_node = CompactNodeInfoV6 {
            id: NodeId::new([0x04; 20]),
            addr: SocketAddrV6::new(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1), 6882, 0, 0),
        };

        let response = Response::GetPeers {
            id: NodeId::new([0x43; 20]),
            token: vec![0xAA, 0xBB],
            values: None,
            nodes: Some(vec![ipv4_node.clone()]),
            nodes6: Some(vec![ipv6_node.clone()]),
        };

        let msg = KrpcMessage::Response {
            transaction_id: vec![0x12, 0x34],
            response: response.clone(),
        };

        let encoded = io.encode_message(&msg).unwrap();
        let decoded = io.decode_message(&encoded).unwrap();

        if let KrpcMessage::Response {
            transaction_id,
            response: decoded_response,
        } = decoded
        {
            assert_eq!(transaction_id, vec![0x12, 0x34]);
            if let Response::GetPeers {
                id,
                token,
                nodes,
                nodes6,
                ..
            } = decoded_response
            {
                assert_eq!(id, NodeId::new([0x43; 20]));
                assert_eq!(token, vec![0xAA, 0xBB]);
                assert_eq!(nodes.unwrap().len(), 1);
                assert_eq!(nodes6.unwrap().len(), 1);
            } else {
                panic!("Expected GetPeers response");
            }
        } else {
            panic!("Expected Response message");
        }
    }

    #[tokio::test]
    async fn test_encode_decode_ping_query() {
        let io = create_test_message_io().await;

        let query = Query::Ping {
            id: NodeId::new([0x55; 20]),
        };

        let msg = KrpcMessage::Query {
            transaction_id: vec![0xAB, 0xCD],
            query: query.clone(),
        };

        let encoded = io.encode_message(&msg).unwrap();
        let decoded = io.decode_message(&encoded).unwrap();

        if let KrpcMessage::Query {
            transaction_id,
            query: decoded_query,
        } = decoded
        {
            assert_eq!(transaction_id, vec![0xAB, 0xCD]);
            if let Query::Ping { id } = decoded_query {
                assert_eq!(id, NodeId::new([0x55; 20]));
            } else {
                panic!("Expected Ping query");
            }
        } else {
            panic!("Expected Query message");
        }
    }

    #[tokio::test]
    async fn test_encode_decode_find_node_query() {
        let io = create_test_message_io().await;

        let query = Query::FindNode {
            id: NodeId::new([0x66; 20]),
            target: NodeId::new([0x77; 20]),
        };

        let msg = KrpcMessage::Query {
            transaction_id: vec![0x11, 0x22],
            query: query.clone(),
        };

        let encoded = io.encode_message(&msg).unwrap();
        let decoded = io.decode_message(&encoded).unwrap();

        if let KrpcMessage::Query {
            transaction_id,
            query: decoded_query,
        } = decoded
        {
            assert_eq!(transaction_id, vec![0x11, 0x22]);
            if let Query::FindNode { id, target } = decoded_query {
                assert_eq!(id, NodeId::new([0x66; 20]));
                assert_eq!(target, NodeId::new([0x77; 20]));
            } else {
                panic!("Expected FindNode query");
            }
        } else {
            panic!("Expected Query message");
        }
    }

    #[test]
    fn test_find_node_response_with_both_ipv4_and_ipv6_nodes() {
        let ipv4_node = CompactNodeInfo {
            id: NodeId::new([0x01; 20]),
            addr: SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 6881),
        };

        let ipv6_node = CompactNodeInfoV6 {
            id: NodeId::new([0x02; 20]),
            addr: SocketAddrV6::new(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1), 6881, 0, 0),
        };

        let response = Response::FindNode {
            id: NodeId::new([0x42; 20]),
            nodes: vec![ipv4_node.clone()],
            nodes6: vec![ipv6_node.clone()],
        };

        // Verify response contains both node types
        if let Response::FindNode { id, nodes, nodes6 } = response {
            assert_eq!(id, NodeId::new([0x42; 20]));
            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0].id, ipv4_node.id);
            assert_eq!(nodes6.len(), 1);
            assert_eq!(nodes6[0].id, ipv6_node.id);
        } else {
            panic!("Expected FindNode response");
        }
    }

    #[test]
    fn test_get_peers_response_with_both_ipv4_and_ipv6_nodes() {
        let token = vec![0xAA, 0xBB];

        let ipv4_node = CompactNodeInfo {
            id: NodeId::new([0x03; 20]),
            addr: SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 6882),
        };

        let ipv6_node = CompactNodeInfoV6 {
            id: NodeId::new([0x04; 20]),
            addr: SocketAddrV6::new(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1), 6882, 0, 0),
        };

        let response = Response::GetPeers {
            id: NodeId::new([0x43; 20]),
            token: token.clone(),
            values: None,
            nodes: Some(vec![ipv4_node.clone()]),
            nodes6: Some(vec![ipv6_node.clone()]),
        };

        // Verify response contains both node types
        if let Response::GetPeers {
            id,
            token: resp_token,
            values,
            nodes,
            nodes6,
        } = response
        {
            assert_eq!(id, NodeId::new([0x43; 20]));
            assert_eq!(resp_token, token);
            assert!(values.is_none());

            let nodes = nodes.unwrap();
            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0].id, ipv4_node.id);

            let nodes6 = nodes6.unwrap();
            assert_eq!(nodes6.len(), 1);
            assert_eq!(nodes6[0].id, ipv6_node.id);
        } else {
            panic!("Expected GetPeers response");
        }
    }
}
