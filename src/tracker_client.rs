use crate::encoding::Decoder;
use crate::error::AppError;
use crate::traits::{AnnounceRequest, AnnounceResponse, TrackerClient};
use crate::types::{BencodeTypes, Peer};
use anyhow::Result;
use async_trait::async_trait;
use reqwest::{Client, Url};

/// Production implementation of TrackerClient using HTTP
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
                        // Compact format: 6 bytes per peer (4 for IP, 2 for port)
                        let mut peers = Vec::new();
                        for chunk in peer_bytes.chunks(6) {
                            if chunk.len() == 6 {
                                let ip =
                                    format!("{}.{}.{}.{}", chunk[0], chunk[1], chunk[2], chunk[3]);
                                let port = u16::from_be_bytes([chunk[4], chunk[5]]);
                                peers.push(Peer::new(ip, port));
                            }
                        }
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
