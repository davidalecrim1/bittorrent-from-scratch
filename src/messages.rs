use anyhow::{Result, anyhow};
use byteorder::{BigEndian, ReadBytesExt};
use log::debug;
use std::fmt::Debug;
use std::io::Cursor;

use crate::error::CodecError;

#[derive(Debug)]
pub enum PeerMessage {
    Unchoke(UnchokeMessage),
    Interested(InterestedMessage),
    Bitfield(BitfieldMessage),
    Request(RequestMessage),
    Piece(PieceMessage),
}

impl PeerMessage {
    pub fn from_bytes(src: &[u8], num_pieces: usize) -> Result<(usize, Self)> {
        if src.len() < 5 {
            return Err(CodecError::MessageTooShort(src.len()).into());
        }

        let length = Self::get_length(src)?;
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
            1 => {
                let message = UnchokeMessage::from_bytes(src)?;
                Ok((total_size, Self::Unchoke(message)))
            }
            5 => {
                let payload = &src[5..];
                let message = BitfieldMessage::from_bytes(payload, num_pieces)?;
                Ok((total_size, Self::Bitfield(message)))
            }
            7 => {
                let message = PieceMessage::from_bytes(src)?;
                Ok((total_size, Self::Piece(message)))
            }
            _ => {
                debug!("Unknown message type {}", message_type);
                Err(CodecError::UnknownMessageType(message_type).into())
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
        bytes[0..4].copy_from_slice(&1u32.to_be_bytes());
        bytes[4] = 2;
        bytes
    }
}

#[derive(Debug)]
pub struct UnchokeMessage {}

impl UnchokeMessage {
    pub fn from_bytes(_bytes: &[u8]) -> Result<Self> {
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
        bytes[0..4].copy_from_slice(&13u32.to_be_bytes());
        bytes[4] = 6;
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
        let _length = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;
        let msg_id = ReadBytesExt::read_u8(&mut cursor)?;
        if msg_id != 7 {
            return Err(anyhow!("the provided data is not a valid piece message"));
        }
        let piece_index = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;
        let begin = ReadBytesExt::read_u32::<BigEndian>(&mut cursor)?;
        let mut block = vec![0u8; bytes.len() - 9];
        if std::io::Read::read_exact(&mut cursor, &mut block).is_err() {
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
