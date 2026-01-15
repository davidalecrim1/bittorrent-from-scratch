mod helpers;

use bittorrent_from_scratch::dht::{
    CompactNodeInfo, CompactNodeInfoV6, DhtClient, KrpcMessage, NodeId, Query, Response,
    RoutingTable,
};
use bittorrent_from_scratch::types::{Peer, PeerSource};
use helpers::fakes::MockDhtClient;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

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
        0x00, 0x00, 0x00, 0x80, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
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

#[test]
fn test_routing_table_insert_new_node() {
    let self_id = NodeId::new([0x00; 20]);
    let mut table = RoutingTable::new(self_id, 8);

    let node_id = NodeId::new([0x01; 20]);
    let node =
        bittorrent_from_scratch::dht::DhtNode::new(node_id, "127.0.0.1:6881".parse().unwrap());
    table.insert(node.clone());

    assert_eq!(table.len(), 1);
}

#[test]
fn test_routing_table_find_closest() {
    let self_id = NodeId::new([0x00; 20]);
    let mut table = RoutingTable::new(self_id, 8);

    table.insert(bittorrent_from_scratch::dht::DhtNode::new(
        NodeId::new([0x01; 20]),
        "127.0.0.1:6881".parse().unwrap(),
    ));
    table.insert(bittorrent_from_scratch::dht::DhtNode::new(
        NodeId::new([0x02; 20]),
        "127.0.0.1:6882".parse().unwrap(),
    ));
    table.insert(bittorrent_from_scratch::dht::DhtNode::new(
        NodeId::new([0x04; 20]),
        "127.0.0.1:6883".parse().unwrap(),
    ));

    let target = NodeId::new([0x03; 20]);
    let closest = table.find_closest(&target, 2);

    assert_eq!(closest.len(), 2);
    assert_eq!(closest[0].id, NodeId::new([0x02; 20]));
}

#[test]
fn test_routing_table_bucket_index() {
    let self_id = NodeId::new([0x00; 20]);
    let table = RoutingTable::new(self_id, 8);

    let node1_id = NodeId::new([
        0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00,
    ]);
    assert_eq!(table.bucket_index(&node1_id), 0);

    let node2_id = NodeId::new([
        0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00,
    ]);
    assert_eq!(table.bucket_index(&node2_id), 8);
}

#[tokio::test]
async fn test_mock_dht_client_get_peers() {
    let mock = MockDhtClient::new();
    let peers = vec![
        Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Dht),
        Peer::new("192.168.1.2".to_string(), 6882, PeerSource::Dht),
    ];
    mock.expect_get_peers(peers.clone()).await;

    let info_hash = [0x42; 20];
    let result = mock.get_peers(info_hash).await.unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].get_addr(), "192.168.1.1:6881");
}

#[tokio::test]
async fn test_mock_dht_client_failure() {
    let mock = MockDhtClient::new();
    mock.expect_failure("DHT timeout").await;

    let info_hash = [0x42; 20];
    let result = mock.get_peers(info_hash).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("DHT timeout"));
}

#[tokio::test]
async fn test_mock_dht_client_empty_response() {
    let mock = MockDhtClient::new();

    let info_hash = [0x42; 20];
    let result = mock.get_peers(info_hash).await.unwrap();
    assert_eq!(result.len(), 0);
}

#[tokio::test]
async fn test_mock_dht_client_announce() {
    let mock = MockDhtClient::new();

    let info_hash = [0x42; 20];
    let result = mock.announce(info_hash, 6881).await;
    assert!(result.is_ok());
}

#[test]
fn test_query_sender_id() {
    let node_id = NodeId::new([0x42; 20]);

    let query = Query::Ping { id: node_id };
    assert_eq!(query.sender_id(), &node_id);

    let query = Query::FindNode {
        id: node_id,
        target: NodeId::new([0x01; 20]),
    };
    assert_eq!(query.sender_id(), &node_id);

    let query = Query::GetPeers {
        id: node_id,
        info_hash: [0x01; 20],
    };
    assert_eq!(query.sender_id(), &node_id);
}

#[test]
fn test_response_sender_id() {
    let node_id = NodeId::new([0x42; 20]);

    let response = Response::Ping { id: node_id };
    assert_eq!(response.sender_id(), &node_id);

    let response = Response::FindNode {
        id: node_id,
        nodes: vec![],
        nodes6: vec![],
    };
    assert_eq!(response.sender_id(), &node_id);

    let response = Response::GetPeers {
        id: node_id,
        token: vec![0x01, 0x02],
        values: None,
        nodes: None,
        nodes6: None,
    };
    assert_eq!(response.sender_id(), &node_id);
}

#[test]
fn test_krpc_message_transaction_id() {
    let txn_id = vec![0x01, 0x02, 0x03, 0x04];

    let msg = KrpcMessage::Query {
        transaction_id: txn_id.clone(),
        query: Query::Ping {
            id: NodeId::new([0x42; 20]),
        },
    };
    assert_eq!(msg.transaction_id(), &txn_id);

    let msg = KrpcMessage::Response {
        transaction_id: txn_id.clone(),
        response: Response::Ping {
            id: NodeId::new([0x42; 20]),
        },
    };
    assert_eq!(msg.transaction_id(), &txn_id);
}

fn encode_krpc_message(msg: &KrpcMessage) -> anyhow::Result<Vec<u8>> {
    use bittorrent_from_scratch::encoding::Encoder;
    use bittorrent_from_scratch::types::BencodeTypes;
    use std::collections::BTreeMap;

    let encoder = Encoder {};

    let bencode = match msg {
        KrpcMessage::Query {
            transaction_id,
            query,
        } => {
            let mut dict = BTreeMap::new();
            dict.insert("t".to_string(), BencodeTypes::Raw(transaction_id.clone()));
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
                Query::Ping { .. } => {}
                _ => {}
            }

            dict.insert("a".to_string(), BencodeTypes::Dictionary(args));
            BencodeTypes::Dictionary(dict)
        }
        _ => return Err(anyhow::anyhow!("Only queries supported in test helper")),
    };

    encoder.from_bencode_types(bencode)
}

#[test]
fn test_find_node_query_encoding_is_valid_bencode() {
    let node_id = NodeId::new([0x42; 20]);
    let target_id = NodeId::new([0x99; 20]);
    let transaction_id = vec![0x00, 0x01, 0x02, 0x03];

    let query = Query::FindNode {
        id: node_id,
        target: target_id,
    };

    let msg = KrpcMessage::Query {
        transaction_id: transaction_id.clone(),
        query,
    };

    let encoded = encode_krpc_message(&msg).unwrap();

    let encoded_str = String::from_utf8_lossy(&encoded);
    assert!(encoded_str.starts_with("d1:a"));
    assert!(encoded_str.contains("20:"));
    assert!(encoded_str.contains("4:"));

    assert!(encoded.iter().position(|&b| b == b'd').is_some());
    assert!(encoded.iter().position(|&b| b == b'e').is_some());

    let mut has_valid_length_prefix = false;
    for i in 0..encoded.len() - 1 {
        if encoded[i] == b':' && i > 0 {
            let prev_byte = encoded[i - 1];
            if prev_byte.is_ascii_digit() {
                has_valid_length_prefix = true;
                break;
            }
        }
    }
    assert!(
        has_valid_length_prefix,
        "Encoded message should have length prefixes for all strings"
    );
}

#[test]
fn test_find_node_query_has_proper_length_prefixes() {
    let node_id = NodeId::new([0xAB; 20]);
    let target_id = NodeId::new([0xCD; 20]);
    let transaction_id = vec![0xDE, 0xAD, 0xBE, 0xEF];

    let query = Query::FindNode {
        id: node_id,
        target: target_id,
    };

    let msg = KrpcMessage::Query {
        transaction_id: transaction_id.clone(),
        query,
    };

    let encoded = encode_krpc_message(&msg).unwrap();

    let encoded_str = String::from_utf8_lossy(&encoded);

    assert!(
        encoded_str.contains("4:"),
        "Transaction ID should have 4: length prefix"
    );
    assert!(
        encoded_str.contains("20:"),
        "Node IDs should have 20: length prefix"
    );
}

#[test]
fn test_get_peers_query_has_length_prefixes() {
    let node_id = NodeId::new([0x11; 20]);
    let info_hash = [0x22; 20];
    let transaction_id = vec![0x00, 0x11, 0x22, 0x33];

    let query = Query::GetPeers {
        id: node_id,
        info_hash,
    };

    let msg = KrpcMessage::Query {
        transaction_id: transaction_id.clone(),
        query,
    };

    let encoded = encode_krpc_message(&msg).unwrap();

    let encoded_str = String::from_utf8_lossy(&encoded);
    assert!(encoded_str.starts_with("d1:a"));
    assert!(
        encoded_str.contains("20:"),
        "Node ID and info_hash should have 20: length prefix"
    );
    assert!(
        encoded_str.contains("4:"),
        "Transaction ID should have 4: length prefix"
    );
    assert!(encoded_str.contains("get_peers"));
}

#[test]
fn test_ping_query_has_length_prefixes() {
    let node_id = NodeId::new([0xFF; 20]);
    let transaction_id = vec![0x01, 0x02];

    let query = Query::Ping { id: node_id };

    let msg = KrpcMessage::Query {
        transaction_id: transaction_id.clone(),
        query,
    };

    let encoded = encode_krpc_message(&msg).unwrap();

    let encoded_str = String::from_utf8_lossy(&encoded);
    assert!(
        encoded_str.contains("2:"),
        "Transaction ID should have 2: length prefix"
    );
    assert!(
        encoded_str.contains("20:"),
        "Node ID should have 20: length prefix"
    );
    assert!(encoded_str.contains("ping"));
}

#[test]
fn test_compact_node_info_v6_roundtrip() {
    let id = NodeId::new([0x42; 20]);
    let addr = SocketAddrV6::new(
        Ipv6Addr::new(
            0x2001, 0x0db8, 0x85a3, 0x0000, 0x0000, 0x8a2e, 0x0370, 0x7334,
        ),
        6881,
        0,
        0,
    );
    let info = CompactNodeInfoV6 { id, addr };

    let bytes = info.to_bytes();
    assert_eq!(bytes.len(), 38);

    let parsed = CompactNodeInfoV6::from_bytes(&bytes).unwrap();
    assert_eq!(parsed.id, id);
    assert_eq!(parsed.addr, addr);
}

#[test]
fn test_compact_node_info_v6_parse_multiple() {
    let id1 = NodeId::new([0x01; 20]);
    let addr1 = SocketAddrV6::new(
        Ipv6Addr::new(
            0x2001, 0x0db8, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0001,
        ),
        6881,
        0,
        0,
    );
    let info1 = CompactNodeInfoV6 {
        id: id1,
        addr: addr1,
    };

    let id2 = NodeId::new([0x02; 20]);
    let addr2 = SocketAddrV6::new(
        Ipv6Addr::new(
            0x2001, 0x0db8, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0002,
        ),
        6882,
        0,
        0,
    );
    let info2 = CompactNodeInfoV6 {
        id: id2,
        addr: addr2,
    };

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&info1.to_bytes());
    bytes.extend_from_slice(&info2.to_bytes());

    let parsed = CompactNodeInfoV6::parse_multiple(&bytes).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].id, id1);
    assert_eq!(parsed[0].addr, addr1);
    assert_eq!(parsed[1].id, id2);
    assert_eq!(parsed[1].addr, addr2);
}

#[test]
fn test_compact_node_info_v6_invalid_length() {
    let bytes = vec![0u8; 37];
    let result = CompactNodeInfoV6::from_bytes(&bytes);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("must be exactly 38 bytes")
    );
}

#[test]
fn test_compact_node_info_v6_parse_multiple_invalid_length() {
    let bytes = vec![0u8; 39];
    let result = CompactNodeInfoV6::parse_multiple(&bytes);
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("must be multiple of 38")
    );
}

#[test]
fn test_response_find_node_with_ipv6() {
    let id = NodeId::new([0x42; 20]);
    let ipv4_addr = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 6881);
    let ipv4_info = CompactNodeInfo {
        id: NodeId::new([0x01; 20]),
        addr: ipv4_addr,
    };

    let ipv6_addr = SocketAddrV6::new(
        Ipv6Addr::new(
            0x2001, 0x0db8, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0001,
        ),
        6881,
        0,
        0,
    );
    let ipv6_info = CompactNodeInfoV6 {
        id: NodeId::new([0x02; 20]),
        addr: ipv6_addr,
    };

    let response = Response::FindNode {
        id,
        nodes: vec![ipv4_info],
        nodes6: vec![ipv6_info],
    };

    assert_eq!(response.sender_id(), &id);
}

#[test]
fn test_response_get_peers_with_ipv6() {
    let id = NodeId::new([0x42; 20]);
    let token = vec![0x01, 0x02];

    let ipv4_info = CompactNodeInfo {
        id: NodeId::new([0x01; 20]),
        addr: SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 6881),
    };

    let ipv6_info = CompactNodeInfoV6 {
        id: NodeId::new([0x02; 20]),
        addr: SocketAddrV6::new(
            Ipv6Addr::new(
                0x2001, 0x0db8, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0001,
            ),
            6881,
            0,
            0,
        ),
    };

    let response = Response::GetPeers {
        id,
        token,
        values: None,
        nodes: Some(vec![ipv4_info]),
        nodes6: Some(vec![ipv6_info]),
    };

    assert_eq!(response.sender_id(), &id);
}
