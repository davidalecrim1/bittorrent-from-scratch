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

**Test Organization:**
- **Unit tests**: Located in `src/**` files using `#[cfg(test)]` modules (26 tests including DHT)
- **Integration tests**: Located in `tests/` directory for black-box testing of public APIs (87 tests)
  - `tests/encoding_tests.rs`: Bencode encoding/decoding tests (15 tests)
  - `tests/messages_tests.rs`: Peer protocol message serialization tests (28 tests)
  - `tests/peer_connection_tests.rs`: PeerConnection protocol logic tests (24 tests)
  - `tests/peer_manager_tests.rs`: PeerManager orchestration tests (28 tests)
  - `tests/dht_tests.rs`: DHT protocol and integration tests (15 tests)
  - `tests/bittorrent_client_tests.rs`: File I/O and piece verification tests (8 tests)
  - `tests/helpers/fakes.rs`: Test doubles (FakeMessageIO, MockTrackerClient, MockDhtClient, MockPeerConnectionFactory)

**Total: 113 tests achieving 70%+ coverage on core modules**

### Coverage
```bash
make coverage
```

Runs `cargo tarpaulin --all-targets --engine llvm --out Stdout` to measure test coverage:
- **encoding.rs**: 82.04% coverage (169/206 lines)
- **messages.rs**: 72.69% coverage (173/238 lines)
- **peer_manager.rs**: 77.25% coverage (309/400 lines)
- **peer_connection.rs**: 71.55% coverage (171/239 lines)
- **Overall**: 56.70% coverage (829/1462 lines)

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
- **messages.rs**: BitTorrent peer protocol message types and serialization
- **types.rs**: Core types including PeerConnection, PeerManagerHandle, and protocol structs
- **peer_connection.rs**: Individual peer connection lifecycle and piece download logic
- **peer_manager.rs**: Peer pool orchestration, piece assignment, retry logic, and PeerConnectionFactory trait
- **bittorrent_client.rs**: Torrent metadata, file I/O, piece verification, and download progress tracking
- **tracker_client.rs**: HTTP tracker communication and TrackerClient trait
- **tcp_connector.rs**: TCP connection establishment and TcpStreamFactory trait
- **io.rs**: Message I/O implementation using tokio_util::codec and MessageIO trait
- **dht/**: DHT (Distributed Hash Table) implementation for trackerless peer discovery
  - **types.rs**: DHT protocol types (NodeId, KrpcMessage, Query/Response enums)
  - **routing_table.rs**: Kademlia routing table with 160 k-buckets
  - **socket_factory.rs**: UDP socket abstraction (UdpSocketFactory trait)
  - **message_io.rs**: KRPC message serialization over UDP (DhtMessageIO trait)
  - **query_manager.rs**: In-flight query tracking with timeout/retry
  - **manager.rs**: DHT orchestration (DhtManager, background tasks)
  - **client.rs**: DhtClient trait and production implementation

### Key Architecture Patterns

**Async I/O with Tokio**: The entire project uses Tokio for async runtime. TCP connections follow the recommended pattern of dedicated read/write tasks with message passing via `mpsc` channels (see docs/ASYNC_IO.md for rationale).

**Dependency Injection for Testability**: Core I/O operations are abstracted behind traits to enable testing without network/disk access. Traits are co-located with their implementations:
- **TcpStreamFactory** (in `tcp_connector.rs`): Abstracts TCP connection establishment (real vs in-memory)
- **MessageIO** (in `io.rs`): Abstracts peer message I/O (TCP streams vs channels)
- **TrackerClient** (in `tracker_client.rs`): Abstracts HTTP tracker communication (reqwest vs mock responses)
- **DhtClient** (in `dht/client.rs`): Abstracts DHT peer discovery (UDP network vs mock responses)
- **PeerConnectionFactory** (in `peer_manager.rs`): Abstracts peer connection lifecycle (real TCP vs test doubles)

Production code uses concrete implementations (`DefaultTcpStreamFactory`, `TcpMessageIO`, `HttpTrackerClient`, `DhtManager`, `DefaultPeerConnectionFactory`), while tests inject fakes (`FakeMessageIO`) and mocks (`MockTrackerClient`, `MockDhtClient`, `MockPeerConnectionFactory`).

**Peer Connection Model**: Each `PeerConnection` splits TCP stream into reader/writer tasks after handshake. Inbound messages flow through `inbound_tx` channel, outbound messages through `outbound_tx` channel. This avoids locking and provides natural backpressure.

**Message Codec**: Uses `tokio_util::codec` with custom `PeerMessageDecoder` and `PeerMessageEncoder` for framing peer protocol messages.

**Background Task Orchestration**: PeerManager spawns background tasks with graceful shutdown:
- **Peer connector task**: Periodically connects to new peers (up to max_peers limit)
- **Piece assignment task**: Continuously assigns pending pieces to available peers
- **Completion handler**: Processes completed pieces, forwards to BitTorrent client, tracks progress
- **Failure handler**: Handles failed pieces with retry logic (up to 3 attempts)
- **Disconnect handler**: Cleans up disconnected peers, requeues in-flight pieces
- **Tracker refresh task**: Periodically announces to HTTP tracker for new peers (every 30 minutes)
- **DHT refresh task**: Periodically queries DHT for new peers (every 30 minutes)
- **Progress reporter**: Displays download progress once per minute

All tasks listen on a `broadcast::Receiver<()>` shutdown channel and terminate gracefully via `tokio::select!` when `PeerManagerHandle.shutdown()` is called.

**DHT Background Tasks**: DhtManager spawns 3 additional background tasks:
- **Message handler**: Listens on UDP socket, dispatches DHT responses to QueryManager
- **Bootstrap refresh**: Re-bootstraps DHT routing table every 15 minutes
- **Query cleanup**: Removes expired queries (5-second timeout) every 1 second

These tasks are spawned in main.rs and receive shutdown signals via `broadcast::channel()`.

**Download Orchestration**: The architecture cleanly separates concerns:
- **BitTorrent** (in `bittorrent_client.rs`): Handles torrent metadata, writes completed pieces to disk, verifies file integrity, and displays download progress with timing metrics
- **PeerManager**: Manages the peer connection pool (up to 20 concurrent peers by default), assigns pieces to least-busy peers based on bitfield availability, handles retry logic for failed pieces (unlimited retries), tracks completed piece indices to prevent re-queuing
- **PeerConnection**: Handles raw TCP I/O with individual peers, downloads piece blocks using pipelined requests (5 blocks ahead), reports completion/failure via channels

Pieces flow through channels: BitTorrent client requests pieces → PeerManager assigns to optimal peers → PeerConnection downloads → completion reported back to BitTorrent client for writing.

### BitTorrent Protocol Implementation

**Bencode**: Custom implementation (not using serde_bencode) for parsing .torrent files. Handles strings, integers, lists, dictionaries, and raw bytes (for piece hashes).

**Peer Wire Protocol**: Implements handshake, bitfield, interested, unchoke, request, and piece messages. Block size is 16 KiB (DEFAULT_BLOCK_SIZE).

**Info Hash**: Computed as SHA1 of the bencoded "info" dictionary from the torrent file.

**Tracker Protocol**: Uses compact format (compact=1) to get peer list as binary data.

**DHT Protocol**: Implements Kademlia-based DHT following BEP 5. Uses KRPC protocol over UDP with bencode encoding. Enables trackerless peer discovery via iterative lookups (alpha=3 parallel queries, max 8 iterations). See docs/DHT.md for detailed protocol documentation.

## Important Implementation Details

### Piece Hashes
The "pieces" field in .torrent files contains concatenated SHA1 hashes (20 bytes each). The encoder/decoder has special handling for this field to preserve raw bytes instead of attempting UTF-8 conversion.

### Error Handling
Uses `anyhow::Result<T>` for error propagation throughout the codebase. Incomplete message errors are handled via the custom `PeerMessageDecoder` which returns `Ok(None)` when more data is needed.

### Retry Logic
- **Handshake**: Exponential backoff retry logic (3 attempts with 200ms, 400ms, 800ms delays)
- **Piece downloads**: Failed pieces are retried with unlimited attempts
- **Tracker announces**: Background task retries on failure with exponential backoff

### Current State
The BitTorrent client has achieved 70%+ test coverage on core modules and has a clean separation of concerns:
- **BitTorrent** (`bittorrent_client.rs`): Orchestrates the download process, writes pieces to disk, verifies file integrity with SHA1 hashes, displays download progress and timing metrics (duration, speed)
- **PeerManager** (`peer_manager.rs`): Manages a pool of connected peers (up to 20 by default), assigns pieces to least-busy peers based on bitfield availability, handles unlimited retry logic for failed pieces, tracks completed piece indices to prevent re-queuing, spawns 7 background tasks for orchestration
- **PeerConnection** (`peer_connection.rs`): Handles block-level downloads using the BitTorrent peer protocol with pipelined requests (5 blocks ahead)

The client uses eager piece assignment (all pieces requested at once) for better parallelism and supports multi-peer parallel downloads. Pieces are limited to 1 concurrent download per peer to ensure stability.

## Common Gotchas

### Ownership and Channels
When working with async tasks, remember that receivers (mpsc::Receiver) must be taken out of structs using `.take()` before moving into spawned tasks.

### Message Framing
Peer messages are length-prefixed (4 bytes big-endian) followed by message type (1 byte) and payload. The decoder must handle partial reads gracefully by returning `Ok(None)` when more data is needed.

### Bitfield Parsing
Bitfield messages use MSB-first bit ordering. Bit 0 of byte 0 represents piece 0. The implementation in types.rs handles this correctly.

## Rust Coding Best Practices

### Tokio Interval Pattern Standard

When using `tokio::time::interval` in background loops, use one of these patterns:

**PATTERN A - Tick at start (RECOMMENDED):**
```rust
let mut interval = tokio::time::interval(Duration::from_secs(60));
loop {
    interval.tick().await;  // First tick immediate, subsequent ticks wait
    // Do work here
}
```

**PATTERN B - Skip first immediate tick:**
```rust
let mut interval = tokio::time::interval(Duration::from_secs(60));
interval.tick().await;  // Consume immediate tick
loop {
    interval.tick().await;  // All ticks now wait for full duration
    // Do work here
}
```

**ANTI-PATTERN (DO NOT USE):**
```rust
let mut interval = tokio::time::interval(Duration::from_secs(60));
loop {
    // Do work first
    interval.tick().await;  // WRONG: work happens immediately, then waits
}
```

**Why Pattern A is recommended:**
- The first tick completes immediately by design (allows loops to start working right away)
- Subsequent ticks wait for the specified duration
- Work happens at predictable intervals: T+0 (immediate), T+60, T+120, etc.

**Common pitfall:**
Placing `interval.tick().await` at the END of the loop causes work to execute immediately on the first iteration (before any async state is ready), then enters a long wait. This creates duplicate logs at startup and incorrect timing.

### Import Organization

Always declare imports at the top of the file and reference types/functions directly. Avoid inline module paths in type signatures and function calls.

**Bad:**
```rust
// Inline module paths in type signatures
fn process(
    stats: Arc<crate::BandwidthStats>,
    rx: broadcast::Receiver<crate::peer_messages::PeerMessage>
) { }

// Inline module paths in function calls
fn example() {
    log::debug!("message");
    std::mem::drop(value);
}
```

**Good:**
```rust
use crate::peer_messages::PeerMessage;
use crate::BandwidthStats;
use log::debug;

fn process(
    stats: Arc<BandwidthStats>,
    rx: broadcast::Receiver<PeerMessage>
) { }

fn example() {
    debug!("message");
    drop(value);
}
```

**Common imports to add:**
- Logging: `use log::{debug, info, warn, error};`
- Standard library: Import specific items rather than using `std::` prefix

### Avoid Deep Nesting (TL;DR)
- **Avoid deep nesting** → prefer early returns (`return`, `?`, `let-else`)
- **Use `?` aggressively** to eliminate match/if pyramids
- **Flatten control flow**: validate → exit early → proceed linearly
- **Prefer `let-else` over `match`** for guard-style checks
- **Extract logic into functions**, not inner blocks
- **Avoid `else` after `return`** — linear flow is idiomatic
- **Indentation > 3 levels** = refactor

**Example:**
```rust
// Bad: Deep nesting
fn process(data: Option<Vec<u8>>) -> Result<()> {
    if let Some(bytes) = data {
        if bytes.len() > 0 {
            match parse(bytes) {
                Ok(result) => {
                    // ... deep logic
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

// Good: Early returns, flat structure
fn process(data: Option<Vec<u8>>) -> Result<()> {
    let Some(bytes) = data else { return Ok(()) };
    if bytes.is_empty() { return Ok(()) }

    let result = parse(bytes)?;
    // ... logic at top level
    Ok(())
}
```

## Testing Strategy

### Core Principles
- **Target public APIs**: Write tests against public functions and methods to avoid coupling to implementation details
- **Coverage goal**: 70% on core modules (peer_manager.rs, peer_connection.rs, messages.rs)
- **Refactor resilience**: Tests should survive internal refactors by focusing on observable behavior, not internal state
- **No dead code**: Remove unused test helpers immediately; no `#[allow(dead_code)]` except during active refactoring

### Test Doubles: Fakes over Mocks

Following Rust async testing best practices (Tokio, Jon Gjengset's Rust for Rustaceans), this project uses **Fakes instead of Mocks**:

**Fake vs Mock:**
- **Fake**: Simplified working implementation (e.g., in-memory channel behaving like TCP)
- **Mock**: Pre-programmed expectations and responses (brittle, order-dependent)

**Why Fakes?**
- Test real behavior, not just error paths
- No order-dependent expectations that break easily
- Idiomatic Rust async patterns (channels, `tokio::io::duplex`)
- Used by Tokio, Actix, Hyper, Tonic for their own tests

**Implementation Guidelines:**

1. **FakeTcpConnector**: Use `tokio::io::duplex()` to return real readable/writable streams (in-memory)
   - Can test actual handshake logic
   - Returns `TcpStream`-like behavior without network

2. **FakeMessageIO**: Use `tokio::sync::mpsc::unbounded_channel()` for message passing
   - Behaves like real async I/O
   - Test sends messages via channel, code reads them naturally
   - No "expect" calls - just real async communication

3. **MockTrackerClient**: HTTP is external boundary - mock acceptable here
   - Use `tokio::sync::Mutex` (not `std::sync::Mutex`) in async code
   - Keep VecDeque pattern for queued responses

**Anti-patterns to Avoid:**
- `Arc<Mutex<VecDeque<T>>>` with `.lock().unwrap()` everywhere (use channels instead)
- Order-dependent expectations that make tests fragile
- Mocks that can only test failure cases
- `std::sync::Mutex` in async code (use `tokio::sync::Mutex`)
- Keeping "future-proofing" code that isn't currently needed
- Panicking. Prefer always returning an error instead.

**Coverage Achieved:**
- messages.rs: 78.26% (exceeds 70% target)
- peer_manager.rs: 71.35% (exceeds 70% target)
- peer_connection.rs: 73.25% (exceeds 70% target)

### Test Infrastructure

**Test Helpers** (`tests/helpers/fakes.rs`):
- **FakeMessageIO**: Channel-based bidirectional message I/O using `tokio::sync::mpsc::unbounded_channel()`
  - Creates pairs of connected MessageIO instances
  - Messages written to one side can be read from the other
  - Real async behavior without network I/O

- **MockTrackerClient**: VecDeque-based tracker response mock using `tokio::sync::Mutex`
  - `expect_announce(peers, interval)`: Queue successful response
  - `expect_failure(error_msg)`: Queue error response
  - Falls back to empty peer list if queue exhausted

- **MockDhtClient**: VecDeque-based DHT response mock using `tokio::sync::Mutex`
  - `expect_get_peers(peers)`: Queue successful DHT peer discovery response
  - `expect_failure(error_msg)`: Queue error response
  - Falls back to empty peer list if queue exhausted
  - Mirrors MockTrackerClient pattern for consistency

- **MockPeerConnector**: Always-successful peer connection mock
  - Returns ConnectedPeer with empty bitfield
  - Allows testing PeerManager orchestration without real TCP

**Background Task Testing**: All infinite loop background tasks accept `shutdown_rx: broadcast::Receiver<()>` and use `tokio::select!` to enable graceful shutdown in tests. Tests can verify task behavior by calling `handle.shutdown()` and awaiting completion.
