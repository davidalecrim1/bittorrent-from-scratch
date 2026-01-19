use anyhow::{Result, anyhow};

/// Represents a parsed magnet link for BitTorrent downloads
#[derive(Debug, Clone, PartialEq)]
pub struct MagnetLink {
    pub info_hash: [u8; 20],
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
}

impl MagnetLink {
    /// Parse a magnet URI string into a MagnetLink structure
    ///
    /// Format: magnet:?xt=urn:btih:<info_hash>&dn=<name>&tr=<tracker_url>
    ///
    /// Only hex-encoded info hashes (40 characters) are supported.
    /// Base32 encoding is not supported.
    pub fn parse(uri: &str) -> Result<Self> {
        if !uri.starts_with("magnet:?") {
            return Err(anyhow!("Invalid magnet URI: must start with 'magnet:?'"));
        }

        let query_string = &uri[8..]; // Skip "magnet:?"
        let params = Self::parse_query_params(query_string)?;

        let info_hash = Self::extract_info_hash(&params)?;
        let display_name = params
            .iter()
            .find(|(k, _)| k == "dn")
            .map(|(_, v)| v.to_string());
        let trackers = params
            .iter()
            .filter(|(k, _)| *k == "tr")
            .map(|(_, v)| v.to_string())
            .collect();

        Ok(Self {
            info_hash,
            display_name,
            trackers,
        })
    }

    fn parse_query_params(query: &str) -> Result<Vec<(String, String)>> {
        let mut params = Vec::new();

        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                let decoded_key = Self::url_decode(key)?;
                let decoded_value = Self::url_decode(value)?;
                params.push((decoded_key, decoded_value));
            }
        }

        Ok(params)
    }

    fn extract_info_hash(params: &[(String, String)]) -> Result<[u8; 20]> {
        let xt = params
            .iter()
            .find(|(k, _)| k == "xt")
            .ok_or_else(|| anyhow!("Missing required 'xt' parameter"))?;

        let info_hash_str =
            xt.1.strip_prefix("urn:btih:")
                .ok_or_else(|| anyhow!("Invalid 'xt' format: must start with 'urn:btih:'"))?;

        if info_hash_str.len() != 40 {
            return Err(anyhow!(
                "Invalid info hash length: expected 40 hex characters, got {}",
                info_hash_str.len()
            ));
        }

        let decoded = hex::decode(info_hash_str)
            .map_err(|_| anyhow!("Invalid info hash: must be valid hex"))?;

        if decoded.len() != 20 {
            return Err(anyhow!("Info hash must decode to 20 bytes"));
        }

        let mut hash = [0u8; 20];
        hash.copy_from_slice(&decoded);
        Ok(hash)
    }

    fn url_decode(s: &str) -> Result<String> {
        let mut result = String::new();
        let mut chars = s.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '%' {
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() != 2 {
                    return Err(anyhow!("Invalid URL encoding: incomplete escape sequence"));
                }
                let byte = u8::from_str_radix(&hex, 16)
                    .map_err(|_| anyhow!("Invalid URL encoding: bad hex in escape sequence"))?;
                result.push(byte as char);
            } else if ch == '+' {
                result.push(' ');
            } else {
                result.push(ch);
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_magnet_with_all_fields() {
        let uri = "magnet:?xt=urn:btih:1e873cd33f55737aaaefc0c282c428593c16e106&dn=archlinux-2026.01.01-x86_64.iso&tr=http://tracker.example.com:8080/announce";
        let magnet = MagnetLink::parse(uri).unwrap();

        assert_eq!(
            magnet.info_hash,
            hex::decode("1e873cd33f55737aaaefc0c282c428593c16e106")
                .unwrap()
                .as_slice()
        );
        assert_eq!(
            magnet.display_name,
            Some("archlinux-2026.01.01-x86_64.iso".to_string())
        );
        assert_eq!(
            magnet.trackers,
            vec!["http://tracker.example.com:8080/announce"]
        );
    }

    #[test]
    fn test_parse_minimal_magnet() {
        let uri = "magnet:?xt=urn:btih:1e873cd33f55737aaaefc0c282c428593c16e106";
        let magnet = MagnetLink::parse(uri).unwrap();

        assert_eq!(
            magnet.info_hash,
            hex::decode("1e873cd33f55737aaaefc0c282c428593c16e106")
                .unwrap()
                .as_slice()
        );
        assert_eq!(magnet.display_name, None);
        assert_eq!(magnet.trackers, Vec::<String>::new());
    }

    #[test]
    fn test_parse_multiple_trackers() {
        let uri = "magnet:?xt=urn:btih:1e873cd33f55737aaaefc0c282c428593c16e106&tr=http://tracker1.com&tr=http://tracker2.com";
        let magnet = MagnetLink::parse(uri).unwrap();

        assert_eq!(magnet.trackers.len(), 2);
        assert_eq!(magnet.trackers[0], "http://tracker1.com");
        assert_eq!(magnet.trackers[1], "http://tracker2.com");
    }

    #[test]
    fn test_url_decode_display_name() {
        let uri =
            "magnet:?xt=urn:btih:1e873cd33f55737aaaefc0c282c428593c16e106&dn=My%20File%20Name";
        let magnet = MagnetLink::parse(uri).unwrap();

        assert_eq!(magnet.display_name, Some("My File Name".to_string()));
    }

    #[test]
    fn test_invalid_prefix() {
        let uri = "http://example.com";
        assert!(MagnetLink::parse(uri).is_err());
    }

    #[test]
    fn test_missing_xt_parameter() {
        let uri = "magnet:?dn=test";
        assert!(MagnetLink::parse(uri).is_err());
    }

    #[test]
    fn test_invalid_xt_format() {
        let uri = "magnet:?xt=invalid";
        assert!(MagnetLink::parse(uri).is_err());
    }

    #[test]
    fn test_invalid_info_hash_length() {
        let uri = "magnet:?xt=urn:btih:1e873cd"; // Too short
        assert!(MagnetLink::parse(uri).is_err());
    }

    #[test]
    fn test_invalid_hex_in_info_hash() {
        let uri = "magnet:?xt=urn:btih:gggggggggggggggggggggggggggggggggggggggg"; // Invalid hex
        assert!(MagnetLink::parse(uri).is_err());
    }
}
