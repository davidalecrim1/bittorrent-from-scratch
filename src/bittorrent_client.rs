use anyhow::{Result, anyhow};
use log::{debug, info, warn};
use sha1::{Digest, Sha1};
use std::sync::Arc;
use std::time::Instant;
use std::{collections::BTreeMap, fs};

use crate::encoding::Decoder;
use crate::encoding::Encoder;
use crate::peer_manager::PeerManager;
use crate::types::{BencodeTypes, PeerManagerConfig, PieceDownloadRequest};

pub struct BitTorrent {
    // Encoder and Decoder are used to encode and decode
    // the data from a torrent file and some specific messages with tracker.
    decoder: Decoder,
    encoder: Encoder,

    // Store the metadata of the torrent file,
    // after parsing the file.
    metadata: Option<BTreeMap<String, BencodeTypes>>,

    // Peer manager to handle the communication with the peers.
    peer_manager: Arc<PeerManager>,

    // Download timing metrics
    start_time: Option<Instant>,
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
            peer_manager: Arc::new(peer_manager),
            start_time: None,
        };

        torrent.load_file(input_file_path)?;
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
        // Record download start time
        self.start_time = Some(Instant::now());

        let info_hash_bytes = self.get_info_hash()?;
        let info_hash: [u8; 20] = info_hash_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Invalid info hash length"))?;

        let file_size = self.get_file_size()?;
        let announce_url = self.get_announce_url()?;

        if announce_url.is_none() {
            warn!("No announce URL found in torrent file - tracker will not be used");
        }

        let piece_length = self.get_piece_length()?;
        let num_pieces = self.get_num_pieces()?;
        let pieces_hashes = self.get_pieces_hashes()?;
        let client_peer_id = *b"postman-000000000001"; // TODO: Improve this to something clearer.

        let config = PeerManagerConfig {
            info_hash,
            client_peer_id,
            file_size,
            num_pieces,
            piece_length,
        };

        let filename = self.get_filename()?;
        let output_path = format!("{}/{}", output_directory_path, filename);
        let file_path = std::path::PathBuf::from(&output_path);

        // Initialize PeerManager with file metadata - it will create and manage the file
        Arc::get_mut(&mut self.peer_manager)
            .ok_or_else(|| anyhow!("Failed to get mutable reference to PeerManager"))?
            .initialize(config, file_path, file_size as u64, piece_length)
            .await?;

        let _peer_manager_handle = self
            .peer_manager
            .clone()
            .start(announce_url.clone())
            .await?;

        let piece_requests: Vec<PieceDownloadRequest> = (0..num_pieces)
            .map(|idx| {
                let piece_hash = pieces_hashes[idx];

                // Calculate the correct piece length for this piece
                let offset = idx * piece_length;
                let remaining = file_size.saturating_sub(offset);
                let current_piece_length = std::cmp::min(piece_length, remaining);

                PieceDownloadRequest {
                    piece_index: idx as u32,
                    piece_length: current_piece_length,
                    expected_hash: piece_hash,
                }
            })
            .collect();

        self.peer_manager.request_pieces(piece_requests).await?;

        // Wait for all pieces to complete by checking download status
        self.wait_for_download_completion(num_pieces).await?;

        info!("✅ All pieces downloaded and written to disk");

        // Verify all pieces by reading from disk and checking hashes
        info!("🔍 Verifying piece integrity...");
        let failed_pieces = self
            .peer_manager
            .verify_completed_pieces(&pieces_hashes)
            .await?;

        if !failed_pieces.is_empty() {
            info!(
                "⚠️ Found {} pieces with hash mismatches, re-downloading...",
                failed_pieces.len()
            );
            // Pieces are already re-queued, wait for completion again
            self.wait_for_download_completion(num_pieces).await?;

            // Verify again after re-download
            let failed_pieces_retry = self
                .peer_manager
                .verify_completed_pieces(&pieces_hashes)
                .await?;

            if !failed_pieces_retry.is_empty() {
                return Err(anyhow!(
                    "Verification failed after retry. {} pieces still corrupted",
                    failed_pieces_retry.len()
                ));
            }

            info!("✅ All pieces verified successfully after retry");
        } else {
            info!("✅ All pieces verified successfully");
        }

        // Display download metrics
        if let Some(start_time) = self.start_time {
            let duration = start_time.elapsed();
            let duration_secs = duration.as_secs_f64();
            let file_size_mb = file_size as f64 / (1024.0 * 1024.0);
            let avg_speed_mbps = file_size_mb / duration_secs;

            info!("📊 Download completed in {:.2}s", duration_secs);
            info!("📦 File size: {:.2} MB", file_size_mb);
            info!("⚡ Average speed: {:.2} MB/s", avg_speed_mbps);
        }

        Ok(())
    }

    /// Wait for all pieces to be downloaded and written to disk.
    /// Polls the piece manager's completion status periodically.
    async fn wait_for_download_completion(&self, num_pieces: usize) -> Result<()> {
        loop {
            let snapshot = self.peer_manager.get_piece_snapshot().await;

            if snapshot.completed_count >= num_pieces {
                debug!("All {} pieces completed", num_pieces);
                break;
            }

            // Sleep briefly to avoid busy-waiting
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
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

    fn get_announce_url(&self) -> Result<Option<String>> {
        let metadata = self
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow!("Metadata not loaded"))?;

        match metadata.get("announce") {
            Some(BencodeTypes::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(anyhow!("announce field is not a string")),
            None => Ok(None),
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
