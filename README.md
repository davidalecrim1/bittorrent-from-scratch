# BitTorrent Client (Rust)

Educational BitTorrent client implementation in Rust, built from scratch to explore distributed systems, network protocols, and concurrent programming.

This project uses an **actor concurrency model** with **event-driven communication** through channels, enabling true parallelism while naturally preventing shared mutable state issues.

## Features

- Parse .torrent files using custom Bencode decoder
- Connect to tracker servers and discover peers
- Download files from multiple peers concurrently (up to 20 peers by default)
- Block pipelining: 5 concurrent block requests per peer for optimal throughput
- Verify downloaded pieces with SHA1 hashing
- Automatic retry logic for failed pieces with unlimited retries
- Graceful shutdown with proper piece requeuing on peer disconnect
- Real-time progress tracking in terminal

## Quick Start

### Run Sample Download

```bash
make sample-run-1
```

### Build and Install Binary

In MacOS:
```bash
make install
```

This installs the binary as `bittorrent` in `~/.local/bin/`. Make sure `~/.local/bin` is in your PATH.

### Usage

```bash
bittorrent -i <torrent-file> -o <output-directory> [options]

Options:
  -i, --input <FILE>              Path to .torrent file
  -o, --output <DIR>              Output directory for downloaded file
  --max-download-rate <RATE>      Max download rate (e.g., 2M, 500K)
  --max-upload-rate <RATE>        Max upload rate (e.g., 100K, 1M)
  --max-peers <NUM>               Maximum concurrent peers (default: 20)
  --log-dir <DIR>                 Directory to store log files
                                  (default: ~/Library/Logs/bittorrent on macOS, ./logs elsewhere)
```

Example:
```bash
bittorrent -i ./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent -o ~/Downloads --max-download-rate 2M --max-peers 15
```

### Example Output

```
Peers: 5 connected, 52 available, 0 choking | Pieces: 4/24208 (0%) - 24200 pending, 4 downloading | ↓ 59.2 KB/s ↑ 59.3 KB/s
```

Watch as pieces flow in from multiple peers across the internet and reassemble into a verified file!

## Architecture Overview
![Excalidraw Diagram](./docs/system-design-bittorrent-client.excalidraw.png)

## Technical Details

### Protocol & Networking
- **Protocol**: BitTorrent peer wire protocol over TCP with handshake, bitfield, interested, unchoke, request, and piece messages
- **Async Runtime**: Tokio-based with split read/write tasks communicating via `mpsc` channels
- **Piece Validation**: SHA1 hashing of downloaded pieces (20 bytes per hash)
- **Block Size**: 16 KiB per network request (as per BitTorrent spec)

### Concurrency Architecture
- **Peer Pool**: Up to 20 concurrent peer connections by default for maximum parallelism
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
- **Theoretical Max Concurrency**: 20 peers downloading in parallel (configurable)
- **Network Efficiency**: Block pipelining (5 blocks ahead) keeps TCP pipeline full
- **Latency Optimization**: Pipelined requests reduce round-trip wait time
- **Backpressure**: Channel-based communication provides natural backpressure
- **Stability**: Single piece per peer prevents overwhelming individual connections

### Tests
Run tests:
```bash
cargo test              # Run all tests
make coverage          # Generate coverage report with tarpaulin
```

## Download Completion

When the download finishes, you'll see a summary:

```
[2026-01-08T06:40:22Z INFO] ✅ All pieces verified successfully
[2026-01-08T06:40:22Z INFO] 📊 Download completed in 38592.34s
[2026-01-08T06:40:22Z INFO] 📦 File size: 6051.91 MB
[2026-01-08T06:40:22Z INFO] ⚡ Average speed: 0.16 MB/s
```

## Limitations & Educational Notes

This is an **MVP implementation** focused on learning:

- Single-file torrents only (no multi-file support)
- No DHT (Distributed Hash Table) support
- No magnet link support
- Built from scratch for educational purposes
