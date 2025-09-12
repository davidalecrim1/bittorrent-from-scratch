use std::{collections::BTreeMap};
use std::net::IpAddr;
use std::str::FromStr;
use anyhow::{anyhow, Result};
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::fmt::Debug;
use std::time::Duration;
use tokio::time::sleep;
use tokio::sync::mpsc;

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
        Self { ip: IpAddr::from_str(&ip).unwrap(), port }
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

    pub fn get_peer_id(&self) -> [u8; 20] { 
        self.peer_id
    }
}

impl Debug for PeerHandshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let info_hash = hex::encode(self.info_hash);
        let peer_id = hex::encode(self.peer_id);
        write!(f, "PeerHandshake {{ info_hash: {:?}, peer_id: {:?} }}", info_hash, peer_id)
    }
}

#[derive(Debug)]
pub struct PeerConnection {
    peer: Peer,
    peer_id: Option<[u8; 20]>,
    stream: Option<TcpStream>,
}

impl PeerConnection {
    pub fn new(peer: Peer) -> Self { 
        Self { peer, peer_id: None, stream: None }
    }

    pub fn get_peer_id(&self) -> Option<[u8; 20]> {
        self.peer_id
    }

    // Opens the connection and performs the handshake.
    // This is the kick off to run the messages loop.
    pub async fn handshake(&mut self, client_peer_id: [u8; 20], info_hash: [u8; 20]) -> Result<PeerHandshake> {
        let handshake = PeerHandshake::new(info_hash, client_peer_id);
        let bytes = handshake.to_bytes();
        let max_retries = 3;

        let mut last_error = None;

        for attempt in 1..=max_retries {
            match self.try_handshake(&bytes).await {
                Ok(response) => return Ok(response),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        let delay = Duration::from_millis(200 * 2_u64.pow(attempt - 1)); // 200ms, 400ms, 800ms ...
                        dbg!("[handshake] attempt {} failed, retrying in {}ms", 
                               attempt, delay.as_millis());
                        sleep(delay).await;
                    }
                }
            }
        }

        Err(anyhow!("Failed to handshake with peer {} after {} attempts: {}", 
                   self.peer.get_addr(), max_retries, last_error.unwrap()))
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

    pub async fn start_exchanging_messages(&mut self, num_pieces: usize) -> Result<(mpsc::Sender<PeerMessage>, mpsc::Receiver<PeerMessage>)> {
        let (message_tx, message_rx) = mpsc::channel::<PeerMessage>(100);
        let (command_tx, mut command_rx) = mpsc::channel::<PeerMessage>(10);

        let mut buffer = vec![0u8; 1024*10];
        let mut stream = self.stream.take().ok_or_else(|| anyhow!("No stream available"))?;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // Handle commands that should be sent in the TCP connection.
                    command = command_rx.recv() => {
                        if let Some(command) = command {
                            if let Err(e) = Self::handle_command(command, &mut stream).await {
                                dbg!("[peer_loop] error handling command: {}", e);
                                break;
                            }
                        }
                    }
                    // Handle incoming messages in the TCP connection.
                    read_message = stream.read(&mut buffer) => {
                        match read_message {
                            Ok(0) => {
                                dbg!("[peer_loop] connection closed by peer");
                                break;
                            }
                            Ok(n) => {
                                let msg = PeerMessage::from_bytes(&buffer[..n], num_pieces);
                                if let Err(e) = msg {
                                    dbg!("[peer_loop] error parsing message: {}", e);
                                    break;
                                }
                                message_tx.send(msg.unwrap()).await.unwrap();
                            }
                            Err(e) => {
                                dbg!("[peer_loop] error reading message: {}", e);
                                break;
                            }
                        }
                    }
                }
            }
            dbg!("[peer_loop] the loop with the peer has finished");
        });

        Ok((command_tx, message_rx))
    }

    async fn handle_command(command: PeerMessage, stream: &mut TcpStream) -> Result<()>{
        match command {
            PeerMessage::Interested(message) => {
                dbg!("[handle_command] received interested message: {:?}", &message);
                stream.write_all(&InterestedMessage::to_bytes()).await.unwrap();
                Ok(())
            }
            _ => {
                dbg!("[handle_command] received invalid command: {:?}", &command);
                return Err(anyhow!("the provided data is not a valid command"));
            }
        }
    }
}

#[derive(Debug)]
pub enum PeerMessage {
    Unchoke(UnchokeMessage),
    Interested(InterestedMessage),
    Bitfield(BitfieldMessage), 
}

impl PeerMessage {
    pub fn from_bytes(bytes: &[u8], num_pieces: usize) -> Result<Self> {
        if bytes.len() < 5 {
            return Err(anyhow!("the provided data is not a valid message"));
        }
       
        let length = Self::get_length(bytes)?;
        dbg!("[from_bytes] the message length: {}", length);
        let message_type = bytes[4];

        match message_type {
            1 => {
                let message = UnchokeMessage::from_bytes(bytes)?;
                dbg!("[from_bytes] received unchoke message: {:?}", &message);
                Ok(Self::Unchoke(message))
            }
            5 => { 
                let message = BitfieldMessage::from_bytes(bytes, num_pieces)?;
                dbg!("[from_bytes] received bitfield message: {:?}", &message);
                Ok(Self::Bitfield(message))
            } 
            _ => {
                dbg!("[from_bytes] received invalid message: {:?}", &message_type);
                return Err(anyhow!("the provided data is not a valid message"));
            }
        }
    }

    fn get_length(bytes: &[u8]) -> Result<usize> {
        let length = &bytes[0..4];
        let length = u32::from_be_bytes(length.try_into()?);
        Ok(length as usize)
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
}

impl Debug for BitfieldMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bits = self.bitfield.iter()
            .map(|&b| if b { "1" } else { "0" })
            .collect::<String>();

        write!(f, "BitfieldMessage {{ bits: \"{}\" }}", bits)
    }
}

#[derive(Debug)]
pub struct InterestedMessage {}
impl InterestedMessage {
    pub fn to_bytes() -> [u8; 5] {
        let mut bytes = [0u8; 5];
        bytes[0..4].copy_from_slice(&1u32.to_be_bytes()); // length
        bytes[4] = 2; // message type; 2 = interested
        bytes
    }
}

#[derive(Debug)]
pub struct UnchokeMessage {}
impl UnchokeMessage {
    pub fn to_bytes() -> [u8; 5] {
        let mut bytes = [0u8; 5];
        bytes[0..4].copy_from_slice(&1u32.to_be_bytes()); // length
        bytes[4] = 1; // message type; 1 = unchoke
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        // Just ignore the bytes for now since it's just an empty message
        Ok(Self {})
    }
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
            assert!(!bitfield.has_piece(i), "Piece {} should not be available", i);
        }
        for i in 8..12 {
            assert!(!bitfield.has_piece(i), "Piece {} should not be available", i);
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
            assert!(!bitfield.has_piece(i), "Piece {} should not be available", i);
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
            assert!(!bitfield.has_piece(i), "Piece {} should not be available", i);
        }
    }

    #[test]
    fn test_bitfield_message_realistic_scenario() {
        let bytes = vec![0b11010100, 0b10110000, 0b00000001];
        let num_pieces = 20;
        
        let bitfield = BitfieldMessage::from_bytes(&bytes, num_pieces).unwrap();
        
        let expected = [
            true, true, false, true, false, true, false, false,
            true, false, true, true, false, false, false, false,
            false, false, false, true
        ];
        
        for (i, &expected_value) in expected.iter().enumerate() {
            assert_eq!(
                bitfield.has_piece(i), 
                expected_value, 
                "Piece {} should be {}", 
                i, 
                if expected_value { "available" } else { "not available" }
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
            assert!(!bitfield.has_piece(i), "Piece {} should not be available", i);
        }
    }
}

