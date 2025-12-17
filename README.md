# BitTorrent Client (Rust)

Educational BitTorrent client implementation in Rust, built from scratch to learn peer-to-peer networking, TCP async I/O with Tokio, and the BitTorrent protocol.

## Features

- Parse .torrent files using custom Bencode decoder
- Connect to tracker servers and discover peers
- Download files from multiple peers concurrently (up to 10 peers)
- Verify downloaded pieces with SHA1 hashing
- Automatic retry logic for failed pieces
- Real-time progress tracking in terminal

## Quick Start

### Build and Run
```bash
cargo build
cargo run -- -i <path-to-torrent-file> -o <output-directory>
```

### Testing
```bash
cargo test
```

### Code Quality
```bash
make format  # Format code with rustfmt
make lint    # Check for issues with clippy
```

## Architecture Overview

- **FileManager** (`file_manager.rs`): Manages torrent metadata, writes completed pieces to disk, verifies file integrity with SHA1, and displays download progress
- **PeerManager** (`peer_manager.rs`): Orchestrates peer lifecycle with connection pool (max 10 peers), assigns pieces to optimal peers based on availability, handles retry logic for failed pieces
- **PeerConnection** (`peer_connection.rs`): Handles raw TCP I/O with individual peers, downloads piece blocks using the peer wire protocol, reports completion/failure via channels
- **Encoding** (`encoding.rs`): Custom Bencode encoder/decoder for parsing .torrent files (built from scratch, not using serde)

## Technical Details

- **Protocol**: BitTorrent peer wire protocol over TCP with handshake, bitfield, interested, unchoke, request, and piece messages
- **Async Runtime**: Tokio-based with split read/write tasks communicating via `mpsc` channels
- **Piece Validation**: SHA1 hashing of downloaded pieces (20 bytes per hash)
- **Block Size**: 16 KiB per network request (as per BitTorrent spec)
- **Download Strategy**: Eager piece assignment to all available peers for maximum parallelism
- **Error Handling**: Exponential backoff retry logic for handshakes and tracker connections

## Limitations & Educational Notes

This is an **MVP implementation** focused on learning:

- Single-file torrents only (no multi-file support)
- No DHT (Distributed Hash Table) support
- No magnet link support
- No peer upload (seeding) capabilities
- Error handling uses string matching (technical debt noted in code)
- Built from scratch for educational purposes

## Project Structure

```
src/
├── main.rs              # Entry point, CLI setup
├── cli.rs               # Command-line argument parsing
├── file_manager.rs      # Torrent metadata and file I/O
├── peer_manager.rs      # Peer pool orchestration
├── peer_connection.rs   # Individual peer TCP connection
├── messages.rs          # BitTorrent peer wire protocol messages
├── encoding.rs          # Bencode encoder/decoder + message codec
├── download_state.rs    # Piece download state tracking
├── error.rs             # Custom error types
└── types.rs             # Core types and data structures
```

## Development Commands

- `cargo build` - Build the project
- `cargo run -- -i <torrent> -o <output>` - Run with arguments
- `cargo test` - Run tests
- `make format` - Format code with `cargo fmt --all`
- `make lint` - Lint code with `cargo clippy -- -D warnings`

## Logging

The application uses `env_logger` with debug level enabled by default. Logs are helpful for debugging protocol interactions and tracking download progress.

---

**Note**: This project prioritizes educational value and code clarity over performance. It's designed to understand how BitTorrent works at a low level by implementing the protocol from scratch.
