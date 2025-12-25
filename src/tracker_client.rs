use crate::encoding::Decoder;
use crate::error::AppError;
use crate::types::{AnnounceRequest, AnnounceResponse, BencodeTypes, Peer};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::{Client, Url};
use std::net::Ipv6Addr;

/// Abstraction for tracker HTTP communication.
/// Allows testing peer discovery logic without making real HTTP requests.
#[async_trait]
pub trait TrackerClient: Send + Sync {
    /// Send an announce request to the tracker.
    async fn announce(&self, request: AnnounceRequest) -> Result<AnnounceResponse>;
}

pub struct HttpTrackerClient {
    http_client: Client,
    decoder: Decoder,
}

impl HttpTrackerClient {
    pub fn new(http_client: Client, decoder: Decoder) -> Self {
        Self {
            http_client,
            decoder,
        }
    }

    // TODO: This code it too nested. This needs to be refactores. And why is the decoder not being used to parse the types?
    // Is there something we can reuse to make this better and less nested?
    fn parse_peers(&self, val: BencodeTypes) -> Result<Vec<Peer>> {
        match val {
            BencodeTypes::Dictionary(dict) => {
                if let Some(failure_reason) = dict.get("failure reason") {
                    let reason = match failure_reason {
                        BencodeTypes::String(s) => s.clone(),
                        _ => "Unknown failure reason".to_string(),
                    };
                    return Err(AppError::tracker_rejected(reason).into());
                }

                let peers_bencode = dict.get("peers").ok_or_else(|| {
                    anyhow::Error::from(AppError::missing_field(
                        "peers field not found in response",
                    ))
                })?;

                // Extract interval if present
                let _interval = dict.get("interval").and_then(|v| match v {
                    BencodeTypes::Integer(i) => Some(*i as u64),
                    _ => None,
                });

                match peers_bencode {
                    BencodeTypes::List(peers_list) => {
                        let mut peers = Vec::with_capacity(50);
                        for peer_bencode in peers_list {
                            match peer_bencode {
                                BencodeTypes::Dictionary(peer_dict) => {
                                    let ip = peer_dict.get("ip").ok_or_else(|| {
                                        anyhow::Error::from(AppError::missing_field(
                                            "ip field not found in peer",
                                        ))
                                    })?;

                                    let port = peer_dict.get("port").ok_or_else(|| {
                                        anyhow::Error::from(AppError::missing_field(
                                            "port field not found in peer",
                                        ))
                                    })?;

                                    let ip_string = match ip {
                                        BencodeTypes::String(s) => s.clone(),
                                        _ => {
                                            return Err(AppError::InvalidFieldType {
                                                field: "ip".to_string(),
                                                expected: "string".to_string(),
                                            }
                                            .into());
                                        }
                                    };

                                    let port_u16 = match port {
                                        BencodeTypes::Integer(i) => {
                                            if *i < 0 || *i > u16::MAX as isize {
                                                return Err(AppError::invalid_bencode(
                                                    "port value out of range",
                                                )
                                                .into());
                                            }
                                            *i as u16
                                        }
                                        _ => {
                                            return Err(AppError::InvalidFieldType {
                                                field: "port".to_string(),
                                                expected: "integer".to_string(),
                                            }
                                            .into());
                                        }
                                    };

                                    peers.push(Peer::new(ip_string, port_u16));
                                }
                                _ => {
                                    return Err(AppError::InvalidFieldType {
                                        field: "peer".to_string(),
                                        expected: "dictionary".to_string(),
                                    }
                                    .into());
                                }
                            }
                        }
                        Ok(peers)
                    }
                    BencodeTypes::Raw(peer_bytes) => {
                        let mut peers = Vec::new();

                        // Auto-detect format based on byte length
                        if peer_bytes.len() % 18 == 0 {
                            // IPv6 compact format: 18 bytes per peer (16 IP + 2 port)
                            for chunk in peer_bytes.chunks(18) {
                                if chunk.len() == 18 {
                                    let ip_bytes: [u8; 16] = match chunk[0..16].try_into() {
                                        Ok(bytes) => bytes,
                                        Err(_) => continue,
                                    };
                                    let ip = Ipv6Addr::from(ip_bytes);
                                    let port = u16::from_be_bytes([chunk[16], chunk[17]]);
                                    peers.push(Peer::new(ip.to_string(), port));
                                }
                            }
                        } else if peer_bytes.len() % 6 == 0 {
                            // IPv4 compact format: 6 bytes per peer (4 IP + 2 port)
                            for chunk in peer_bytes.chunks(6) {
                                if chunk.len() == 6 {
                                    let ip = format!(
                                        "{}.{}.{}.{}",
                                        chunk[0], chunk[1], chunk[2], chunk[3]
                                    );
                                    let port = u16::from_be_bytes([chunk[4], chunk[5]]);
                                    peers.push(Peer::new(ip, port));
                                }
                            }
                        }
                        // Mixed or invalid format - return what we got (possibly empty)
                        Ok(peers)
                    }
                    _ => Err(AppError::InvalidFieldType {
                        field: "peers".to_string(),
                        expected: "list or raw bytes".to_string(),
                    }
                    .into()),
                }
            }
            _ => Err(AppError::invalid_bencode("expected dictionary at top level").into()),
        }
    }
}

#[async_trait]
impl TrackerClient for HttpTrackerClient {
    async fn announce(&self, request: AnnounceRequest) -> Result<AnnounceResponse> {
        // URL-encode the info_hash (percent-encoding, not hex)
        let info_hash_encoded: String = request
            .info_hash
            .iter()
            .map(|b| format!("%{:02X}", b))
            .collect();

        let params = [
            ("info_hash", info_hash_encoded),
            ("port", request.port.to_string()),
            ("uploaded", request.uploaded.to_string()),
            ("peer_id", request.peer_id.clone()),
            ("downloaded", request.downloaded.to_string()),
            ("left", request.left.to_string()),
        ];

        let raw_query = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");

        let mut url = Url::parse(&request.endpoint)?;
        url.set_query(Some(&raw_query));

        let req = self.http_client.get(url).build()?;
        let res = self.http_client.execute(req).await?;

        if !res.status().is_success() {
            return Err(AppError::tracker_rejected(format!(
                "Tracker returned status: {}",
                res.status()
            ))
            .into());
        }

        let body = res.bytes().await?;
        let (_, val) = self.decoder.from_bytes(&body)?;

        let peers = self.parse_peers(val.clone())?;

        // Extract interval if present
        let interval = if let BencodeTypes::Dictionary(dict) = val {
            dict.get("interval").and_then(|v| match v {
                BencodeTypes::Integer(i) => Some(*i as u64),
                _ => None,
            })
        } else {
            None
        };

        Ok(AnnounceResponse { peers, interval })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ipv4_compact_peers() {
        // IPv4 compact format: 6 bytes per peer (4 IP + 2 port)
        let peer_bytes = vec![
            192, 168, 1, 1, 0x1A, 0xE1, // 192.168.1.1:6881 (0x1AE1 = 6881)
            10, 0, 0, 5, 0x1A, 0xE2, // 10.0.0.5:6882 (0x1AE2 = 6882)
        ];

        let bencode = BencodeTypes::Raw(peer_bytes);
        let dict = std::collections::BTreeMap::from([("peers".to_string(), bencode)]);

        let decoder = crate::encoding::Decoder {};
        let client = HttpTrackerClient::new(reqwest::Client::new(), decoder);

        let result = client.parse_peers(BencodeTypes::Dictionary(dict));
        assert!(result.is_ok(), "Should parse IPv4 compact format");

        let peers = result.unwrap();
        assert_eq!(peers.len(), 2, "Should have 2 peers");
        assert_eq!(peers[0].get_addr(), "192.168.1.1:6881");
        assert_eq!(peers[1].get_addr(), "10.0.0.5:6882");
    }

    #[test]
    fn test_parse_ipv6_compact_peers() {
        // IPv6 compact format: 18 bytes per peer (16 IP + 2 port)
        let peer_bytes = vec![
            // First peer: 2001:db8::1:6881
            0x20, 0x01, 0x0d, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01, 0x1A, 0xE1, // port 6881
            // Second peer: fe80::1:6882
            0xfe, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01, 0x1A, 0xE2, // port 6882
        ];

        let bencode = BencodeTypes::Raw(peer_bytes);
        let dict = std::collections::BTreeMap::from([("peers".to_string(), bencode)]);

        let decoder = crate::encoding::Decoder {};
        let client = HttpTrackerClient::new(reqwest::Client::new(), decoder);

        let result = client.parse_peers(BencodeTypes::Dictionary(dict));
        assert!(result.is_ok(), "Should parse IPv6 compact format");

        let peers = result.unwrap();
        assert_eq!(peers.len(), 2, "Should have 2 peers");
        assert_eq!(peers[0].get_addr(), "[2001:db8::1]:6881");
        assert_eq!(peers[1].get_addr(), "[fe80::1]:6882");
    }

    #[test]
    fn test_parse_empty_peers() {
        let peer_bytes = vec![];
        let bencode = BencodeTypes::Raw(peer_bytes);
        let dict = std::collections::BTreeMap::from([("peers".to_string(), bencode)]);

        let decoder = crate::encoding::Decoder {};
        let client = HttpTrackerClient::new(reqwest::Client::new(), decoder);

        let result = client.parse_peers(BencodeTypes::Dictionary(dict));
        assert!(result.is_ok(), "Should handle empty peer list");

        let peers = result.unwrap();
        assert_eq!(peers.len(), 0, "Should have 0 peers");
    }

    #[test]
    fn test_parse_invalid_peer_length() {
        // Invalid: 7 bytes (not divisible by 6 or 18)
        let peer_bytes = vec![192, 168, 1, 1, 0x1A, 0xE1, 0xFF];
        let bencode = BencodeTypes::Raw(peer_bytes);
        let dict = std::collections::BTreeMap::from([("peers".to_string(), bencode)]);

        let decoder = crate::encoding::Decoder {};
        let client = HttpTrackerClient::new(reqwest::Client::new(), decoder);

        let result = client.parse_peers(BencodeTypes::Dictionary(dict));
        assert!(result.is_ok(), "Should handle invalid length gracefully");

        let peers = result.unwrap();
        assert_eq!(
            peers.len(),
            0,
            "Should return empty list for invalid format"
        );
    }
}
