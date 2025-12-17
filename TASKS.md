# BitTorrent Client - Pending Tasks

This file tracks pending tasks and improvements identified in the codebase.

## High Priority

### Error Handling
- [x] **Refactor error handling in types.rs:726**: Replace string matching for incomplete messages with proper error types/sentinel errors (similar to Golang patterns)

### Orchestration Layer
- [x] **Implement peer connection management in peer_manager.rs:519**: Add `connect_with_peers()` method to connect to available peers up to a configurable limit
- [x] **Implement piece download orchestration in peer_manager.rs:562**: Add `download_piece()` method to request pieces from available peers
- [x] **Add connected peers tracking in peer_manager.rs:32**: Populate `connected_peers` field with ConnectedPeer struct as part of orchestration layer (already implemented in background task)

### Peer Management
- [x] **Handle peer disconnection in types.rs:396**: Implement mechanism to notify peer manager when a peer disconnects and should be dropped from the pool
- [ ] **Implement additional peer message types**: Add support for Have (ID 4), NotInterested (ID 3), Cancel (ID 8), and Keep-Alive messages to complete the BitTorrent peer wire protocol implementation

## Medium Priority

### Logging and Observability
- [x] **Improve logging clarity across codebase**: Review and refactor all debug logging to keep only useful messages that help understand the application's internal workings. Remove verbose/redundant logs and ensure log messages are clear and actionable.
- [x] **Add periodic progress reporting in peer_manager.rs**: Implemented background task that prints download progress (percentage based on completed pieces) and number of connected peers once per minute. Progress format: "[Progress] X/Y pieces (Z%) | N peers connected"

### Multi-Peer Download
- [x] **Evolve download to use multiple peers in file_manager.rs:473**: Multi-peer parallel downloads fully implemented with eager piece assignment, background orchestration, and support for up to 10 concurrent peer connections with load balancing

## Low Priority / Code Cleanup

### Code Organization
- [x] **Move PeerConnection to peer_connection.rs**: Extract PeerConnection struct and implementation from types.rs into a dedicated module to improve code organization and reduce file size

### Deprecated Code Review
- [x] **Review commented bitfield reading in file_manager.rs:555**: Removed in commit c795e66 - functionality refactored into PeerConnection
- [x] **Review async download blocking in file_manager.rs:565**: Removed in commit c795e66 - now handled with proper async/channel separation
- [x] **Review piece writing in file_manager.rs:640**: Removed in commit c795e66 - implemented with proper seek/write operations in download_file()
- [x] **Improve the README file**: Rewritten to be concise and MVP-focused with clear features, architecture overview, technical details, and limitations

## Completed
- [x] Add hash verification after file download
- [x] Implement periodic tracker announcements (watch_tracker integration)
