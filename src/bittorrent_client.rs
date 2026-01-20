use anyhow::{Result, anyhow};
use log::{debug, info, warn};
use sha1::{Digest, Sha1};
use std::sync::Arc;
use std::time::Instant;
use std::{collections::BTreeMap, fs};

use crate::encoding::{BencodeTypes, Decoder, Encoder};
use crate::magnet_link::MagnetLink;
use crate::peer_manager::{PeerManager, PeerManagerConfig, PieceDownloadRequest};

const CLIENT_PEER_ID: &[u8; 20] = b"bittorrent-rust-0001";

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

    // Magnet link (for magnet-based downloads)
    magnet_link: Option<MagnetLink>,
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
            magnet_link: None,
        };

        torrent.load_torrent_file(input_file_path)?;
        Ok(torrent)
    }

    pub fn new_from_magnet(
        decoder: Decoder,
        encoder: Encoder,
        peer_manager: PeerManager,
        magnet: MagnetLink,
    ) -> Result<Self> {
        Ok(Self {
            decoder,
            encoder,
            metadata: None,
            peer_manager: Arc::new(peer_manager),
            start_time: None,
            magnet_link: Some(magnet),
        })
    }

    // Loads the torrent source file to extract metadata.
    fn load_torrent_file(&mut self, path: String) -> Result<()> {
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
        let client_peer_id = *CLIENT_PEER_ID;

        let filename = self.get_filename()?;
        let output_path = format!("{}/{}", output_directory_path, filename);
        let file_path = std::path::PathBuf::from(&output_path);

        let config = PeerManagerConfig {
            info_hash,
            client_peer_id,
            file_size,
            num_pieces,
            piece_length,
        };

        self.initialize_and_download(config, announce_url, file_path, &pieces_hashes)
            .await
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

    pub fn set_metadata_for_test(&mut self, metadata: BTreeMap<String, BencodeTypes>) {
        self.metadata = Some(metadata);
    }

    pub fn build_piece_requests(
        &self,
        num_pieces: usize,
        piece_length: usize,
        file_size: usize,
        pieces_hashes: &[[u8; 20]],
    ) -> Vec<PieceDownloadRequest> {
        (0..num_pieces)
            .map(|idx| {
                let piece_hash = pieces_hashes[idx];
                let offset = idx * piece_length;
                let remaining = file_size.saturating_sub(offset);
                let current_piece_length = std::cmp::min(piece_length, remaining);

                PieceDownloadRequest {
                    piece_index: idx as u32,
                    piece_length: current_piece_length,
                    expected_hash: piece_hash,
                }
            })
            .collect()
    }

    async fn download_and_verify_pieces(
        &mut self,
        num_pieces: usize,
        piece_length: usize,
        file_size: usize,
        pieces_hashes: &[[u8; 20]],
    ) -> Result<()> {
        let piece_requests =
            self.build_piece_requests(num_pieces, piece_length, file_size, pieces_hashes);
        self.peer_manager.request_pieces(piece_requests).await?;
        self.wait_for_download_completion(num_pieces).await?;

        info!("✅ All pieces downloaded and written to disk");

        info!("🔍 Verifying piece integrity...");
        let failed_pieces = self
            .peer_manager
            .verify_completed_pieces(pieces_hashes)
            .await?;

        if !failed_pieces.is_empty() {
            info!(
                "⚠️ Found {} pieces with hash mismatches, re-downloading...",
                failed_pieces.len()
            );

            let retry_requests: Vec<PieceDownloadRequest> = failed_pieces
                .iter()
                .map(|&idx| {
                    let piece_hash = pieces_hashes[idx as usize];
                    let offset = idx as usize * piece_length;
                    let remaining = file_size.saturating_sub(offset);
                    let current_piece_length = std::cmp::min(piece_length, remaining);

                    PieceDownloadRequest {
                        piece_index: idx,
                        piece_length: current_piece_length,
                        expected_hash: piece_hash,
                    }
                })
                .collect();

            self.peer_manager.request_pieces(retry_requests).await?;
            self.wait_for_download_completion(num_pieces).await?;

            let failed_pieces_second = self
                .peer_manager
                .verify_completed_pieces(pieces_hashes)
                .await?;

            if !failed_pieces_second.is_empty() {
                return Err(anyhow!(
                    "Verification failed after retry. {} pieces still corrupted",
                    failed_pieces_second.len()
                ));
            }

            info!("✅ All pieces verified successfully after retry");
        } else {
            info!("✅ All pieces verified successfully");
        }

        Ok(())
    }

    async fn initialize_and_download(
        &mut self,
        config: PeerManagerConfig,
        announce_url: Option<String>,
        file_path: std::path::PathBuf,
        pieces_hashes: &[[u8; 20]],
    ) -> Result<()> {
        let file_size = config.file_size;
        let piece_length = config.piece_length;
        let num_pieces = config.num_pieces;

        Arc::get_mut(&mut self.peer_manager)
            .ok_or_else(|| anyhow!("Failed to get mutable reference to PeerManager"))?
            .initialize(config, file_path, file_size as u64, piece_length)
            .await?;

        let _peer_manager_handle = self
            .peer_manager
            .clone()
            .start(announce_url.clone())
            .await?;

        self.start_time = Some(Instant::now());

        self.download_and_verify_pieces(num_pieces, piece_length, file_size, pieces_hashes)
            .await?;

        self.print_download_metrics(file_size);

        Ok(())
    }

    fn print_download_metrics(&self, file_size: usize) {
        if let Some(start_time) = self.start_time {
            let duration = start_time.elapsed();
            let duration_secs = duration.as_secs_f64();
            let file_size_mb = file_size as f64 / (1024.0 * 1024.0);
            let avg_speed_mbps = file_size_mb / duration_secs;

            info!("📊 Download completed in {:.2}s", duration_secs);
            info!("📦 File size: {:.2} MB", file_size_mb);
            info!("⚡ Average speed: {:.2} MB/s", avg_speed_mbps);
        }
    }

    async fn fetch_metadata_from_peers(
        &mut self,
        info_hash: [u8; 20],
        announce_url: Option<String>,
        client_peer_id: [u8; 20],
    ) -> Result<BTreeMap<String, BencodeTypes>> {
        use crate::peer_connection::PeerConnection;
        use crate::tcp_connector::DefaultTcpStreamFactory;
        use std::io::Write;

        let tcp_factory = Arc::new(DefaultTcpStreamFactory);
        let mut info_dict = None;
        let mut total_attempts = 0;

        loop {
            let peer_addrs = self
                .peer_manager
                .discover_peers(info_hash, announce_url.clone())
                .await?;

            if peer_addrs.is_empty() {
                total_attempts += 1;
                print!(
                    "\r\x1B[K🔍 Discovering peers... (attempt {})",
                    total_attempts
                );
                let _ = std::io::stdout().flush();
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                continue;
            }

            {
                use crate::peer::{Peer, PeerSource};
                let peers: Vec<Peer> = peer_addrs
                    .iter()
                    .map(|addr| Peer {
                        ip: addr.ip(),
                        port: addr.port(),
                        source: PeerSource::Dht,
                    })
                    .collect();

                self.peer_manager.add_available_peers(peers).await;
            }

            let batch_size = 5;
            let total_peers = peer_addrs.len();

            for batch_start in (0..total_peers).step_by(batch_size) {
                let batch_end = std::cmp::min(batch_start + batch_size, total_peers);
                let batch = &peer_addrs[batch_start..batch_end];

                print!(
                    "\r\x1B[K📥 Requesting metadata from peers {}-{} of {}...",
                    batch_start + 1,
                    batch_end,
                    total_peers
                );
                let _ = std::io::stdout().flush();

                let mut tasks = Vec::new();
                for peer_addr in batch {
                    let addr = *peer_addr;
                    let hash = info_hash;
                    let peer_id = client_peer_id;
                    let factory = tcp_factory.clone();

                    let task = tokio::spawn(async move {
                        PeerConnection::fetch_metadata(hash, addr, peer_id, factory).await
                    });
                    tasks.push((addr, task));
                }

                for (peer_addr, task) in tasks {
                    match task.await {
                        Ok(Ok(dict)) => {
                            print!("\r\x1B[K");
                            let _ = std::io::stdout().flush();
                            println!("✅ Metadata fetched successfully from {}", peer_addr);
                            info_dict = Some(dict);
                            break;
                        }
                        Ok(Err(e)) => {
                            debug!("Metadata fetch from {} failed: {}", peer_addr, e);
                        }
                        Err(e) => {
                            debug!("Metadata fetch task from {} panicked: {}", peer_addr, e);
                        }
                    }
                }

                if info_dict.is_some() {
                    break;
                }
            }

            if info_dict.is_some() {
                break;
            }

            total_attempts += 1;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }

        let info_dict = info_dict.ok_or_else(|| anyhow!("Failed to fetch metadata"))?;

        let mut full_metadata = BTreeMap::new();
        full_metadata.insert("info".to_string(), BencodeTypes::Dictionary(info_dict));

        if let Some(tracker) = &announce_url {
            full_metadata.insert(
                "announce".to_string(),
                BencodeTypes::String(tracker.clone()),
            );
        }

        Ok(full_metadata)
    }

    /// Downloads a torrent from a magnet link by fetching metadata from peers and then downloading the content.
    pub async fn download_from_magnet(
        &mut self,
        output_directory_path: String,
        filename_override: Option<String>,
    ) -> Result<()> {
        let magnet = self
            .magnet_link
            .clone()
            .ok_or_else(|| anyhow!("Not a magnet link download"))?;

        println!("🧲 Magnet link detected - fetching metadata from peers first...");
        println!("🔑 Info hash: {}", hex::encode(magnet.info_hash));

        let client_peer_id = *CLIENT_PEER_ID;
        let announce_url = magnet.trackers.first().cloned();

        if announce_url.is_none() {
            println!("🌐 Using DHT for peer discovery (no tracker URL in magnet link)");
        }

        let full_metadata = self
            .fetch_metadata_from_peers(magnet.info_hash, announce_url.clone(), client_peer_id)
            .await?;

        self.metadata = Some(full_metadata);

        let computed_hash = self.get_info_hash()?;
        if computed_hash != magnet.info_hash.to_vec() {
            return Err(anyhow!("Metadata info hash mismatch"));
        }

        let file_size = self.get_file_size()?;
        let piece_length = self.get_piece_length()?;
        let num_pieces = self.get_num_pieces()?;
        let pieces_hashes = self.get_pieces_hashes()?;

        let filename = filename_override
            .or_else(|| magnet.display_name.clone())
            .or_else(|| self.get_filename().ok())
            .ok_or_else(|| anyhow!("No filename available - use --name flag"))?;

        println!("\n📥 Downloading: {}", filename);
        self.print_file_metadata()?;

        let output_path = format!("{}/{}", output_directory_path, filename);
        let file_path = std::path::PathBuf::from(&output_path);

        let config = PeerManagerConfig {
            info_hash: magnet.info_hash,
            client_peer_id,
            file_size,
            num_pieces,
            piece_length,
        };

        self.initialize_and_download(config, announce_url, file_path, &pieces_hashes)
            .await
    }
}
