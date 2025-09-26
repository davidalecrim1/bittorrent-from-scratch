use anyhow::{Result, anyhow};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use bytes::Buf;
use futures_util::{SinkExt, StreamExt};
use log::debug;
use std::collections::BTreeMap;
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
        write!(
            f,
            "PeerHandshake {{ info_hash: {:?}, peer_id: {:?} }}",
            info_hash, peer_id
        )
    }
}

pub type PeerId = [u8; 20];

#[derive(Debug)]
pub struct PeerConnection {
    peer: Peer,
    peer_id: Option<PeerId>,
    stream: Option<TcpStream>,
}

impl PeerConnection {
    pub fn new(peer: Peer) -> Self {
        Self {
            peer,
            peer_id: None,
            stream: None,
        }
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
                Ok(response) => return Ok(response),
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

    // This should block if everthing is working correctly
    // for reading and writing messages to the peer over the channels.
    pub async fn run(
        &mut self,
        num_pieces: usize,
        notify: mpsc::Sender<PeerMessage>,
        mut rx: mpsc::Receiver<PeerMessage>,
    ) -> Result<()> {
        if self.stream.is_none() {
            return Err(anyhow!("the stream is not present"));
        }

        let stream = self.stream.take().unwrap();
        let (reader, writer) = stream.into_split();
        let mut reader = FramedRead::new(reader, PeerMessageDecoder { num_pieces });
        let mut writer = FramedWrite::new(writer, PeerMessageEncoder { num_pieces });

        // Spawn write loop
        tokio::task::spawn(async move {
            while let Some(message) = rx.recv().await {
                let _ = writer.send(message).await;
            }
        });

        // Spawn read loop
        while let Some(item) = reader.next().await {
            match item {
                Ok(msg) => {
                    let _ = notify.send(msg).await;
                }
                Err(e) => {
                    debug!("[run] error receiving message: {}", &e);
                    return Err(anyhow!("error receiving message: {}", e));
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
                debug!("[from_bytes] received unchoke message: {:?}", &message);
                Ok((total_size, Self::Unchoke(message)))
            }
            5 => {
                // Pass the payload data (excluding length and message type bytes)
                let payload = &src[5..];
                let message = BitfieldMessage::from_bytes(payload, num_pieces)?;
                debug!("[from_bytes] received bitfield message: {:?}", &message);
                Ok((total_size, Self::Bitfield(message)))
            }
            _ => {
                debug!(
                    "[from_bytes] received invalid message: {:?} with length {}",
                    &message_type, length
                );
                return Err(anyhow!("the message type is invalid: {:?}", &message_type));
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
            _ => {
                return Err(anyhow!(
                    "the message being encoded is not supported: {:?}",
                    &self
                ));
            }
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

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
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

#[derive(Debug)]
pub struct PieceMessage {
    pub piece_index: u32,
    pub begin: u32,
    pub block: Vec<u8>,
}

impl PieceMessage {
    pub fn from_bytes(&self, bytes: &[u8]) -> Result<Self> {
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
        std::io::Read::read_exact(&mut cursor, &mut block)?;

        Ok(PieceMessage {
            piece_index,
            begin,
            block,
        })
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
