use anyhow::{Result, anyhow};
use sha1::{Digest, Sha1};
use std::sync::Arc;
use std::{collections::BTreeMap, fs};
use tokio::sync::RwLock;

use crate::encoding::Decoder;
use crate::encoding::Encoder;
use crate::peer_manager::PeerManager;
use crate::types::{BencodeTypes, Piece, PieceStatus};
use crate::types::{CompletedPiece, DownloadComplete, PeerManagerConfig, PieceDownloadRequest};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::mpsc::{self, Receiver};

use log::{debug, info};

const MAX_PEERS_TO_CONNECT: usize = 50;

pub struct BitTorrent {
    // Encoder and Decoder are used to encode and decode
    // the data from a torrent file and some specific messages with tracker.
    decoder: Decoder,
    encoder: Encoder,

    // Store the metadata of the torrent file,
    // after parsing the file.
    metadata: Option<BTreeMap<String, BencodeTypes>>,

    // Keep track of the pieces status.
    // TODO: Remove this and control all pieces within the peer manager.
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

    // Loads the torrent source file to extract metadata.
    fn load_file(&mut self, path: String) -> Result<()> {
        let bytes = fs::read(path)?;

        let (_n, val) = self.decoder.from_bytes(&bytes)?;

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

    // Loads the desired file pieces from the torrent source file metadata.
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
        let _hash = self.get_info_hash_as_hex()?;
        let _size = self.get_file_size()?;
        let _piece_length = self.get_piece_length()?;
        Ok(())
    }

    // Starts downloading the desired file using the torrent source file metadata.
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
        let client_peer_id = *b"postman-000000000001"; // TODO: Improve this to something clearer.

        let config = PeerManagerConfig {
            info_hash,
            client_peer_id,
            file_size,
            num_pieces,
            max_peers: MAX_PEERS_TO_CONNECT,
        };

        Arc::get_mut(&mut self.peer_manager)
            .ok_or_else(|| anyhow!("Failed to get mutable reference to PeerManager"))?
            .initialize(config)
            .await?;

        // Peer Manager will let the File Manager know when the pieces are completed.
        let (completion_tx, mut completion_rx) = mpsc::channel::<CompletedPiece>(100);
        let (download_complete_tx, mut download_complete_rx) = mpsc::channel::<DownloadComplete>(1);

        self.peer_manager
            .clone()
            .start(announce_url.clone(), completion_tx, download_complete_tx)
            .await?;

        let filename = self.get_filename()?;
        let output_path = format!("{}/{}", output_directory_path, filename);

        let mut file = File::create(&output_path).await?;
        file.set_len(file_size as u64).await?;

        // TODO: This should be refactored to be moved completely to the Peer Manager.
        // The File Manager should only care about completed pieces. So the start function should be responsible
        // for this in the Peer Manager.
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

        self.watch_for_completed_pieces(
            &mut file,
            &mut completion_rx,
            &mut download_complete_rx,
            num_pieces,
            piece_length,
        )
        .await?;

        self.verify_file(&output_path, &pieces_hashes, piece_length, file_size)
            .await?;

        Ok(())
    }

    async fn watch_for_completed_pieces(
        &self,
        file: &mut File,
        completion_rx: &mut Receiver<CompletedPiece>,
        download_complete_rx: &mut Receiver<DownloadComplete>,
        num_pieces: usize,
        piece_length: usize,
    ) -> Result<()> {
        let mut downloaded_pieces = 0;

        loop {
            tokio::select! {
                Some(completed) = completion_rx.recv() => {
                    let offset = completed.piece_index as u64 * piece_length as u64;
                    file.seek(SeekFrom::Start(offset)).await?;
                    file.write_all(&completed.data).await?;
                    file.flush().await?;

                    downloaded_pieces += 1;
                    let percentage = (downloaded_pieces * 100) / num_pieces;

                    info!(
                        "Downloaded {}/{} pieces ({}%)",
                        downloaded_pieces, num_pieces, percentage
                    );
                }
                Some(_) = download_complete_rx.recv() => {
                    debug!("All pieces downloaded, exiting write loop");
                    break;
                }
            }
        }

        Ok(())
    }

    async fn verify_file(
        &self,
        file_path: &str,
        expected_hashes: &[[u8; 20]],
        piece_length: usize,
        file_size: usize,
    ) -> Result<()> {
        let mut file = File::open(file_path).await?;

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
        }

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
