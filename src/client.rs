use std::fmt::{Debug};
use std::{collections::BTreeMap, fs};
use anyhow::{anyhow, Result};
use log::debug;
use tokio::sync::mpsc;
use reqwest::{Client, Url};
use sha1::{Sha1, Digest};

use crate::encoding::Decoder;
use crate::encoding::Encoder;
use crate::types::{BencodeTypes, InterestedMessage, Peer, PeerConnection, PeerMessage};


#[derive(Debug)]
pub struct BitTorrent {
    decoder: Decoder,
    encoder: Encoder,
    metadata: Option<BTreeMap<String, BencodeTypes>>,
    http_client: Client,
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

    fn get_info_hash_as_hex(&self) -> Result<String> {
        let hash = self.get_info_hash()?;
        Ok(hex::encode(hash))
    }

    fn get_info_hash(&self) -> Result<Vec<u8>> {
        let info_dict = self.get_info_dict()?;

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

    fn get_file_size(&self) -> Result<usize> {
        let info_dict = self.get_info_dict()?;

        let length = match info_dict.get("length").ok_or(anyhow!("the length is not present"))? {
            BencodeTypes::Integer(i) => i,
            _ => return Err(anyhow!("the length is not a integer")),
        };

        Ok(*length as usize)
    }

    async fn get_peers(&self) -> Result<Vec<Peer>> {
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
                debug!("[get_peers] body: {}", String::from_utf8_lossy(&body));
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

    async fn create_peer_connection(&mut self, client_peer_id: [u8; 20], peer: &Peer) -> Result<PeerConnection> {
        let info_hash: [u8; 20] = self.get_info_hash()?
            .try_into()
            .map_err(|_| anyhow!("info hash is not 20 bytes"))?;

        let mut peer_conn = PeerConnection::new(peer.clone());
        peer_conn.handshake(client_peer_id, info_hash).await?;
        Ok(peer_conn)
    }

    fn get_num_pieces(&self) -> Result<usize> {
        let info_dict = self.get_info_dict()?;
        let num_pieces = match info_dict.get("piece length").ok_or(anyhow!("the piece length is not present"))? {
            BencodeTypes::Integer(i) => *i as usize,
            _ => return Err(anyhow!("the pieces is not a string")),
        };

        Ok(num_pieces)
    }

    fn get_info_dict(&self) -> Result<&BTreeMap<String, BencodeTypes>> {
        let info = self.metadata
            .as_ref()
            .ok_or(anyhow!("the metadata is not present"))?
            .get("info")
            .ok_or(anyhow!("the info is not present"))?;
    
        let info_dict = match info {
            BencodeTypes::Dictionary(dict) => dict,
            _ => return Err(anyhow!("the info is not a dictionary")),
        };

        Ok(info_dict)
    }

    pub fn print_file_metadata(&self) -> Result<()> {
        let hash = self.get_info_hash_as_hex()?;
        let size = self.get_file_size()?;
        let num_pieces = self.get_num_pieces()?;
        debug!("[print_file_metadata] hash: {:?}, size: {:?}, num_pieces: {:?}", hash, size, num_pieces);
        Ok(())
    }

    // TODO: Evolve this to download the file from multiple peers.
    // Initially this will donwload only a single piece.
    pub async fn download_file(&mut self) -> Result<()> {
        let peers = self.get_peers().await?;
        debug!("[download_file] torrent peers: {:?}", &peers);

        let num_pieces = self.get_num_pieces()?;
        debug!("[download_file] pieces count: {:?}", num_pieces);
        let client_peer_id = *b"postman-000000000001";

        for peer in peers {
            let mut conn = match self.create_peer_connection(client_peer_id, &peer).await {
                Ok(conn) => conn,
                Err(e) => {
                    debug!("[download_file] failed to create peer connection: {:?}", e);
                    continue;
                }
            };
    
            let peer_id: [u8; 20] = conn.get_peer_id().ok_or(anyhow!("the peer id is not present"))?;
            debug!("[download_file] peer connection success: {:?}", &peer_id);

            // Channel for receiving messages FROM the peer
            let (peer_to_client_tx, mut peer_to_client_rx) = mpsc::channel::<PeerMessage>(100);
            // Channel for sending messages TO the peer  
            let (client_to_peer_tx, client_to_peer_rx) = mpsc::channel::<PeerMessage>(100);

            // Spawn task to handle messages RECEIVED from peer
            tokio::task::spawn(async move {
                while let Some(message) = peer_to_client_rx.recv().await {
                    debug!("[download_file] received message from peer: {:?}", &message);
                    if let PeerMessage::Bitfield(bitfield) = message {
                        debug!("[download_file] received bitfield message from peer: {:?}", &bitfield);
                        //client_to_peer_tx.send(PeerMessage::Interested(InterestedMessage{})).await.unwrap();
                        //debug!("[download_file] sent interested message to peer");
                    }
                }
            });

            // Run the peer connection in a separate task
            conn.run(num_pieces, peer_to_client_tx, client_to_peer_rx).await?;
            break; // For now just find one peer that accepts the connection.
        }

        Ok(())
    }
}
