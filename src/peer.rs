use std::net::IpAddr;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSource {
    Tracker,
    Dht,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Peer {
    pub ip: IpAddr,
    pub port: u16,
    pub source: PeerSource,
}

pub type PeerId = [u8; 20];
pub type PeerAddr = String;

impl Peer {
    pub fn new(ip: String, port: u16, source: PeerSource) -> Self {
        Self {
            ip: IpAddr::from_str(&ip).unwrap(),
            port,
            source,
        }
    }

    pub fn get_addr(&self) -> PeerAddr {
        if self.ip.is_ipv6() {
            format!("[{}]:{}", &self.ip, &self.port)
        } else {
            format!("{}:{}", &self.ip, &self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Peer, PeerSource};

    #[test]
    fn test_peer_ipv4_address_formatting() {
        let peer = Peer::new("192.168.1.100".to_string(), 6881, PeerSource::Tracker);
        assert_eq!(peer.get_addr(), "192.168.1.100:6881");
    }

    #[test]
    fn test_peer_ipv6_address_formatting() {
        let peer = Peer::new("2001:db8::1".to_string(), 6881, PeerSource::Tracker);
        assert_eq!(peer.get_addr(), "[2001:db8::1]:6881");

        let peer2 = Peer::new("fe80::1".to_string(), 8080, PeerSource::Tracker);
        assert_eq!(peer2.get_addr(), "[fe80::1]:8080");

        let peer3 = Peer::new("::1".to_string(), 6882, PeerSource::Tracker);
        assert_eq!(peer3.get_addr(), "[::1]:6882");
    }

    #[test]
    fn test_peer_ipv6_full_address_formatting() {
        let peer = Peer::new(
            "2001:0db8:0000:0000:0000:0000:0000:0001".to_string(),
            6881,
            PeerSource::Tracker,
        );
        assert_eq!(peer.get_addr(), "[2001:db8::1]:6881");
    }
}
