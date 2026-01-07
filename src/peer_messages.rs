use anyhow::{Result, anyhow};
use byteorder::{BigEndian, ReadBytesExt};
use log::debug;
use std::fmt::Debug;
use std::io::Cursor;

use crate::error::CodecError;

#[derive(Debug, Clone, PartialEq)]
pub enum PeerMessage {
    KeepAlive,
    Choke(ChokeMessage),
    Unchoke(UnchokeMessage),
    Interested(InterestedMessage),
    NotInterested(NotInterestedMessage),
    Bitfield(BitfieldMessage),
    Have(HaveMessage),
    Request(RequestMessage),
    Piece(PieceMessage),
    Cancel(CancelMessage),
}

impl PeerMessage {
    pub fn from_bytes(src: &[u8], num_pieces: usize) -> Result<(usize, Self)> {
        if src.len() < 4 {
            return Err(CodecError::MessageTooShort(src.len()).into());
        }

        let length = Self::get_length(src)?;

        // Handle Keep-Alive message (length = 0)
        if length == 0 {
            return Ok((4, Self::KeepAlive));
        }

        if src.len() < 5 {
            return Err(CodecError::MessageTooShort(src.len()).into());
        }

        let message_type = src[4];

        let total_size = 4 + length;
        if src.len() < total_size {
            return Err(CodecError::IncompleteMessage {
                needed: total_size,
                available: src.len(),
            }
            .into());
        }

        match message_type {
            0 => {
                let message = ChokeMessage::from_bytes(src)?;
                Ok((total_size, Self::Choke(message)))
            }
            1 => {
                let message = UnchokeMessage::from_bytes(src)?;
                Ok((total_size, Self::Unchoke(message)))
            }
            2 => {
                let message = InterestedMessage::from_bytes(src)?;
                Ok((total_size, Self::Interested(message)))
            }
            3 => {
                let message = NotInterestedMessage::from_bytes(src)?;
                Ok((total_size, Self::NotInterested(message)))
            }
            4 => {
                let message = HaveMessage::from_bytes(src)?;
                Ok((total_size, Self::Have(message)))
            }
            5 => {
                let payload = &src[5..];
                let message = BitfieldMessage::from_bytes(payload, num_pieces)?;
                Ok((total_size, Self::Bitfield(message)))
            }
            6 => {
                let message = RequestMessage::from_bytes(src)?;
                Ok((total_size, Self::Request(message)))
            }
            7 => {
                let message = PieceMessage::from_bytes(src)?;
                Ok((total_size, Self::Piece(message)))
            }
            8 => {
                let message = CancelMessage::from_bytes(src)?;
                Ok((total_size, Self::Cancel(message)))
            }
            _ => {
                debug!(
                    "Unknown message type {}, skipping {} bytes",
                    message_type, total_size
                );
                Ok((total_size, Self::KeepAlive))
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
            Self::KeepAlive => Ok(vec![0, 0, 0, 0]),
            Self::Choke(message) => Ok(message.to_bytes().to_vec()),
            Self::Unchoke(message) => Ok(message.to_bytes().to_vec()),
            Self::Interested(message) => Ok(message.to_bytes().to_vec()),
            Self::NotInterested(message) => Ok(message.to_bytes().to_vec()),
            Self::Have(message) => Ok(message.to_bytes().to_vec()),
            Self::Bitfield(message) => Ok(message.to_bytes()),
            Self::Request(message) => Ok(message.to_bytes().to_vec()),
            Self::Piece(message) => Ok(message.to_bytes()),
            Self::Cancel(message) => Ok(message.to_bytes().to_vec()),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct BitfieldMessage {
    pub bitfield: Vec<bool>,
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

    #[allow(dead_code)]
    pub fn has_piece(&self, index: usize) -> bool {
        self.bitfield.get(index).copied().unwrap_or(false)
    }

    #[allow(dead_code)]
    pub fn get_first_available_piece(&self) -> Option<usize> {
        self.bitfield
            .iter()
            .enumerate()
            .find_map(|(i, &has_piece)| if has_piece { Some(i) } else { None })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        // Convert bitfield back to bytes
        let num_bytes = self.bitfield.len().div_ceil(8);
        let mut bytes = vec![0u8; num_bytes];

        for (i, &has_piece) in self.bitfield.iter().enumerate() {
            if has_piece {
                let byte_index = i / 8;
                let bit_index = 7 - (i % 8);
                bytes[byte_index] |= 1 << bit_index;
            }
        }

        // Build full message: length (4) + type (1) + bitfield data
        let message_length = 1 + bytes.len();
        let mut result = Vec::with_capacity(4 + message_length);
        result.extend_from_slice(&(message_length as u32).to_be_bytes());
        result.push(5); // Message type for bitfield
        result.extend_from_slice(&bytes);
        result
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

#[derive(Debug, Clone, PartialEq)]
pub struct InterestedMessage {}

impl InterestedMessage {
    pub fn from_bytes(_bytes: &[u8]) -> Result<Self> {
        Ok(Self {})
    }

    pub fn to_bytes(&self) -> [u8; 5] {
        let mut bytes = [0u8; 5];
        bytes[0..4].copy_from_slice(&1u32.to_be_bytes());
        bytes[4] = 2;
        bytes
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChokeMessage {}

impl ChokeMessage {
    pub fn from_bytes(_bytes: &[u8]) -> Result<Self> {
        Ok(Self {})
    }

    pub fn to_bytes(&self) -> [u8; 5] {
        let mut bytes = [0u8; 5];
        bytes[0..4].copy_from_slice(&1u32.to_be_bytes());
        bytes[4] = 0; // Message type 0 for choke
        bytes
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnchokeMessage {}

impl UnchokeMessage {
    pub fn from_bytes(_bytes: &[u8]) -> Result<Self> {
        Ok(Self {})
    }

    pub fn to_bytes(&self) -> [u8; 5] {
        let mut bytes = [0u8; 5];
        bytes[0..4].copy_from_slice(&1u32.to_be_bytes());
        bytes[4] = 1; // Message type 1 for unchoke
        bytes
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RequestMessage {
    pub piece_index: u32,
    pub begin: u32,
    pub length: u32,
}

impl RequestMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 17 {
            return Err(anyhow!("Request message too short: {} bytes", bytes.len()));
        }

        let mut cursor = Cursor::new(&bytes[5..17]);
        let piece_index = cursor.read_u32::<BigEndian>()?;
        let begin = cursor.read_u32::<BigEndian>()?;
        let length = cursor.read_u32::<BigEndian>()?;

        Ok(Self {
            piece_index,
            begin,
            length,
        })
    }

    pub fn to_bytes(&self) -> [u8; 17] {
        let mut bytes = [0u8; 17];
        bytes[0..4].copy_from_slice(&13u32.to_be_bytes());
        bytes[4] = 6;
        bytes[5..9].copy_from_slice(&self.piece_index.to_be_bytes());
        bytes[9..13].copy_from_slice(&self.begin.to_be_bytes());
        bytes[13..17].copy_from_slice(&self.length.to_be_bytes());
        bytes
    }
}

#[derive(Clone, PartialEq)]
pub struct PieceMessage {
    pub piece_index: u32,
    pub begin: u32,
    pub block: Vec<u8>,
}

impl PieceMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 13 {
            return Err(anyhow!(
                "Piece message too short: {} bytes (need at least 13)",
                bytes.len()
            ));
        }
        let mut cursor = Cursor::new(bytes);
        let length = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)? as usize;
        let msg_id = ReadBytesExt::read_u8(&mut cursor)?;
        if msg_id != 7 {
            return Err(anyhow!("Invalid message ID for Piece message: {}", msg_id));
        }
        let piece_index = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;
        let begin = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;

        // Block size = message length - msg_id (1) - piece_index (4) - begin (4) = length - 9
        let block_size = length - 9;
        let mut block = vec![0u8; block_size];
        std::io::Read::read_exact(&mut cursor, &mut block)?;

        Ok(PieceMessage {
            piece_index,
            begin,
            block,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let message_length = 1 + 4 + 4 + self.block.len(); // msg_id + piece_index + begin + block
        let mut bytes = Vec::with_capacity(4 + message_length);
        bytes.extend_from_slice(&(message_length as u32).to_be_bytes());
        bytes.push(7); // Message type 7 for piece
        bytes.extend_from_slice(&self.piece_index.to_be_bytes());
        bytes.extend_from_slice(&self.begin.to_be_bytes());
        bytes.extend_from_slice(&self.block);
        bytes
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

#[derive(Debug, Clone, PartialEq)]
pub struct NotInterestedMessage {}

impl NotInterestedMessage {
    pub fn from_bytes(_bytes: &[u8]) -> Result<Self> {
        Ok(Self {})
    }

    pub fn to_bytes(&self) -> [u8; 5] {
        let mut bytes = [0u8; 5];
        bytes[0..4].copy_from_slice(&1u32.to_be_bytes());
        bytes[4] = 3; // Message type 3 for not interested
        bytes
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HaveMessage {
    pub piece_index: u32,
}

impl HaveMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 9 {
            return Err(anyhow!(
                "the provided data has less than 9 bytes, it has {}",
                bytes.len()
            ));
        }
        let mut cursor = Cursor::new(bytes);
        let _length = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;
        let msg_id = ReadBytesExt::read_u8(&mut cursor)?;
        if msg_id != 4 {
            return Err(anyhow!("the provided data is not a valid have message"));
        }
        let piece_index = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;

        Ok(HaveMessage { piece_index })
    }

    pub fn to_bytes(&self) -> [u8; 9] {
        let mut bytes = [0u8; 9];
        bytes[0..4].copy_from_slice(&5u32.to_be_bytes()); // Length = 5
        bytes[4] = 4; // Message type 4 for have
        bytes[5..9].copy_from_slice(&self.piece_index.to_be_bytes());
        bytes
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CancelMessage {
    #[allow(dead_code)]
    pub piece_index: u32,
    #[allow(dead_code)]
    pub begin: u32,
    #[allow(dead_code)]
    pub length: u32,
}

impl CancelMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 17 {
            return Err(anyhow!(
                "the provided data has less than 17 bytes, it has {}",
                bytes.len()
            ));
        }
        let mut cursor = Cursor::new(bytes);
        let _length = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;
        let msg_id = ReadBytesExt::read_u8(&mut cursor)?;
        if msg_id != 8 {
            return Err(anyhow!("the provided data is not a valid cancel message"));
        }
        let piece_index = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;
        let begin = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;
        let length = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;

        Ok(CancelMessage {
            piece_index,
            begin,
            length,
        })
    }

    pub fn to_bytes(&self) -> [u8; 17] {
        let mut bytes = [0u8; 17];
        bytes[0..4].copy_from_slice(&13u32.to_be_bytes());
        bytes[4] = 8; // Message type 8 for cancel
        bytes[5..9].copy_from_slice(&self.piece_index.to_be_bytes());
        bytes[9..13].copy_from_slice(&self.begin.to_be_bytes());
        bytes[13..17].copy_from_slice(&self.length.to_be_bytes());
        bytes
    }
}

/// Message during the handshake with a Peer.
/// Not a standard message like the ones during the client is
/// connected to the peer, but just for the handshake.
pub struct PeerHandshakeMessage {
    info_hash: [u8; 20], // NOT the hexadecimal string, but the actual bytes
    peer_id: [u8; 20],
    reserved: [u8; 8],
}

impl PeerHandshakeMessage {
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

impl Debug for PeerHandshakeMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let info_hash = hex::encode(self.info_hash);
        let peer_id = hex::encode(self.peer_id);
        write!(
            f,
            "PeerHandshakeMessage {{ info_hash: {:?}, peer_id: {:?} }}",
            info_hash, peer_id
        )
    }
}
