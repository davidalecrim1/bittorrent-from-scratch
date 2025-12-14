# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a BitTorrent client implementation in Rust built from scratch for educational purposes. It parses .torrent files, connects to tracker servers, discovers peers, and downloads files using the BitTorrent peer protocol over raw TCP connections.

## Development Commands

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
Always run the following after implementing a plan to fix formatting and linting errors:
```bash
make format && make lint
```

- **make format**: Runs `cargo fmt --all` to format all Rust files according to Rust conventions
- **make lint**: Runs `cargo clippy -- -D warnings` to check for code quality issues and common mistakes

### Logging
The application uses `env_logger` with debug level enabled by default in main.rs. Logs are verbose and helpful for debugging protocol interactions.

## Architecture

### Module Structure
- **main.rs**: Entry point, initializes components and CLI
- **cli.rs**: Command-line argument parsing using clap
- **encoding.rs**: Bencode encoder/decoder implementation
- **types.rs**: Core types including peer messages, connections, and protocol structs
- **file_manager.rs**: BitTorrent client (manages torrent metadata, pieces, and orchestration)
- **peer_manager.rs**: Handles tracker communication and peer lifecycle

### Key Architecture Patterns

**Async I/O with Tokio**: The entire project uses Tokio for async runtime. TCP connections follow the recommended pattern of dedicated read/write tasks with message passing via `mpsc` channels (see docs/ASYNC_IO.md for rationale).

**Peer Connection Model**: Each `PeerConnection` splits TCP stream into reader/writer tasks after handshake. Inbound messages flow through `inbound_tx` channel, outbound messages through `outbound_tx` channel. This avoids locking and provides natural backpressure.

**Message Codec**: Uses `tokio_util::codec` with custom `PeerMessageDecoder` and `PeerMessageEncoder` for framing peer protocol messages.

**Piece Management**: Pieces are tracked in `Arc<RwLock<Vec<Piece>>>` for shared state across async tasks.

### BitTorrent Protocol Implementation

**Bencode**: Custom implementation (not using serde_bencode) for parsing .torrent files. Handles strings, integers, lists, dictionaries, and raw bytes (for piece hashes).

**Peer Wire Protocol**: Implements handshake, bitfield, interested, unchoke, request, and piece messages. Block size is 16 KiB (DEFAULT_BLOCK_SIZE).

**Info Hash**: Computed as SHA1 of the bencoded "info" dictionary from the torrent file.

**Tracker Protocol**: Uses compact format (compact=1) to get peer list as binary data.

## Important Implementation Details

### Piece Hashes
The "pieces" field in .torrent files contains concatenated SHA1 hashes (20 bytes each). The encoder/decoder has special handling for this field to preserve raw bytes instead of attempting UTF-8 conversion.

### Error Handling
Current error handling relies on string matching (e.g., `e.to_string().contains("incomplete message")`). This is noted as technical debt in the code and should be refactored to use proper error types.

### Retry Logic
Handshake has exponential backoff retry logic (3 attempts with 200ms, 400ms, 800ms delays).

### Current State
The codebase is mid-refactoring. The BitTorrent client previously had a working single-piece download flow (now commented out in file_manager.rs). The new architecture separates concerns:
- FileManager decides which pieces to request
- PeerManager matches pieces to peers based on bitfield availability
- PeerConnection handles raw I/O with individual peers

The `download_file` method in file_manager.rs is currently empty - this is the integration point being worked on.

## Common Gotchas

### Ownership and Channels
When working with async tasks, remember that receivers (mpsc::Receiver) must be taken out of structs using `.take()` before moving into spawned tasks.

### Message Framing
Peer messages are length-prefixed (4 bytes big-endian) followed by message type (1 byte) and payload. The decoder must handle partial reads gracefully by returning `Ok(None)` when more data is needed.

### Bitfield Parsing
Bitfield messages use MSB-first bit ordering. Bit 0 of byte 0 represents piece 0. The implementation in types.rs handles this correctly.

### Edition
The project uses Rust edition 2024 (see Cargo.toml).
