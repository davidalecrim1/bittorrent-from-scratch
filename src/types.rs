use std::collections::BTreeMap;
use std::net::IpAddr;
use std::str::FromStr;
use anyhow::{anyhow, Result};
use std::fmt::Debug;

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
}

impl Debug for PeerHandshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let info_hash = hex::encode(self.info_hash);
        let peer_id = hex::encode(self.peer_id);
        write!(f, "PeerHandshake {{ info_hash: {:?}, peer_id: {:?} }}", info_hash, peer_id)
    }
}