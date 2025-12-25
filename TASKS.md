# BitTorrent Client - Pending Tasks

This file tracks pending tasks and improvements identified in the codebase.

## Overall Improvements
- [ ] **Enable multiple concurrent downloads per peer**: Future improvement to allow each peer to download multiple pieces simultaneously
  - Requires: Update `assign_piece_to_peer` to remove `not_busy` check
  - Requires: Test with varying concurrency limits (2, 5, 10 pieces per peer)
  - Requires: Monitor memory usage and connection stability
  - Benefits: Improved download speeds by better utilizing peer bandwidth

## Code Organization
- Update the README to consider the latest changes.

## Testability Improvements (Optional)
- [ ] **Create PeerManager trait**: Extract public interface into a trait to enable mocking PeerManager for FileManager tests. Currently FileManager is tightly coupled to concrete PeerManager implementation.
  - Define `trait PeerManager: Send + Sync` with key methods
  - Implement trait for concrete `PeerManagerImpl`
  - Create `MockPeerManager` for FileManager tests

## FileManager Abstraction (Optional - Low Priority)
- [ ] **Implement FileHandler trait abstraction**: Create trait abstraction for file I/O to enable FileManager testing without disk access
  - Define `FileHandler`, `FileWriter`, `FileReader` traits
  - Implement `RealFileHandler` using tokio::fs
  - Create `MockFileHandler` for in-memory testing
  - Add FileManager unit tests
