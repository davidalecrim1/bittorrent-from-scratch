#![allow(dead_code)]

use anyhow::{Result, anyhow};
use byteorder::{BigEndian, ReadBytesExt};
use bytes::Buf;
use futures_util::{SinkExt, StreamExt};
use log::debug;
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Debug;
use std::io::Cursor;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};

// Piece download constants
#[allow(dead_code)]
const DEFAULT_BLOCK_SIZE: usize = 16 * 1024; // 16 KiB per BitTorrent spec
#[allow(dead_code)]
const BLOCK_PIPELINE_SIZE: usize = 5; // Request 5 blocks ahead

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PieceDownloadRequest {
    pub piece_index: u32,
    pub piece_length: usize,
    pub expected_hash: [u8; 20], // SHA1 hash for verification
}

#[derive(Debug, Clone)]
pub struct PeerManagerConfig {
    pub info_hash: [u8; 20],
    pub client_peer_id: [u8; 20],
    pub file_size: usize,
    pub num_pieces: usize,
    pub max_peers: usize,
}

#[derive(Debug)]
pub struct ConnectedPeer {
    pub peer: Peer,
    pub peer_id: [u8; 20],
    pub download_request_tx: mpsc::Sender<PieceDownloadRequest>,
    pub bitfield: Vec<bool>,
    pub active_downloads: HashSet<u32>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CompletedPiece {
    pub piece_index: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct FailedPiece {
    pub piece_index: u32,
    pub reason: String, // "hash_mismatch" or "peer_disconnected"
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum BencodeTypes {
    String(String),
    Integer(isize),
    List(Vec<BencodeTypes>),
    Dictionary(BTreeMap<String, BencodeTypes>),
    Raw(Vec<u8>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Peer {
    pub ip: IpAddr,
    pub port: u16,
}

impl Peer {
    pub fn new(ip: String, port: u16) -> Self {
        Self {
            ip: IpAddr::from_str(&ip).unwrap(),
            port,
        }
    }

    pub fn get_addr(&self) -> String {
        format!("{}:{}", &self.ip, &self.port)
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

        bytes
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

    pub fn get_peer_id(&self) -> [u8; 20] {
        self.peer_id
    }
}

impl Debug for PeerHandshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let info_hash = hex::encode(self.info_hash);
        let peer_id = hex::encode(self.peer_id);
        write!(
            f,
            "PeerHandshake {{ info_hash: {:?}, peer_id: {:?} }}",
            info_hash, peer_id
        )
    }
}

pub type PeerId = [u8; 20];

#[derive(Debug)]
struct DownloadState {
    piece_index: u32,
    piece_length: usize,
    block_size: usize,
    total_blocks: usize,
    expected_hash: [u8; 20],
    received_blocks: HashMap<u32, Vec<u8>>, // key: offset, value: data
    pending_blocks: HashSet<u32>,           // offsets we've requested
}

impl DownloadState {
    fn new(
        piece_index: u32,
        piece_length: usize,
        block_size: usize,
        expected_hash: [u8; 20],
    ) -> Self {
        let total_blocks = piece_length.div_ceil(block_size);

        Self {
            piece_index,
            piece_length,
            block_size,
            total_blocks,
            expected_hash,
            received_blocks: HashMap::new(),
            pending_blocks: HashSet::new(),
        }
    }

    fn add_block(&mut self, begin: u32, data: Vec<u8>) -> Result<()> {
        if self.received_blocks.contains_key(&begin) {
            debug!("[DownloadState] Duplicate block at offset {}", begin);
            return Ok(()); // Ignore duplicate
        }

        self.received_blocks.insert(begin, data);
        self.pending_blocks.remove(&begin);
        Ok(())
    }

    fn is_complete(&self) -> bool {
        self.received_blocks.len() == self.total_blocks
    }

    fn assemble_piece(&self) -> Result<Vec<u8>> {
        if !self.is_complete() {
            return Err(anyhow!("Cannot assemble incomplete piece"));
        }

        let mut piece_data = Vec::with_capacity(self.piece_length);
        let mut offset = 0u32;

        for _ in 0..self.total_blocks {
            let block = self
                .received_blocks
                .get(&offset)
                .ok_or_else(|| anyhow!("Missing block at offset {}", offset))?;

            piece_data.extend_from_slice(block);
            offset += self.block_size as u32;
        }

        // Trim to exact piece length (last block may be padded)
        piece_data.truncate(self.piece_length);
        Ok(piece_data)
    }

    fn verify_hash(&self, data: &[u8]) -> Result<bool> {
        let mut hasher = Sha1::new();
        hasher.update(data);
        let computed_hash = hasher.finalize();
        Ok(computed_hash.as_slice() == self.expected_hash)
    }

    fn get_next_block_to_request(&mut self) -> Option<(u32, u32)> {
        let mut offset = 0u32;

        for _ in 0..self.total_blocks {
            if !self.received_blocks.contains_key(&offset) && !self.pending_blocks.contains(&offset)
            {
                let remaining = self.piece_length.saturating_sub(offset as usize);
                let length = std::cmp::min(self.block_size, remaining) as u32;

                self.pending_blocks.insert(offset);
                return Some((offset, length));
            }
            offset += self.block_size as u32;
        }

        None
    }
}

#[derive(Debug)]
pub struct PeerConnection {
    peer: Peer,
    peer_id: Option<PeerId>,

    // Stream is only present after handshake
    stream: Option<TcpStream>,

    // Outbound messages the actor will write to the wire
    outbound_tx: mpsc::Sender<PeerMessage>,
    outbound_rx: Option<mpsc::Receiver<PeerMessage>>,

    // Inbound messages read from the wire and sent out
    inbound_tx: mpsc::Sender<PeerMessage>,
    inbound_rx: Option<mpsc::Receiver<PeerMessage>>,

    // Piece download channels
    download_request_tx: mpsc::Sender<PieceDownloadRequest>,
    download_request_rx: Option<mpsc::Receiver<PieceDownloadRequest>>,
    piece_completion_tx: mpsc::Sender<CompletedPiece>,
    piece_failure_tx: mpsc::Sender<FailedPiece>,

    // Peer Status
    is_choking: bool,
    is_interested: bool,

    // Peer Information
    bitfield: Vec<bool>,

    // Download state
    current_download: Option<DownloadState>,
}

// Creates a connection with the Peer provided and wraps it within
// It will rely on the inbound_tx and outbound_rx channels to communicate with the peer connection.
impl PeerConnection {
    pub fn new(
        peer: Peer,
        piece_completion_tx: mpsc::Sender<CompletedPiece>,
        piece_failure_tx: mpsc::Sender<FailedPiece>,
    ) -> (Self, mpsc::Sender<PieceDownloadRequest>) {
        let (outbound_tx, outbound_rx) = mpsc::channel(10);
        let (inbound_tx, inbound_rx) = mpsc::channel(10);
        let (download_request_tx, download_request_rx) = mpsc::channel(10);

        let peer_conn = Self {
            peer,
            peer_id: None,
            stream: None,
            outbound_tx,
            outbound_rx: Some(outbound_rx),
            inbound_tx,
            inbound_rx: Some(inbound_rx),
            download_request_tx: download_request_tx.clone(),
            download_request_rx: Some(download_request_rx),
            piece_completion_tx,
            piece_failure_tx,
            is_choking: false,
            is_interested: false,
            bitfield: Vec::new(),
            current_download: None,
        };

        (peer_conn, download_request_tx)
    }

    pub fn get_peer_id(&self) -> Option<[u8; 20]> {
        self.peer_id
    }

    // Opens the connection and performs the handshake.
    // This is the kick off to run the messages loop.
    pub async fn handshake(
        &mut self,
        client_peer_id: [u8; 20],
        info_hash: [u8; 20],
    ) -> Result<PeerHandshake> {
        let handshake = PeerHandshake::new(info_hash, client_peer_id);
        let bytes = handshake.to_bytes();
        let max_retries = 3;

        let mut last_error = None;

        for attempt in 1..=max_retries {
            match self.try_handshake(&bytes).await {
                Ok(response) => {
                    return Ok(response);
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        let delay = Duration::from_millis(200 * 2_u64.pow(attempt - 1)); // 200ms, 400ms, 800ms ...
                        debug!(
                            "[handshake] attempt {} failed, retrying in {}ms",
                            attempt,
                            delay.as_millis()
                        );
                        sleep(delay).await;
                    }
                }
            }
        }

        Err(anyhow!(
            "Failed to handshake with peer {} after {} attempts: {}",
            self.peer.get_addr(),
            max_retries,
            last_error.unwrap()
        ))
    }

    async fn try_handshake(&mut self, handshake_bytes: &[u8]) -> Result<PeerHandshake> {
        let mut stream = TcpStream::connect(self.peer.get_addr()).await?;
        stream.write_all(handshake_bytes).await?;

        let mut buffer = vec![0u8; 68];
        let n = stream.read(&mut buffer).await?;

        let response = PeerHandshake::from_bytes(&buffer[..n])?;
        self.peer_id = Some(response.get_peer_id());
        self.stream = Some(stream);

        Ok(response)
    }

    pub async fn start(mut self, num_pieces: usize) -> Result<()> {
        let stream = self
            .stream
            .take()
            .ok_or_else(|| anyhow!("handshake not done"))?;
        let (reader, writer) = stream.into_split();

        let decoder = PeerMessageDecoder { num_pieces };
        let encoder = PeerMessageEncoder { num_pieces };

        let mut reader = FramedRead::new(reader, decoder);
        let mut writer = FramedWrite::new(writer, encoder);

        // Take the receivers out of self
        let mut outbound_rx = self.outbound_rx.take().unwrap();
        let inbound_tx = self.inbound_tx.clone();

        // Spawn writer task
        tokio::spawn(async move {
            while let Some(msg) = outbound_rx.recv().await {
                let _ = writer.send(msg).await;
            }
        });

        // Spawn reader task
        tokio::spawn(async move {
            while let Some(result) = reader.next().await {
                match result {
                    Ok(msg) => {
                        let _ = inbound_tx.send(msg).await;
                    }
                    Err(err) => {
                        // TODO: How to let the peer manager now this is disconnected and should be dropped?
                        debug!("read error: {}", err);
                        break;
                    }
                }
            }
        });

        self.send_interested_message().await;
        self.handle_incoming_messages();

        Ok(())
    }

    fn handle_incoming_messages(mut self) {
        let mut inbound_rx = self.inbound_rx.take().unwrap();
        let mut download_request_rx = self.download_request_rx.take().unwrap();

        tokio::task::spawn(async move {
            loop {
                tokio::select! {
                    Some(msg) = inbound_rx.recv() => {
                        if let Err(e) = self.handle_peer_message(msg).await {
                            debug!("[handle_incoming_messages] Error handling peer message: {}", e);
                            break;
                        }
                    }
                    Some(request) = download_request_rx.recv() => {
                        let piece_index = request.piece_index;
                        if let Err(e) = self.start_piece_download(request).await {
                            debug!("[handle_incoming_messages] Error starting download: {}", e);
                            // Send failure to manager but continue
                            let _ = self.piece_failure_tx.send(FailedPiece {
                                piece_index,
                                reason: e.to_string(),
                            }).await;
                        }
                    }
                    else => break,
                }
            }
            debug!("[handle_incoming_messages] Message handler exiting");
        });
    }

    async fn handle_peer_message(&mut self, msg: PeerMessage) -> Result<()> {
        match msg {
            PeerMessage::Bitfield(bitfield) => {
                self.bitfield = bitfield.bitfield;
                debug!(
                    "[handle_peer_message] Received bitfield with {} pieces",
                    self.bitfield.len()
                );
            }

            PeerMessage::Unchoke(_) => {
                self.is_choking = false;
                debug!("[handle_peer_message] Peer unchoked us");

                // If we have a pending download, start requesting blocks
                if self.current_download.is_some() {
                    self.handle_piece_download().await?;
                }
            }

            PeerMessage::Interested(_) => {
                self.is_interested = true;
            }

            PeerMessage::Request(_request) => {
                // We don't support uploading yet
                debug!("[handle_peer_message] Ignoring request message (upload not implemented)");
            }

            PeerMessage::Piece(piece) => {
                self.handle_piece_block(piece).await?;
            }
        }

        Ok(())
    }

    async fn handle_piece_block(&mut self, piece_msg: PieceMessage) -> Result<()> {
        let download_state = match self.current_download.as_mut() {
            Some(state) => state,
            None => {
                debug!(
                    "[handle_piece_block] Received unexpected piece block for piece {}, no active download",
                    piece_msg.piece_index
                );
                return Ok(());
            }
        };

        // Verify this block belongs to our current download
        if piece_msg.piece_index != download_state.piece_index {
            debug!(
                "[handle_piece_block] Received block for piece {}, but downloading piece {}",
                piece_msg.piece_index, download_state.piece_index
            );
            return Ok(());
        }

        // Add block to our state
        download_state.add_block(piece_msg.begin, piece_msg.block)?;

        debug!(
            "[handle_piece_block] Received block: piece={}, begin={}, blocks={}/{}",
            piece_msg.piece_index,
            piece_msg.begin,
            download_state.received_blocks.len(),
            download_state.total_blocks
        );

        if download_state.is_complete() {
            self.finish_piece_download().await?;
        } else {
            self.handle_piece_download().await?;
        }

        Ok(())
    }

    async fn start_piece_download(&mut self, request: PieceDownloadRequest) -> Result<()> {
        if !self.has_piece(request.piece_index as usize) {
            return Err(anyhow!("Peer does not have piece {}", request.piece_index));
        }

        if !self.can_download_pieces() {
            return Err(anyhow!(
                "Peer not ready for download (choking={}, interested={}, bitfield={})",
                self.is_choking,
                self.is_interested,
                self.bitfield.is_empty()
            ));
        }

        if self.current_download.is_some() {
            return Err(anyhow!(
                "Already downloading a piece, cannot start new download"
            ));
        }

        debug!(
            "[start_piece_download] Starting download of piece {}, length={}",
            request.piece_index, request.piece_length
        );

        self.current_download = Some(DownloadState::new(
            request.piece_index,
            request.piece_length,
            DEFAULT_BLOCK_SIZE,
            request.expected_hash,
        ));

        self.handle_piece_download().await?;

        Ok(())
    }

    async fn finish_piece_download(&mut self) -> Result<()> {
        let download_state = self
            .current_download
            .take()
            .ok_or_else(|| anyhow!("No active download to finish"))?;

        debug!(
            "[finish_piece_download] Piece {} complete, assembling data",
            download_state.piece_index
        );

        let piece_data = download_state.assemble_piece()?;

        match download_state.verify_hash(&piece_data)? {
            true => {
                let completed = CompletedPiece {
                    piece_index: download_state.piece_index,
                    data: piece_data,
                };

                self.piece_completion_tx
                    .send(completed)
                    .await
                    .map_err(|_| anyhow!("Failed to send completed piece to manager"))?;

                debug!(
                    "[finish_piece_download] Sent completed piece {} to manager",
                    download_state.piece_index
                );
            }
            false => {
                let failed = FailedPiece {
                    piece_index: download_state.piece_index,
                    reason: "hash_mismatch".to_string(),
                };

                self.piece_failure_tx
                    .send(failed)
                    .await
                    .map_err(|_| anyhow!("Failed to send failed piece to manager"))?;

                debug!(
                    "[finish_piece_download] Hash mismatch for piece {}, notified manager",
                    download_state.piece_index
                );
            }
        }

        Ok(())
    }

    fn is_ready_to_download_piece(&self) -> bool {
        debug!(
            "[PeerConnection] peer_id: {:?}, is_interested: {:?}, bitfield_len: {:?}, is_choking: {:?}",
            self.peer_id,
            self.is_interested,
            self.bitfield.len(),
            self.is_choking
        );

        self.is_interested && !self.bitfield.is_empty() && !self.is_choking
    }

    fn fetch_piece(&self) -> Result<usize> {
        for (idx, has_piece) in self.bitfield.iter().enumerate() {
            if *has_piece {
                return Ok(idx);
            }
        }

        Err(anyhow!(
            "No pieces available in peer connection for peer {:?}",
            self.peer_id
        ))
    }

    pub fn has_piece(&self, piece_index: usize) -> bool {
        self.bitfield.get(piece_index).copied().unwrap_or(false)
    }

    pub fn can_download_pieces(&self) -> bool {
        !self.is_choking && self.is_interested && !self.bitfield.is_empty()
    }

    pub fn get_bitfield(&self) -> &[bool] {
        &self.bitfield
    }

    async fn send_interested_message(&self) {
        let outbound_tx = self.outbound_tx.clone();

        match outbound_tx
            .send(PeerMessage::Interested(InterestedMessage {}))
            .await
        {
            Ok(_) => {}
            Err(err) => {
                debug!(
                    "Failed to send interested message to peer {:?}: {}",
                    self.peer_id, err
                );
            }
        }
    }

    async fn handle_piece_download(&mut self) -> Result<()> {
        // Check if we have an active download
        let download_state = match self.current_download.as_mut() {
            Some(state) => state,
            None => {
                debug!("[handle_piece_download] No active download");
                return Ok(());
            }
        };

        // Request blocks in pipeline fashion
        let mut requests_sent = 0;
        while requests_sent < BLOCK_PIPELINE_SIZE {
            match download_state.get_next_block_to_request() {
                Some((begin, length)) => {
                    let request = RequestMessage {
                        piece_index: download_state.piece_index,
                        begin,
                        length,
                    };

                    debug!(
                        "[handle_piece_download] Requesting block: piece={}, begin={}, length={}",
                        download_state.piece_index, begin, length
                    );

                    self.outbound_tx.send(PeerMessage::Request(request)).await?;

                    requests_sent += 1;
                }
                None => {
                    // No more blocks to request
                    break;
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum PeerMessage {
    Unchoke(UnchokeMessage),
    Interested(InterestedMessage),
    Bitfield(BitfieldMessage),
    Request(RequestMessage),
    Piece(PieceMessage),
}

pub struct PeerMessageDecoder {
    num_pieces: usize,
}

impl Decoder for PeerMessageDecoder {
    type Item = PeerMessage;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match PeerMessage::from_bytes(src.as_ref(), self.num_pieces) {
            Ok((n, message)) => {
                src.advance(n);
                Ok(Some(message))
            }
            Err(e) => {
                // TODO: See if there is a better way to handle this more like Golang sentinel errors.
                if e.to_string().contains("incomplete message")
                    || e.to_string().contains("the message has less than 5 bytes")
                {
                    Ok(None) // Need more data - this is normal!
                } else {
                    Err(e)
                }
            }
        }
    }
}

pub struct PeerMessageEncoder {
    num_pieces: usize,
}

impl Encoder<PeerMessage> for PeerMessageEncoder {
    type Error = anyhow::Error;

    fn encode(&mut self, item: PeerMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes = item.to_bytes();
        if let Err(e) = bytes {
            debug!("[encode] error converting message to bytes: {}", &e);
            return Err(anyhow!("error converting message to bytes: {}", e));
        }

        dst.extend_from_slice(&bytes.unwrap());
        Ok(())
    }
}

impl PeerMessage {
    pub fn from_bytes(src: &[u8], num_pieces: usize) -> Result<(usize, Self)> {
        if src.len() < 5 {
            return Err(anyhow!(
                "the message has less than 5 bytes, it has {}",
                src.len()
            ));
        }

        let length = Self::get_length(src)?;
        debug!("[from_bytes] the message length: {}", length);
        let message_type = src[4];

        // Total message size = 4 bytes (length field) + length bytes (payload including message type)
        let total_size = 4 + length;
        if src.len() < total_size {
            return Err(anyhow!(
                "incomplete message, need {} bytes, got {}",
                total_size,
                src.len()
            ));
        }

        match message_type {
            1 => {
                let message = UnchokeMessage::from_bytes(src)?;
                debug!("[from_bytes] received `unchoke` message: {:?}", &message);
                Ok((total_size, Self::Unchoke(message)))
            }
            5 => {
                // Pass the payload data (excluding length and message type bytes)
                let payload = &src[5..];
                let message = BitfieldMessage::from_bytes(payload, num_pieces)?;
                debug!("[from_bytes] received `bitfield` message: {:?}", &message);
                Ok((total_size, Self::Bitfield(message)))
            }
            7 => {
                let message = PieceMessage::from_bytes(src)?;
                debug!("[from_bytes] received `piece` message`: {:?}", &message);
                Ok((total_size, Self::Piece(message)))
            }
            _ => {
                debug!(
                    "[from_bytes] received invalid message: {:?} with length {}",
                    &message_type, length
                );
                Err(anyhow!("the message type is invalid: {:?}", &message_type))
            }
        }
    }

    fn get_length(bytes: &[u8]) -> Result<usize> {
        if bytes.len() < 4 {
            return Err(anyhow!(
                "the provided data has less than 4 bytes, it has {}",
                bytes.len()
            ));
        }

        let length = &bytes[0..4];
        let length = u32::from_be_bytes(length.try_into()?);
        Ok(length as usize)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        match self {
            Self::Interested(message) => Ok(message.to_bytes().to_vec()),
            Self::Request(message) => Ok(message.to_bytes().to_vec()),
            _ => Err(anyhow!(
                "the message being encoded is not supported: {:?}",
                &self
            )),
        }
    }
}

// This message keeps track of which pieces the peer has.
// This can be seen by the pieces of the file using the index of the vector.
// e.g. bitfield[0] = true means that the peer has the first piece.
// The pieces hashes and total number of pieces come from the torrent file.
pub struct BitfieldMessage {
    bitfield: Vec<bool>,
}

impl BitfieldMessage {
    pub fn from_bytes(bytes: &[u8], num_pieces: usize) -> Result<Self> {
        let mut bitfield = Vec::with_capacity(num_pieces);
        for i in 0..num_pieces {
            let byte_index = i / 8;
            let bit_index = 7 - (i % 8);
            let has_piece = if byte_index < bytes.len() {
                (bytes[byte_index] >> bit_index) & 1 == 1
            } else {
                false
            };
            bitfield.push(has_piece);
        }

        Ok(Self { bitfield })
    }

    pub fn has_piece(&self, index: usize) -> bool {
        self.bitfield.get(index).copied().unwrap_or(false)
    }

    pub fn get_first_available_piece(&self) -> Option<usize> {
        self.bitfield
            .iter()
            .enumerate()
            .find_map(|(i, &has_piece)| if has_piece { Some(i) } else { None })
    }
}

impl Debug for BitfieldMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let amount_of_bits = self.bitfield.len();
        write!(
            f,
            "BitfieldMessage {{ amount_of_bits: {} }}",
            amount_of_bits
        )
    }
}

#[derive(Debug)]
pub struct InterestedMessage {}
impl InterestedMessage {
    pub fn to_bytes(&self) -> [u8; 5] {
        let mut bytes = [0u8; 5];
        bytes[0..4].copy_from_slice(&1u32.to_be_bytes()); // length
        bytes[4] = 2; // message type; 2 = interested
        bytes
    }
}

#[derive(Debug)]
pub struct UnchokeMessage {}
impl UnchokeMessage {
    pub fn to_bytes(&self) -> [u8; 5] {
        let mut bytes = [0u8; 5];
        bytes[0..4].copy_from_slice(&1u32.to_be_bytes()); // length
        bytes[4] = 1; // message type; 1 = unchoke
        bytes
    }

    pub fn from_bytes(_bytes: &[u8]) -> Result<Self> {
        // Just ignore the bytes for now since it's just an empty message
        Ok(Self {})
    }
}

#[derive(Debug)]
pub struct RequestMessage {
    pub piece_index: u32,
    pub begin: u32,
    pub length: u32,
}

impl RequestMessage {
    pub fn to_bytes(&self) -> [u8; 17] {
        let mut bytes = [0u8; 17];
        // length prefix (13 bytes payload)
        bytes[0..4].copy_from_slice(&13u32.to_be_bytes());
        bytes[4] = 6; // message ID = 6 (request)
        bytes[5..9].copy_from_slice(&self.piece_index.to_be_bytes());
        bytes[9..13].copy_from_slice(&self.begin.to_be_bytes());
        bytes[13..17].copy_from_slice(&self.length.to_be_bytes());
        bytes
    }
}

pub struct PieceMessage {
    pub piece_index: u32,
    pub begin: u32,
    pub block: Vec<u8>,
}

impl PieceMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 9 {
            return Err(anyhow!(
                "the provided data has less than 9 bytes, it has {}",
                bytes.len()
            ));
        }
        let mut cursor = Cursor::new(bytes);
        let _length = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?; // can verify length
        let msg_id = ReadBytesExt::read_u8(&mut cursor)?;
        if msg_id != 7 {
            // 7 = Piece message
            return Err(anyhow!("the provided data is not a valid piece message"));
        }
        let piece_index = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;
        let begin = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;
        let mut block = vec![0u8; bytes.len() - 9];
        if let Err(e) = std::io::Read::read_exact(&mut cursor, &mut block) {
            debug!("[from_bytes] error reading block: {:?}", e);
            std::io::Read::read_to_end(&mut cursor, &mut block)?;
        }

        Ok(PieceMessage {
            piece_index,
            begin,
            block,
        })
    }
}

impl Debug for PieceMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PieceMessage {{ piece_index: {}, begin: {} }}",
            self.piece_index, self.begin
        )
    }
}

// pub struct Piece {
//     index: u32,
//     piece_length: usize,
//     blocks: Vec<Option<Vec<u8>>>,
// }

// impl Piece {
//     pub fn new(index: u32, piece_length: usize, blocks_size: usize) -> Self {
//         Self {
//             index,
//             piece_length,
//             blocks: vec![None; blocks_size],
//         }
//     }

//     pub fn add_block(&mut self, message: PieceMessage) -> Result<()> {
//         if message.piece_index != self.index {
//             return Err(anyhow!(
//                 "the piece index does not match the piece index of the piece"
//             ));
//         }

//         self.blocks[message.begin as usize] = Some(message.block);
//         Ok(())
//     }

//     pub fn is_complete(&self) -> bool {
//         self.blocks.iter().all(|block| block.is_some())
//     }

//     pub fn update_idx(&mut self, index: u32) {
//         self.index = index
//     }
// }

#[derive(Debug)]
pub struct Piece {
    index: usize,
    status: PieceStatus,
}

impl Piece {
    pub fn new(index: usize, status: PieceStatus) -> Self {
        Self { index, status }
    }

    pub fn set_status(&mut self, status: PieceStatus) {
        self.status = status;
    }

    pub fn get_status(&self) -> &PieceStatus {
        &self.status
    }
}

#[derive(Debug)]
pub enum PieceStatus {
    Pending,
    Downloading,
    Completed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitfield_message_from_bytes_single_byte() {
        let bytes = vec![0b10101010];
        let num_pieces = 8;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        assert!(bitfield.has_piece(0));
        assert!(!bitfield.has_piece(1));
        assert!(bitfield.has_piece(2));
        assert!(!bitfield.has_piece(3));
        assert!(bitfield.has_piece(4));
        assert!(!bitfield.has_piece(5));
        assert!(bitfield.has_piece(6));
        assert!(!bitfield.has_piece(7));
    }

    #[test]
    fn test_bitfield_message_from_bytes_multiple_bytes() {
        let bytes = vec![0b11110000, 0b00001111];
        let num_pieces = 16;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        for i in 0..4 {
            assert!(bitfield.has_piece(i), "Piece {} should be available", i);
        }
        for i in 4..8 {
            assert!(
                !bitfield.has_piece(i),
                "Piece {} should not be available",
                i
            );
        }
        for i in 8..12 {
            assert!(
                !bitfield.has_piece(i),
                "Piece {} should not be available",
                i
            );
        }
        for i in 12..16 {
            assert!(bitfield.has_piece(i), "Piece {} should be available", i);
        }
    }

    #[test]
    fn test_bitfield_message_fewer_pieces_than_bits() {
        let bytes = vec![0b11111111];
        let num_pieces = 5;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        for i in 0..5 {
            assert!(bitfield.has_piece(i), "Piece {} should be available", i);
        }

        assert!(!bitfield.has_piece(5));
        assert!(!bitfield.has_piece(6));
        assert!(!bitfield.has_piece(7));
    }

    #[test]
    fn test_bitfield_message_all_pieces_available() {
        let bytes = vec![0b11111111, 0b11111111];
        let num_pieces = 16;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        for i in 0..16 {
            assert!(bitfield.has_piece(i), "Piece {} should be available", i);
        }
    }

    #[test]
    fn test_bitfield_message_no_pieces_available() {
        let bytes = vec![0b00000000, 0b00000000];
        let num_pieces = 16;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        for i in 0..16 {
            assert!(
                !bitfield.has_piece(i),
                "Piece {} should not be available",
                i
            );
        }
    }

    #[test]
    fn test_bitfield_message_has_piece_out_of_bounds() {
        let bytes = vec![0b10101010];
        let num_pieces = 4;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        assert!(!bitfield.has_piece(100));
        assert!(!bitfield.has_piece(8));
    }

    #[test]
    fn test_bitfield_message_empty_bytes() {
        let bytes = vec![];
        let num_pieces = 0;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        assert!(!bitfield.has_piece(0));
    }

    #[test]
    fn test_bitfield_message_msb_ordering() {
        let bytes = vec![0b10000000];
        let num_pieces = 8;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        assert!(bitfield.has_piece(0));
        for i in 1..8 {
            assert!(
                !bitfield.has_piece(i),
                "Piece {} should not be available",
                i
            );
        }
    }

    #[test]
    fn test_bitfield_message_realistic_scenario() {
        let bytes = vec![0b11010100, 0b10110000, 0b00000001];
        let num_pieces = 20;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        let expected = [
            true, true, false, true, false, true, false, false, true, false, true, true, false,
            false, false, false, false, false, false, true,
        ];

        for (i, &expected_value) in expected.iter().enumerate() {
            assert_eq!(
                bitfield.has_piece(i),
                expected_value,
                "Piece {} should be {}",
                i,
                if expected_value {
                    "available"
                } else {
                    "not available"
                }
            );
        }
    }

    #[test]
    fn test_bitfield_message_insufficient_bytes() {
        let bytes = vec![0b11110000];
        let num_pieces = 12;

        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();

        for i in 0..4 {
            assert!(bitfield.has_piece(i), "Piece {} should be available", i);
        }
        for i in 4..12 {
            assert!(
                !bitfield.has_piece(i),
                "Piece {} should not be available",
                i
            );
        }
    }
}
