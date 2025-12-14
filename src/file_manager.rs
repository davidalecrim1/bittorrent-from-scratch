#![allow(dead_code)]

use anyhow::{Result, anyhow};
use log::debug;
use sha1::{Digest, Sha1};
use std::fmt::Debug;
use std::sync::Arc;
use std::{collections::BTreeMap, fs};
use tokio::sync::RwLock;

use crate::encoding::Decoder;
use crate::encoding::Encoder;
use crate::peer_manager::PeerManager;
use crate::types::{BencodeTypes, Piece, PieceStatus};

use crate::types::{CompletedPiece, FailedPiece, PeerManagerConfig, PieceDownloadRequest};
use std::collections::HashMap;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::mpsc;

const DEFAULT_BLOCK_SIZE: usize = 16 * 1024; // 16 KiB

#[derive(Debug)]
pub struct BitTorrent {
    // Encoder and Decoder are used to encode and decode
    // the data from a torrent file and some specific messages with tracker.
    decoder: Decoder,
    encoder: Encoder,

    // Store the metadata of the torrent file,
    // after parsing the file.
    metadata: Option<BTreeMap<String, BencodeTypes>>,

    // Keep track of the pieces status.
    pieces: Option<Arc<RwLock<Vec<Piece>>>>,

    // Peer manager to handle the communication with the peers.
    peer_manager: Arc<PeerManager>,
}

impl BitTorrent {
    pub fn new(
        decoder: Decoder,
        encoder: Encoder,
        peer_manager: PeerManager,
        input_file_path: String,
    ) -> Result<Self> {
        let mut torrent = Self {
            decoder,
            encoder,
            metadata: None,
            pieces: None,
            peer_manager: Arc::new(peer_manager),
        };

        torrent.load_file(input_file_path)?;
        torrent.load_pieces()?;
        Ok(torrent)
    }

    fn load_file(&mut self, path: String) -> Result<()> {
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

    fn load_pieces(&mut self) -> Result<()> {
        let n = self.get_num_pieces()?;
        let pieces = (0..n)
            .map(|i| Piece::new(i, PieceStatus::Pending))
            .collect();

        self.pieces = Some(Arc::new(RwLock::new(pieces)));
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

    pub async fn download_file(&mut self, output_directory_path: String) -> Result<()> {
        let info_hash_bytes = self.get_info_hash()?;
        let info_hash: [u8; 20] = info_hash_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Invalid info hash length"))?;

        let file_size = self.get_file_size()?;
        let announce_url = self.get_announce_url()?;
        let piece_length = self.get_piece_length()?;
        let num_pieces = self.get_num_pieces()?;
        let pieces_hashes = self.get_pieces_hashes()?;
        let client_peer_id = *b"postman-000000000001";

        let config = PeerManagerConfig {
            info_hash,
            client_peer_id,
            file_size,
            num_pieces,
            max_peers: 5,
        };

        Arc::get_mut(&mut self.peer_manager)
            .ok_or_else(|| anyhow!("Failed to get mutable reference to PeerManager"))?
            .initialize(config)
            .await?;

        let (completion_tx, mut completion_rx) = mpsc::channel::<CompletedPiece>(100);
        let (failure_tx, mut failure_rx) = mpsc::channel::<FailedPiece>(100);

        self.peer_manager
            .clone()
            .start(announce_url.clone(), completion_tx, failure_tx)
            .await?;

        debug!("[download_file] PeerManager started");

        let filename = self.get_filename()?;
        let output_path = format!("{}/{}", output_directory_path, filename);
        let mut file = File::create(&output_path).await?;
        file.set_len(file_size as u64).await?;
        debug!(
            "[download_file] Created file: {} ({} bytes)",
            output_path, file_size
        );

        tokio::time::sleep(Duration::from_secs(2)).await;

        let piece_requests: Vec<PieceDownloadRequest> = (0..num_pieces)
            .map(|idx| {
                let piece_hash = pieces_hashes[idx];
                PieceDownloadRequest {
                    piece_index: idx as u32,
                    piece_length,
                    expected_hash: piece_hash,
                }
            })
            .collect();

        self.peer_manager.request_pieces(piece_requests).await?;
        debug!("[download_file] Requested all {} pieces", num_pieces);

        let mut downloaded_pieces = 0;
        let mut failed_attempts: HashMap<u32, usize> = HashMap::new();

        while downloaded_pieces < num_pieces {
            tokio::select! {
                Some(completed) = completion_rx.recv() => {
                    let offset = completed.piece_index as u64 * piece_length as u64;
                    file.seek(SeekFrom::Start(offset)).await?;
                    file.write_all(&completed.data).await?;
                    file.flush().await?;

                    downloaded_pieces += 1;

                    let percentage = (downloaded_pieces * 100) / num_pieces;
                    println!(
                        "Downloaded {}/{} pieces ({}%)",
                        downloaded_pieces, num_pieces, percentage
                    );
                }
                Some(failed) = failure_rx.recv() => {
                    let attempts = failed_attempts.entry(failed.piece_index).or_insert(0);
                    *attempts += 1;

                    if *attempts >= 3 {
                        return Err(anyhow!("Failed to download piece {} after 3 attempts", failed.piece_index));
                    }

                    debug!("[download_file] Piece {} failed, will retry", failed.piece_index);
                }
            }
        }

        drop(file);
        debug!("[download_file] File download complete: {}", output_path);

        self.verify_file(&output_path, &pieces_hashes, piece_length, file_size)
            .await?;

        Ok(())
    }

    async fn verify_file(
        &self,
        file_path: &str,
        expected_hashes: &[[u8; 20]],
        piece_length: usize,
        file_size: usize,
    ) -> Result<()> {
        debug!("[verify_file] Starting verification of {}", file_path);

        let mut file = File::open(file_path).await?;
        let mut verified_pieces = 0;

        for (piece_index, expected_hash) in expected_hashes.iter().enumerate() {
            let offset = piece_index * piece_length;
            let remaining = file_size.saturating_sub(offset);
            let current_piece_length = std::cmp::min(piece_length, remaining);

            if current_piece_length == 0 {
                break;
            }

            file.seek(SeekFrom::Start(offset as u64)).await?;

            let mut piece_data = vec![0u8; current_piece_length];
            file.read_exact(&mut piece_data).await?;

            let actual_hash = Sha1::new().chain_update(&piece_data).finalize();
            let actual_hash_bytes: [u8; 20] = actual_hash.into();

            if &actual_hash_bytes != expected_hash {
                return Err(anyhow!(
                    "Hash verification failed for piece {}: expected {:02x?}, got {:02x?}",
                    piece_index,
                    expected_hash,
                    actual_hash_bytes
                ));
            }

            verified_pieces += 1;
            debug!(
                "[verify_file] Piece {} verified ({}/{})",
                piece_index,
                verified_pieces,
                expected_hashes.len()
            );
        }

        debug!(
            "[verify_file] All {} pieces verified successfully",
            verified_pieces
        );
        Ok(())
    }

    fn get_filename(&self) -> Result<String> {
        let info_dict = self.get_info_dict()?;
        match info_dict
            .get("name")
            .ok_or_else(|| anyhow!("name field not found"))?
        {
            BencodeTypes::String(s) => Ok(s.clone()),
            _ => Err(anyhow!("name field is not a string")),
        }
    }

    fn get_announce_url(&self) -> Result<String> {
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow!("Metadata not loaded"))?;

        match metadata
            .get("announce")
            .ok_or_else(|| anyhow!("announce field not found"))?
        {
            BencodeTypes::String(s) => Ok(s.clone()),
            _ => Err(anyhow!("announce field is not a string")),
        }
    }

    fn get_pieces_hashes(&self) -> Result<Vec<[u8; 20]>> {
        let info_dict = self.get_info_dict()?;

        match info_dict
            .get("pieces")
            .ok_or_else(|| anyhow!("pieces field not found"))?
        {
            BencodeTypes::List(hashes) => hashes
                .iter()
                .map(|hash| match hash {
                    BencodeTypes::String(s) => {
                        let bytes = hex::decode(s)?;
                        let arr: [u8; 20] = bytes
                            .as_slice()
                            .try_into()
                            .map_err(|_| anyhow!("Invalid hash length"))?;
                        Ok(arr)
                    }
                    _ => Err(anyhow!("Hash is not a string")),
                })
                .collect(),
            _ => Err(anyhow!("pieces field is not a list")),
        }
    }

}
    //     let peers = self.get_peers().await?;
    //     debug!("[download_file] torrent peers: {:?}", &peers);

    //     let client_peer_id = *b"postman-000000000001";

    //     for peer in peers {
    //         let mut conn = match self.create_peer_connection(client_peer_id, &peer).await {
    //             Ok(conn) => conn,
    //             Err(e) => {
    //                 debug!("[download_file] failed to create peer connection: {:?}", e);
    //                 continue;
    //             }
    //         };

    //         let peer_id: [u8; 20] = conn
    //             .get_peer_id()
    //             .ok_or(anyhow!("the peer id is not present"))?;
    //         debug!("[download_file] peer connection success: {:?}", &peer_id);

    //         // Channel for receiving messages FROM the peer
    //         let (peer_to_client_tx, mut peer_to_client_rx) = mpsc::channel::<PeerMessage>(100);
    //         // Channel for sending messages TO the peer
    //         let (client_to_peer_tx, client_to_peer_rx) = mpsc::channel::<PeerMessage>(100);

    //         let mut download_piece_state = DownloadPieceState::new();

    //         let piece_length = self.get_piece_length()?;

    //         let piece = Arc::new(tokio::sync::Mutex::new(Piece::new(
    //             0,
    //             piece_length,
    //             piece_length / DEFAULT_BLOCK_SIZE,
    //         )));

    //         // Clone Arc references needed in the spawned task
    //         let peer_bitfield_clone = self.peer_bitfield.clone();

    //         // Spawn task to handle messages RECEIVED from peer
    //         tokio::task::spawn(async move {
    //             while let Some(message) = peer_to_client_rx.recv().await {
    //                 debug!("[download_file] received message from peer: {:?}", &message);
    //                 match message {
    //                     PeerMessage::Bitfield(bitfield) => {
    //                         download_piece_state.received_bitfield = true;
    //                         debug!(
    //                             "[download_file] received bitfield message from peer: {:?}",
    //                             &bitfield
    //                         );

    //                         if let Err(e) = client_to_peer_tx
    //                             .send(PeerMessage::Interested(InterestedMessage {}))
    //                             .await
    //                         {
    //                             debug!(
    //                                 "[download_file] failed to send interested message: {:?}",
    //                                 e
    //                             );
    //                             return;
    //                         }

    //                         download_piece_state.sent_interested = true;

    //                         // Store the bitfield message in the map.
    //                         peer_bitfield_clone.lock().await.insert(peer_id, bitfield);

    //                         debug!("[download_file] sent interested message to peer");
    //                     }
    //                     PeerMessage::Unchoke(unchoke) => {
    //                         debug!(
    //                             "[download_file] received unchoke message from peer: {:?}",
    //                             &unchoke
    //                         );

    //                         download_piece_state.received_unchoke = true;
    //                         if download_piece_state.is_ready_for_download() {
    //                             debug!(
    //                                 "[download_file] is ready for downloading pieces from the peer"
    //                             );

    //                             // TODO: Read the bitfield message from the client
    //                             // Copy into this task
    //                             // Use it to download a piece
    //                             // Once I receive the chunk of the piece, have some kind of Arc to store it in the FS.
    //                             // Do this for all chunks
    //                             // Use the Arc for the downloaded piece state to tell the piece is downloaded.

    //                             // All that the task needs here must come from an Arc to avoid ownership problems.
    //                             // Consider using an Arc here for self to help with this.

    //                             // TODO: Make this async to not block the read of messages from the peer.
    //                             let piece_index = {
    //                                 let bitfield_map = peer_bitfield_clone.lock().await;
    //                                 match bitfield_map.get(&peer_id) {
    //                                     Some(bitfield) => {
    //                                         match bitfield.get_first_available_piece() {
    //                                             Some(index) => index,
    //                                             None => {
    //                                                 debug!(
    //                                                     "[download_file] no available pieces from peer"
    //                                                 );
    //                                                 return;
    //                                             }
    //                                         }
    //                                     }
    //                                     None => {
    //                                         debug!("[download_file] peer bitfield not found");
    //                                         return;
    //                                     }
    //                                 }
    //                             }; // Lock is dropped here

    //                             {
    //                                 piece.lock().await.update_idx(piece_index as u32);
    //                             } // Lock is dropped here

    //                             let mut offset = 0;
    //                             let mut block_index = 0;
    //                             while offset < piece_length {
    //                                 let size =
    //                                     std::cmp::min(DEFAULT_BLOCK_SIZE, piece_length - offset);
    //                                 debug!(
    //                                     "[download_file] Block {}: offset={}, size={}",
    //                                     block_index, offset, size
    //                                 );
    //                                 offset += size;
    //                                 block_index += 1;

    //                                 if let Err(e) = client_to_peer_tx
    //                                     .send(PeerMessage::Request(RequestMessage {
    //                                         piece_index: piece_index as u32,
    //                                         begin: offset as u32,
    //                                         length: size as u32,
    //                                     }))
    //                                     .await
    //                                 {
    //                                     debug!(
    //                                         "[download_file] failed to send request message: {:?}",
    //                                         e
    //                                     );
    //                                     return;
    //                                 }
    //                             }

    //                             debug!(
    //                                 "[download_file] send a request message for each block of the piece {}",
    //                                 piece_index
    //                             );
    //                         }
    //                     }
    //                     PeerMessage::Piece(message) => {
    //                         debug!(
    //                             "[download_file] received piece message from peer: {:?}",
    //                             &message
    //                         );

    //                         {
    //                             let mut p = piece.lock().await;

    //                             if let Err(e) = p.add_block(message) {
    //                                 debug!("[download_file] error adding block to piece: {:?}", e);
    //                             }

    //                             if p.is_complete() {
    //                                 debug!("[download_file] piece is complete");
    //                                 // TODO: Write the piece to the file system.
    //                             }
    //                         }
    //                     }
    //                     _ => {
    //                         debug!(
    //                             "[download_file] received invalid message from peer: {:?}",
    //                             &message
    //                         );
    //                     }
    //                 }
    //             }
    //         });

    //         let num_pieces = self.get_num_pieces()?;

    //         // Run the peer connection in a separate task
    //         // Writes will be handled here when sent through the `client_to_peer_tx` channel.
    //         // Reads will be notified in the channel `peer_to_client_tx` that is read above.
    //         conn.run(num_pieces, peer_to_client_tx, client_to_peer_rx)
    //             .await?;
    //         break; // For now just find one peer that accepts the connection.
    //     }

    //     Ok(())
    // }
}
