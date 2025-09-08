use std::fmt::{Debug};
use std::{collections::BTreeMap, fs};
use anyhow::{anyhow, Result};
use log::debug;
use reqwest::{Client, Url};
use sha1::{Sha1, Digest};

use crate::encoding::Decoder;
use crate::encoding::Encoder;
use crate::types::BencodeTypes;
use crate::types::Peer;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug)]
pub struct BitTorrent {
    decoder: Decoder,
    encoder: Encoder,
    metadata: Option<BTreeMap<String, BencodeTypes>>,
    http_client: Client
}

impl BitTorrent {
    pub fn new(decoder: Decoder, encoder: Encoder, http_client: Client) -> Self {
        Self { decoder, encoder, metadata: None, http_client }
    }

    pub fn load_file(&mut self,path: &str) -> Result<()>{
        let bytes = fs::read(path)?;
        debug!("[BitTorrent] bytes length loaded from file: {:?}", bytes.len());

        let (n, val) = self.decoder.from_bytes(&bytes)?;
        debug!("[BitTorrent] bytes length decoded: {:?}", n);

        match val {
            BencodeTypes::Dictionary(val) => {
                self.metadata = Some(val);
            }
            _ => {
                return Err(anyhow!("the provided data is not a valid torrent file"));
            }
        }
        Ok(())
    }

    pub fn get_info_hash_as_hex(&self) -> Result<String> {
        let hash = self.get_info_hash()?;
        Ok(hex::encode(hash))
    }

    fn get_info_hash(&self) -> Result<Vec<u8>> {
        let info = self.metadata
        .as_ref()
        .ok_or(anyhow!("the metadata is not present"))?
        .get("info")
        .ok_or(anyhow!("the info is not present"))?;

        let info_dict = match info {
            BencodeTypes::Dictionary(dict) => dict,
            _ => return Err(anyhow!("the info is not a dictionary")),
        };

        let bytes = self.encoder.from_bencode_types(BencodeTypes::Dictionary(info_dict.clone()))?;
        Ok(Sha1::new().chain_update(&bytes).finalize().to_vec())
    }

    fn get_info_hash_as_url_encoded(&self) -> Result<String> {
        let hash = self.get_info_hash_as_hex()?;
        let raw_bytes: Vec<u8> = hex::decode(hash).unwrap();
    
        let formatted: String = raw_bytes
            .iter()
            .map(|b| format!("%{:02X}", b))
            .collect::<Vec<_>>()
            .join("")
            .to_lowercase();
    
        Ok(formatted)
    }

    pub fn get_file_size(&self) -> Result<usize> {
        let info = self.metadata
            .as_ref()
            .ok_or(anyhow!("the metadata is not present"))?
            .get("info")
            .ok_or(anyhow!("the info is not present"))?;

        let info_dict = match info {
            BencodeTypes::Dictionary(dict) => dict,
            _ => return Err(anyhow!("the info is not a dictionary")),
        };

        let length = match info_dict.get("length").ok_or(anyhow!("the length is not present"))? {
            BencodeTypes::Integer(i) => i,
            _ => return Err(anyhow!("the length is not a integer")),
        };

        Ok(*length as usize)
    }

    pub async fn get_peers(&self) -> Result<Vec<Peer>> {
        let endpoint = self.metadata.as_ref()
        .ok_or(anyhow!("the metadata is not present"))?
        .get("announce")
        .ok_or(anyhow!("the announce is not present"))?;

        let endpoint = match endpoint {
            BencodeTypes::String(s) => s,
            _ => return Err(anyhow!("the announce is not a string")),
        };

        let params = [
            ("info_hash", self.get_info_hash_as_url_encoded()?),
            ("port", "6881".to_string()),
            ("uploaded", "0".to_string()),
            ("peer_id", "postman-000000000001".to_string()),
            ("downloaded", "0".to_string()),
            ("left", self.get_file_size()?.to_string()),
            ("compact", "1".to_string()),
        ];

        // The info_hash was being messed up if we send using the .query() method.
        // This way avoids reqwest from re-encoding the info_hash.
        let raw_query = params.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join("&");
        let mut url = Url::parse(endpoint)?;
        url.set_query(Some(&raw_query)); // <- keeps it as-is, no re-encoding

        let req = self.http_client
            .get(url)
            .build()
            .unwrap();

        let res = self.http_client.execute(req).await;
        let val: BencodeTypes = match res {
            Ok(res) => {
                if !res.status().is_success() {
                    return Err(anyhow!("the request to the tracker server failed, status: {:?}", res.status()));
                }
                let body = res.bytes().await.unwrap();
                dbg!("[get_peers] body: {:?}", String::from_utf8_lossy(&body));
                let (_, val) = self.decoder.from_bytes(&body)?;
                val
            }
            Err(e) => {
                return Err(anyhow!("the request to the tracker server failed: {}", e));
            }
        };

        Ok(self.to_peers(val)?)
    }

    fn to_peers(&self, val: BencodeTypes) -> Result<Vec<Peer>> {
        match val {
            BencodeTypes::Dictionary(dict) => {
                let peers_bencode = dict.get("peers")
                    .ok_or(anyhow!("peers field not found in response"))?;
                
                match peers_bencode {
                    BencodeTypes::List(peers_list) => {
                        let mut peers = Vec::new();
                        for peer_bencode in peers_list {
                            match peer_bencode {
                                BencodeTypes::Dictionary(peer_dict) => {
                                    let ip = peer_dict.get("ip")
                                        .ok_or(anyhow!("ip field not found in peer"))?;

                                    let port = peer_dict.get("port")
                                        .ok_or(anyhow!("port field not found in peer"))?;
                                    
                                    let ip_string = match ip {
                                        BencodeTypes::String(s) => s.clone(),
                                        _ => return Err(anyhow!("ip field is not a string")),
                                    };
                                    
                                    let port_u16 = match port {
                                        BencodeTypes::Integer(i) => {
                                            if *i < 0 || *i > u16::MAX as isize {
                                                return Err(anyhow!("port value out of range"));
                                            }
                                            *i as u16
                                        },
                                        _ => return Err(anyhow!("port field is not an integer")),
                                    };
                                    
                                    peers.push(Peer::new(ip_string, port_u16));
                                }
                                _ => return Err(anyhow!("peer is not a dictionary")),
                            }
                        }
                        Ok(peers)
                    }
                    _ => Err(anyhow!("peers field is not a list")),
                }
            }
            _ => Err(anyhow!("response is not a dictionary")),
        }
    }

    pub async fn handshake_with_peer(&self, client_peer_id: [u8; 20], peer: &Peer) -> Result<PeerHandshake> {
        let info_hash = self.get_info_hash()?
            .try_into()
            .map_err(|_| anyhow!("info hash is not 20 bytes"))?;

        let handshake = PeerHandshake::new(info_hash, client_peer_id);
        let bytes = handshake.to_bytes();

        let mut stream = TcpStream::connect(format!("{}:{}", &peer.ip, &peer.port)).await?;

        stream.write_all(&bytes).await?;

        let mut buffer = vec![0u8; 68]; // the response is always 68 bytes
        let n = stream.read(&mut buffer).await?;

        let response = PeerHandshake::from_bytes(&buffer[..n])?;
        Ok(response)
    }

}

pub struct PeerHandshake {
    info_hash: [u8; 20], // NOT the hexadecimal string, but the actual bytes
    peer_id: [u8; 20],
    reserved: [u8; 8],
}

impl PeerHandshake {
    pub fn new(info_hash: [u8; 20], peer_id: [u8; 20]) -> Self {
        Self {
            info_hash,
            peer_id,
            reserved: [0; 8],
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(68);
        // pstrlen (1 byte)
        bytes.push(19);
        // pstr (19 bytes)
        bytes.extend_from_slice(b"BitTorrent protocol");
        // reserved (8 bytes)
        bytes.extend_from_slice(&self.reserved);
        // info_hash (20 bytes)
        bytes.extend_from_slice(&self.info_hash);
        // peer_id (20 bytes)
        bytes.extend_from_slice(&self.peer_id);

        return bytes;
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 68 {
            return Err(anyhow!("the provided data is not a valid handshake"));
        }

        let info_hash = &bytes[19..39];
        let peer_id = &bytes[39..59];
        let reserved = &bytes[59..67];

        Ok(Self {
            info_hash: info_hash.try_into()?,
            peer_id: peer_id.try_into()?,
            reserved: reserved.try_into()?,
        })
    }
}

impl Debug for PeerHandshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let info_hash = hex::encode(self.info_hash);
        let peer_id = hex::encode(self.peer_id);
        write!(f, "PeerHandshake {{ info_hash: {:?}, peer_id: {:?} }}", info_hash, peer_id)
    }
}