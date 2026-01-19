# Magnet Link Support

This document describes the implementation of magnet link support in the BitTorrent client, including metadata fetching via the Extension Protocol (BEP 9).

## Overview

Magnet links allow downloading torrents using only an info hash, without requiring a `.torrent` file. The client fetches the torrent metadata from peers using the BitTorrent Extension Protocol.

## Supported Magnet Link Format

```
magnet:?xt=urn:btih:<info_hash>&dn=<display_name>&tr=<tracker_url>
```

**Parameters:**
- `xt` (required): Info hash in hex format
- `dn` (optional): Display name for the file
- `tr` (optional): Tracker URL (can appear multiple times)

**Example:**
```
magnet:?xt=urn:btih:1e873cd33f55737aaaefc0c282c428593c16e106&dn=archlinux-2026.01.01-x86_64.iso
```

## Implementation Details

### Architecture

The magnet link download process involves three main phases:

1. **Peer Discovery** - Find peers that have the content
2. **Metadata Fetch** - Download torrent metadata from peers
3. **Content Download** - Standard piece-based download

### Key Components

#### 1. Magnet Link Parsing (`src/magnet_link.rs`)

Parses magnet URIs and extracts:
- Info hash (20-byte SHA1)
- Display name (optional)
- Tracker URLs (optional)

```rust
pub struct MagnetLink {
    pub info_hash: [u8; 20],
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
}
```

#### 2. Peer Discovery (`src/peer_manager.rs::discover_peers()`)

Discovers peers without establishing full connections:
- Queries HTTP tracker (if provided in magnet link)
- Queries DHT for peer discovery
- Returns list of peer socket addresses

#### 3. Metadata Fetching (`src/peer_connection.rs::fetch_metadata()`)

Implements BEP 9 (Extension for Peers to Send Metadata Files):

**Protocol Flow:**
1. TCP handshake with `extension` bit set in reserved bytes
2. Extension handshake to negotiate `ut_metadata` support
3. Request metadata pieces sequentially
4. Assemble complete metadata from all pieces
5. Verify and parse as bencode dictionary

**Multi-Piece Support:**
- Metadata is transferred in 16 KiB pieces (per BEP 9)
- Client calculates `num_pieces = ceil(total_size / 16384)`
- Requests all pieces sequentially from the same peer
- Assembles pieces in order before parsing

#### 4. Extension Protocol (`src/peer_messages.rs`)

**Extension Handshake:**
```rust
{
    "m": {
        "ut_metadata": 1  // Our extension ID
    }
}
```

**Metadata Request Message:**
```rust
{
    "msg_type": 0,      // 0 = request
    "piece": <index>    // Which piece to request
}
```

**Metadata Response Message:**
```rust
{
    "msg_type": 1,      // 1 = data
    "piece": <index>,
    "total_size": <size>
}
<raw metadata bytes>
```

### Extension ID Handling

Extension IDs are **bidirectional and asymmetric**:
- When **we** advertise `"ut_metadata": 1`, peers use ID `1` when sending TO us
- When **peer** advertises `"ut_metadata": 2`, we use ID `2` when sending TO peer

This is per BEP 10 specification.

### Protocol Message Handling

After the extension handshake, peers typically send standard protocol messages before responding to metadata requests:
- `Bitfield` - Peer's available pieces
- `Unchoke` - Peer allows downloading
- `Interested` - Other protocol messages

The implementation loops through messages and skips non-Extension messages until receiving the metadata response.

### Bencode Decoder Fix

The bencode decoder was fixed to handle dictionaries followed by additional data:
- Original: Checked both first AND last byte for dictionary detection
- Fixed: Only checks if data starts with 'd'
- Reason: Metadata responses have format `d<outer_dict>ed<info_dict>` where outer dict is followed by info dict data

## References

- **BEP 9**: Extension for Peers to Send Metadata Files
  - http://www.bittorrent.org/beps/bep_0009.html

- **BEP 10**: Extension Protocol
  - http://www.bittorrent.org/beps/bep_0010.html

- **BEP 5**: DHT Protocol (for trackerless downloads)
  - http://www.bittorrent.org/beps/bep_0005.html
