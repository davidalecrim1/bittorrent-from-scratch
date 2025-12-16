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

## Medium Priority

### Logging and Observability
- [ ] **Improve logging clarity across codebase**: Review and refactor all debug logging to keep only useful messages that help understand the application's internal workings. Remove verbose/redundant logs and ensure log messages are clear and actionable.

### Multi-Peer Download
- [ ] **Evolve download to use multiple peers in file_manager.rs:473**: Currently downloads pieces sequentially; enhance to download from multiple peers simultaneously (note: current implementation already supports this to some degree with max 5 peers)

### Encoding/Decoding
- [ ] **Review unknown field handling in encoding.rs:167**: Evaluate if there's a better approach than marking everything as unknown when parsing fails
- [ ] **Review pieces hash format in encoding.rs:329**: Consider whether to keep pieces as raw bytes or convert to string format

## Low Priority / Code Cleanup

### Deprecated Code Review
- [ ] **Review commented bitfield reading in file_manager.rs:555**: Evaluate if the old bitfield reading logic should be removed or preserved
- [ ] **Review async download blocking in file_manager.rs:565**: Consider if the commented async download logic should be implemented
- [ ] **Review piece writing in file_manager.rs:640**: Evaluate the commented file system writing logic
- [ ] **Improve the README file**: Mae sure to create a conciser version of the README file explaning this is an MVP of a BitTorrent client.

## Completed
- [x] Add hash verification after file download
- [x] Implement periodic tracker announcements (watch_tracker integration)

---

## Notes

- The codebase is mid-refactoring with some old implementation preserved in comments
- Current focus should be on completing the orchestration layer for robust multi-peer downloads
- Error handling improvements are crucial for production readiness
