# BitTorrent Client - Pending Tasks

This file tracks pending tasks and improvements identified in the codebase.

## High Priority

### Bugs
- [ ] The Peer Connection is now silently failing for some reason. We see muiltiple available peers but cannot connected with any.
- [ ] Understand what is going on what pieces are not being downloaded.
- [ ] Add unit tests and more logs to easy undertand if things break.
- [ ] The Pieces after downloaded are having hash mismatch.

### Code Organization & Cleanup
- [x] **Consolidate peer_manager tests**: Merge `tests/coverage_boost_tests.rs` (15 tests) into `tests/peer_manager_tests.rs` (13 tests) to have all peer_manager tests in one logical location. The name "coverage_boost_tests" is a code smell indicating the file is not organized by responsibility.
- [x] **Remove unused mock code**: Delete `set_fail_next()` method from `MockPeerConnector` in `tests/helpers/fakes.rs` (currently unused and triggering dead_code warnings)
- [x] **Remove unused variable warning**: Fix `mut connected_peer2` warning in peer_manager tests

### Testability Improvements (Optional)
- [ ] **Create PeerManager trait**: Extract public interface into a trait to enable mocking PeerManager for FileManager tests. Currently FileManager is tightly coupled to concrete PeerManager implementation.
  - Define `trait PeerManager: Send + Sync` with key methods
  - Implement trait for concrete `PeerManagerImpl`
  - Create `MockPeerManager` for FileManager tests

### FileManager Abstraction (Optional - Low Priority)
- [ ] **Implement FileHandler trait abstraction**: Create trait abstraction for file I/O to enable FileManager testing without disk access
  - Define `FileHandler`, `FileWriter`, `FileReader` traits
  - Implement `RealFileHandler` using tokio::fs
  - Create `MockFileHandler` for in-memory testing
  - Add FileManager unit tests

## Completed - Test Coverage Achievement ✅

### Test Coverage (All Targets Exceeded!)
- [x] **messages.rs: 78.26% coverage** (108/138 lines) - EXCEEDS 70% target
- [x] **peer_manager.rs: 71.35% coverage** (264/370 lines) - EXCEEDS 70% target
- [x] **peer_connection.rs: 73.25% coverage** (178/243 lines) - EXCEEDS 70% target
- [x] **Overall project: 55.70% coverage** (753/1352 lines) - up from 49.29%
- [x] **88 comprehensive tests** across all test files

### Trait Abstractions for Testability
- [x] **TcpConnector trait**: Abstracts TCP connection establishment (`src/traits/mod.rs`)
  - [x] `RealTcpConnector` implementation (`src/tcp_connector.rs`)
  - [x] Used by PeerConnection for handshake

- [x] **MessageIO trait**: Abstracts peer protocol message I/O (`src/traits/mod.rs`)
  - [x] `TcpMessageIO` implementation (`src/io.rs`)
  - [x] `FakeMessageIO` for testing (`tests/helpers/fakes.rs`)
  - [x] PeerConnection uses via `start_with_io()` method
  - [x] 16 peer_connection tests using FakeMessageIO

- [x] **TrackerClient trait**: Abstracts HTTP tracker communication (`src/traits/mod.rs`)
  - [x] `HttpTrackerClient` implementation (`src/tracker_client.rs`)
  - [x] `MockTrackerClient` for testing (`tests/helpers/fakes.rs`)
  - [x] PeerManager dependency-injected with TrackerClient
  - [x] All peer_manager tests use MockTrackerClient

- [x] **PeerConnector trait**: Abstracts peer connection establishment (`src/peer_manager.rs`)
  - [x] `RealPeerConnector` implementation wraps PeerConnection
  - [x] `MockPeerConnector` for testing (`tests/helpers/fakes.rs`)
  - [x] PeerManager dependency-injected with PeerConnector
  - [x] Tests refactored to use MockPeerConnector instead of real TCP

### Background Task Testing
- [x] **Shutdown channel support**: Added graceful shutdown to infinite loop background tasks
  - [x] `handle_peer_connector` accepts `shutdown_rx` and uses `tokio::select!`
  - [x] `handle_piece_assignment` accepts `shutdown_rx` and uses `tokio::select!`
  - [x] Tests verify shutdown behavior

- [x] **Extracted testable logic**: Split loop bodies from infinite loop wrappers
  - [x] `process_completion()` - public, testable, returns bool
  - [x] `process_failure()` - public, testable, returns Result<bool>
  - [x] `process_disconnect()` - public, testable, returns Result<usize>
  - [x] `try_assign_piece()` - public, testable, returns Result<bool>
  - [x] Comprehensive tests for each extracted function

### Test Infrastructure
- [x] **Test helpers module**: `tests/helpers/fakes.rs` with reusable test doubles
  - [x] FakeMessageIO (channel-based, bidirectional)
  - [x] MockTrackerClient (queue-based responses)
  - [x] MockPeerConnector (always succeeds, configurable)

- [x] **Error path testing**: Comprehensive error scenario coverage
  - [x] Write errors trigger disconnect
  - [x] Read errors trigger disconnect
  - [x] Stream closure triggers disconnect
  - [x] Missing piece requests return errors
  - [x] No peers available handling
  - [x] Piece assignment failures with requeueing

- [x] **Makefile enhancement**: Added `make coverage` command using cargo-tarpaulin

### Test Files Organization
- [x] `tests/peer_connection_tests.rs` - 16 tests for PeerConnection (correct location)
- [x] `tests/peer_manager_tests.rs` - 13 tests for PeerManager (correct location)
- [x] `tests/coverage_boost_tests.rs` - 15 tests for PeerManager (needs consolidation)
- [x] `tests/helpers/fakes.rs` - Reusable test doubles
- [x] `tests/helpers/mod.rs` - Test utilities

## Completed - Previous Work

### Error Handling
- [x] **Refactor error handling in types.rs:726**: Replace string matching for incomplete messages with proper error types/sentinel errors (similar to Golang patterns)

### Orchestration Layer
- [x] **Implement peer connection management in peer_manager.rs:519**: Add `connect_with_peers()` method to connect to available peers up to a configurable limit
- [x] **Implement piece download orchestration in peer_manager.rs:562**: Add `download_piece()` method to request pieces from available peers
- [x] **Add connected peers tracking in peer_manager.rs:32**: Populate `connected_peers` field with ConnectedPeer struct as part of orchestration layer (already implemented in background task)

### Peer Management
- [x] **Handle peer disconnection in types.rs:396**: Implement mechanism to notify peer manager when a peer disconnects and should be dropped from the pool
- [x] **Implement additional peer message types**: Add support for Have (ID 4), NotInterested (ID 3), Cancel (ID 8), and Keep-Alive messages to complete the BitTorrent peer wire protocol implementation

### Logging and Observability
- [x] **Improve logging clarity across codebase**: Review and refactor all debug logging to keep only useful messages that help understand the application's internal workings. Remove verbose/redundant logs and ensure log messages are clear and actionable.
- [x] **Add periodic progress reporting in peer_manager.rs**: Implemented background task that prints download progress (percentage based on completed pieces) and number of connected peers once per minute. Progress format: "[Progress] X/Y pieces (Z%) | N peers connected"

### Multi-Peer Download
- [x] **Evolve download to use multiple peers in file_manager.rs:473**: Multi-peer parallel downloads fully implemented with eager piece assignment, background orchestration, and support for up to 10 concurrent peer connections with load balancing

### Code Organization
- [x] **Move PeerConnection to peer_connection.rs**: Extract PeerConnection struct and implementation from types.rs into a dedicated module to improve code organization and reduce file size

### Deprecated Code Review
- [x] **Review commented bitfield reading in file_manager.rs:555**: Removed in commit c795e66 - functionality refactored into PeerConnection
- [x] **Review async download blocking in file_manager.rs:565**: Removed in commit c795e66 - now handled with proper async/channel separation
- [x] **Review piece writing in file_manager.rs:640**: Removed in commit c795e66 - implemented with proper seek/write operations in download_file()
- [x] **Improve the README file**: Rewritten to be concise and MVP-focused with clear features, architecture overview, technical details, and limitations

### General Completed Items
- [x] Add hash verification after file download
- [x] Implement periodic tracker announcements (watch_tracker integration)
