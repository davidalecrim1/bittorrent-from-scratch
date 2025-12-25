# BitTorrent Client - Pending Tasks

This file tracks pending tasks and improvements identified in the codebase.

## Overall Improvements
- [ ] **Enable multiple concurrent downloads per peer**: Future improvement to allow each peer to download multiple pieces simultaneously
  - Requires: Update `assign_piece_to_peer` to remove `not_busy` check
  - Requires: Test with varying concurrency limits (2, 5, 10 pieces per peer)
  - Requires: Monitor memory usage and connection stability
  - Benefits: Improved download speeds by better utilizing peer bandwidth
- [ ] Find a way to drop peers that the bitfield is 0 when we reach 50 to connect with other ones. I think it's better for the network to limit the peer to less (20?).
- [ ] Keep track of the amount of bytes being downloaded for that purpose.

## Code Organization
- Update the README to consider the latest changes.

## Testability Improvements (Optional)
- [ ] **Create PeerManager trait**: Extract public interface into a trait to enable mocking PeerManager for BitTorrent Client tests. Currently BitTorrent Client is tightly coupled to concrete PeerManager implementation.
  - Define `trait PeerManager: Send + Sync` with key methods
  - Implement trait for concrete `PeerManagerImpl`
  - Create `MockPeerManager` for BitTorrent Client tests
