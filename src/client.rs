use anyhow::{Result, anyhow};
use futures_util::lock::Mutex;
use log::debug;
use reqwest::{Client, Url};
use sha1::{Digest, Sha1};
use std::fmt::Debug;
use std::sync::Arc;
use std::{collections::BTreeMap, collections::HashMap, fs};
use tokio::sync::mpsc;

use crate::encoding::Decoder;
use crate::encoding::Encoder;
use crate::types::{
    BencodeTypes, BitfieldMessage, InterestedMessage, Peer, PeerConnection, PeerId, PeerMessage,
    Piece, RequestMessage,
};

const DEFAULT_BLOCK_SIZE: usize = 16 * 1024; // 16 KiB

#[derive(Debug)]
pub struct BitTorrent {
    decoder: Decoder,
    encoder: Encoder,
    metadata: Option<BTreeMap<String, BencodeTypes>>,
    http_client: Client,

    // Keep track of the pieces that have been downloaded.
    downloaded_pieces: Arc<Mutex<HashMap<u32, bool>>>,

    // Store the bitfield message from the peer.
    peer_bitfield: Arc<Mutex<HashMap<PeerId, BitfieldMessage>>>,
}

impl BitTorrent {
    pub fn new(decoder: Decoder, encoder: Encoder, http_client: Client) -> Self {
        Self {
            decoder,
            encoder,
            metadata: None,
            http_client,
            downloaded_pieces: Arc::new(Mutex::new(HashMap::new())),
            peer_bitfield: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn load_file(&mut self, path: &str) -> Result<()> {
        let bytes = fs::read(path)?;
        debug!(
            "[BitTorrent] bytes length loaded from file: {:?}",
            bytes.len()
        );

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

        let bytes = self
            .encoder
            .from_bencode_types(BencodeTypes::Dictionary(info_dict.clone()))?;
        Ok(Sha1::new().chain_update(&bytes).finalize().to_vec())
    }

    fn get_info_hash_as_url_encoded(&self) -> Result<String> {
        let hash = self.get_info_hash_as_hex()?;
        let raw_bytes: Vec<u8> = hex::decode(hash)?;

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

        let length = match info_dict
            .get("length")
            .ok_or(anyhow!("the length is not present"))?
        {
            BencodeTypes::Integer(i) => i,
            _ => return Err(anyhow!("the length is not a integer")),
        };

        Ok(*length as usize)
    }

    async fn get_peers(&self) -> Result<Vec<Peer>> {
        let endpoint = self
            .metadata
            .as_ref()
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
        let raw_query = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        let mut url = Url::parse(endpoint)?;
        url.set_query(Some(&raw_query)); // <- keeps it as-is, no re-encoding

        let req = self.http_client.get(url).build()?;

        let res = self.http_client.execute(req).await;
        let val: BencodeTypes = match res {
            Ok(res) => {
                if !res.status().is_success() {
                    return Err(anyhow!(
                        "the request to the tracker server failed, status: {:?}",
                        res.status()
                    ));
                }
                let body = res.bytes().await?;
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
                let peers_bencode = dict
                    .get("peers")
                    .ok_or(anyhow!("peers field not found in response"))?;

                match peers_bencode {
                    BencodeTypes::List(peers_list) => {
                        let mut peers = Vec::new();
                        for peer_bencode in peers_list {
                            match peer_bencode {
                                BencodeTypes::Dictionary(peer_dict) => {
                                    let ip = peer_dict
                                        .get("ip")
                                        .ok_or(anyhow!("ip field not found in peer"))?;

                                    let port = peer_dict
                                        .get("port")
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
                                        }
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

    async fn create_peer_connection(
        &mut self,
        client_peer_id: [u8; 20],
        peer: &Peer,
    ) -> Result<PeerConnection> {
        let info_hash: [u8; 20] = self
            .get_info_hash()?
            .try_into()
            .map_err(|_| anyhow!("info hash is not 20 bytes"))?;

        let mut peer_conn = PeerConnection::new(peer.clone());
        peer_conn.handshake(client_peer_id, info_hash).await?;
        Ok(peer_conn)
    }

    fn get_num_pieces(&self) -> Result<usize> {
        let info_dict = self.get_info_dict()?;
        let num_pieces = match info_dict
            .get("pieces")
            .ok_or(anyhow!("the num pieces is not present"))?
        {
            BencodeTypes::List(i) => i.len(),
            _ => return Err(anyhow!("the num pieces is not a string")),
        };
        Ok(num_pieces)
    }

    fn get_piece_length(&self) -> Result<usize> {
        let info_dict = self.get_info_dict()?;
        let num_pieces = match info_dict
            .get("piece length")
            .ok_or(anyhow!("the piece length is not present"))?
        {
            BencodeTypes::Integer(i) => *i as usize,
            _ => return Err(anyhow!("the pieces is not a string")),
        };

        Ok(num_pieces)
    }

    fn get_info_dict(&self) -> Result<&BTreeMap<String, BencodeTypes>> {
        let info = self
            .metadata
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
        let piece_length = self.get_piece_length()?;
        debug!(
            "[print_file_metadata] hash: {:?}, size: {:?}, piece_length: {:?}",
            hash, size, piece_length
        );
        Ok(())
    }

    // TODO: Evolve this to download the file from multiple peers.
    // Initially this will donwload only a single piece.
    pub async fn download_file(&mut self) -> Result<()> {
        let peers = self.get_peers().await?;
        debug!("[download_file] torrent peers: {:?}", &peers);

        let client_peer_id = *b"postman-000000000001";

        for peer in peers {
            let mut conn = match self.create_peer_connection(client_peer_id, &peer).await {
                Ok(conn) => conn,
                Err(e) => {
                    debug!("[download_file] failed to create peer connection: {:?}", e);
                    continue;
                }
            };

            let peer_id: [u8; 20] = conn
                .get_peer_id()
                .ok_or(anyhow!("the peer id is not present"))?;
            debug!("[download_file] peer connection success: {:?}", &peer_id);

            // Channel for receiving messages FROM the peer
            let (peer_to_client_tx, mut peer_to_client_rx) = mpsc::channel::<PeerMessage>(100);
            // Channel for sending messages TO the peer
            let (client_to_peer_tx, client_to_peer_rx) = mpsc::channel::<PeerMessage>(100);

            let mut download_piece_state = DownloadPieceState::new();

            let piece_length = self.get_piece_length()?;

            let piece = Arc::new(tokio::sync::Mutex::new(Piece::new(
                0,
                piece_length,
                piece_length / DEFAULT_BLOCK_SIZE,
            )));

            // Clone Arc references needed in the spawned task
            let peer_bitfield_clone = self.peer_bitfield.clone();

            // Spawn task to handle messages RECEIVED from peer
            tokio::task::spawn(async move {
                while let Some(message) = peer_to_client_rx.recv().await {
                    debug!("[download_file] received message from peer: {:?}", &message);
                    match message {
                        PeerMessage::Bitfield(bitfield) => {
                            download_piece_state.received_bitfield = true;
                            debug!(
                                "[download_file] received bitfield message from peer: {:?}",
                                &bitfield
                            );

                            if let Err(e) = client_to_peer_tx
                                .send(PeerMessage::Interested(InterestedMessage {}))
                                .await
                            {
                                debug!(
                                    "[download_file] failed to send interested message: {:?}",
                                    e
                                );
                                return;
                            }

                            download_piece_state.sent_interested = true;

                            // Store the bitfield message in the map.
                            peer_bitfield_clone.lock().await.insert(peer_id, bitfield);

                            debug!("[download_file] sent interested message to peer");
                        }
                        PeerMessage::Unchoke(unchoke) => {
                            debug!(
                                "[download_file] received unchoke message from peer: {:?}",
                                &unchoke
                            );

                            download_piece_state.received_unchoke = true;
                            if download_piece_state.is_ready_for_download() {
                                debug!(
                                    "[download_file] is ready for downloading pieces from the peer"
                                );

                                // TODO: Read the bitfield message from the client
                                // Copy into this task
                                // Use it to download a piece
                                // Once I receive the chunk of the piece, have some kind of Arc to store it in the FS.
                                // Do this for all chunks
                                // Use the Arc for the downloaded piece state to tell the piece is downloaded.

                                // All that the task needs here must come from an Arc to avoid ownership problems.
                                // Consider using an Arc here for self to help with this.

                                // TODO: Make this async to not block the read of messages from the peer.
                                let piece_index = {
                                    let bitfield_map = peer_bitfield_clone.lock().await;
                                    match bitfield_map.get(&peer_id) {
                                        Some(bitfield) => {
                                            match bitfield.get_first_available_piece() {
                                                Some(index) => index,
                                                None => {
                                                    debug!(
                                                        "[download_file] no available pieces from peer"
                                                    );
                                                    return;
                                                }
                                            }
                                        }
                                        None => {
                                            debug!("[download_file] peer bitfield not found");
                                            return;
                                        }
                                    }
                                }; // Lock is dropped here

                                {
                                    piece.lock().await.update_idx(piece_index as u32);
                                } // Lock is dropped here

                                let mut offset = 0;
                                let mut block_index = 0;
                                while offset < piece_length {
                                    let size =
                                        std::cmp::min(DEFAULT_BLOCK_SIZE, piece_length - offset);
                                    debug!(
                                        "[download_file] Block {}: offset={}, size={}",
                                        block_index, offset, size
                                    );
                                    offset += size;
                                    block_index += 1;

                                    if let Err(e) = client_to_peer_tx
                                        .send(PeerMessage::Request(RequestMessage {
                                            piece_index: piece_index as u32,
                                            begin: offset as u32,
                                            length: size as u32,
                                        }))
                                        .await
                                    {
                                        debug!(
                                            "[download_file] failed to send request message: {:?}",
                                            e
                                        );
                                        return;
                                    }
                                }

                                debug!(
                                    "[download_file] send a request message for each block of the piece {}",
                                    piece_index
                                );
                            }
                        }
                        PeerMessage::Piece(message) => {
                            debug!(
                                "[download_file] received piece message from peer: {:?}",
                                &message
                            );

                            {
                                let mut p = piece.lock().await;

                                if let Err(e) = p.add_block(message) {
                                    debug!("[download_file] error adding block to piece: {:?}", e);
                                }

                                if p.is_complete() {
                                    debug!("[download_file] piece is complete");
                                    // TODO: Write the piece to the file system.
                                }
                            }
                        }
                        _ => {
                            debug!(
                                "[download_file] received invalid message from peer: {:?}",
                                &message
                            );
                        }
                    }
                }
            });

            let num_pieces = self.get_num_pieces()?;

            // Run the peer connection in a separate task
            // Writes will be handled here when sent through the `client_to_peer_tx` channel.
            // Reads will be notified in the channel `peer_to_client_tx` that is read above.
            conn.run(num_pieces, peer_to_client_tx, client_to_peer_rx)
                .await?;
            break; // For now just find one peer that accepts the connection.
        }

        Ok(())
    }
}

// Used to keep track of the state of the download of a piece.
// When all the messages are received, the peer is ready for
// the client to download the available pieces.
struct DownloadPieceState {
    pub sent_interested: bool,
    pub received_bitfield: bool,
    pub received_unchoke: bool,
}

impl DownloadPieceState {
    fn new() -> Self {
        Self {
            sent_interested: false,
            received_bitfield: false,
            received_unchoke: false,
        }
    }

    fn is_ready_for_download(&self) -> bool {
        debug!(
            "[is_ready_for_download] sent_interested: {:?}, received_bitfield: {:?}, received_unchoke: {:?}",
            self.sent_interested, self.received_bitfield, self.received_unchoke
        );
        self.sent_interested && self.received_bitfield && self.received_unchoke
    }
}
