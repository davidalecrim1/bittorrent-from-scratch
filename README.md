# BitTorrent Client (Rust)

Educational BitTorrent client implementation in Rust, built from scratch to learn peer-to-peer networking, TCP async I/O with Tokio, and the BitTorrent protocol.

## Features

- Parse .torrent files using custom Bencode decoder
- Connect to tracker servers and discover peers
- Download files from multiple peers concurrently (up to 50 peers)
- Block pipelining: 5 concurrent block requests per peer for optimal throughput
- Verify downloaded pieces with SHA1 hashing
- Automatic retry logic for failed pieces with unlimited retries
- Graceful shutdown with proper piece requeuing on peer disconnect
- Real-time progress tracking in terminal

## Quick Start

```bash
cargo build
cargo run -- -i <path-to-torrent-file> -o <output-directory>
```

## Architecture Overview

- **BitTorrent** (`bittorrent_client.rs`): Orchestrates the download process, manages torrent metadata, writes completed pieces to disk, verifies file integrity with SHA1 hashes, and displays download progress with timing metrics
- **PeerManager** (`peer_manager.rs`): Manages a pool of connected peers (up to 50), assigns pieces to least-busy peers based on bitfield availability, handles unlimited retry logic for failed pieces, tracks completed piece indices to prevent re-queuing
- **PeerConnection** (`peer_connection.rs`): Handles block-level downloads using the BitTorrent peer protocol with pipelined requests (5 blocks ahead), manages individual peer TCP connections
- **Encoding** (`encoding.rs`): Custom Bencode encoder/decoder for parsing .torrent files (built from scratch, not using serde)

## Technical Details

### Protocol & Networking
- **Protocol**: BitTorrent peer wire protocol over TCP with handshake, bitfield, interested, unchoke, request, and piece messages
- **Async Runtime**: Tokio-based with split read/write tasks communicating via `mpsc` channels
- **Piece Validation**: SHA1 hashing of downloaded pieces (20 bytes per hash)
- **Block Size**: 16 KiB per network request (as per BitTorrent spec)

### Concurrency Architecture
- **Peer Pool**: Up to 50 concurrent peer connections for maximum parallelism
- **Block Pipelining**: 5 concurrent block requests per peer to keep TCP pipeline full
- **Piece Limiting**: 1 concurrent piece download per peer to ensure stability
- **Task Model**: Background tasks for peer connection, piece assignment, completion handling, failure retry, disconnect handling, tracker refresh, and progress reporting
- **Graceful Shutdown**: All background tasks listen on broadcast channel and terminate cleanly

### Download Strategy
- **Piece Assignment**: Eager assignment to all available peers for maximum parallelism
- **Peer Selection**: Least-busy algorithm based on bitfield availability and active download count
- **Retry Logic**: Unlimited retries for failed pieces, exponential backoff for handshakes
- **Disconnect Handling**: Failed pieces are automatically requeued when peers disconnect

### Performance Characteristics
- **Theoretical Max Concurrency**: 50 peers downloading in parallel
- **Network Efficiency**: Block pipelining (5 blocks ahead) keeps TCP pipeline full
- **Latency Optimization**: Pipelined requests reduce round-trip wait time
- **Backpressure**: Channel-based communication provides natural backpressure
- **Stability**: Single piece per peer prevents overwhelming individual connections

## Testing

The project maintains 70%+ test coverage on core modules with 97 total tests:
- **25 unit tests** in `src/` modules
- **72 integration tests** in `tests/` directory
  - 15 encoding tests
  - 28 message tests
  - 16 peer connection tests
  - 28 peer manager tests (including disconnect/requeue tests)

Run tests:
```bash
cargo test              # Run all tests
make coverage          # Generate coverage report with tarpaulin
```

## Limitations & Educational Notes

This is an **MVP implementation** focused on learning:

- Single-file torrents only (no multi-file support)
- No DHT (Distributed Hash Table) support
- No magnet link support
- No peer upload (seeding) capabilities
- Built from scratch for educational purposes

## Project Structure

```
src/
├── main.rs              # Entry point, CLI setup
├── cli.rs               # Command-line argument parsing
├── bittorrent_client.rs # Torrent metadata, file I/O, progress tracking
├── peer_manager.rs      # Peer pool orchestration
├── peer_connection.rs   # Individual peer TCP connection
├── messages.rs          # BitTorrent peer wire protocol messages
├── encoding.rs          # Bencode encoder/decoder
├── tracker_client.rs    # HTTP tracker communication
├── tcp_connector.rs     # TCP connection establishment
├── io.rs                # Message I/O with tokio_util::codec
└── types.rs             # Core types and data structures
```

## Code Quality

```bash
make format  # Format code with rustfmt
make lint    # Check for issues with clippy
```

## Logging

The application uses `env_logger` with debug level enabled by default. Logs are helpful for debugging protocol interactions and tracking download progress.

---

**Note**: This project prioritizes educational value and code clarity over performance. It's designed to understand how BitTorrent works at a low level by implementing the protocol from scratch.
