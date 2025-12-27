# BitTorrent Client - Pending Tasks

This file tracks pending tasks and improvements identified in the codebase.

## Overall Improvements
  - [ ] **PENDING: Improve test coverage for peer_connection.rs back to 70%** (currently 62.6%, need 24 more lines)
    - [ ] Note: Remaining uncovered lines are mostly handshake code (requires real TCP), debug logs, and some error paths
  - [ ] Test with varying concurrency limits (2, 5, 10 pieces per peer)
  - [ ] Monitor memory usage and connection stability in production
- [ ] Keep track of the amount of bytes being downloaded for metrics/monitoring
- [ ] Retry failed pieces found during final integrity verification
  - Currently logs warnings if pieces fail hash check after download
  - Should re-request and re-download failed pieces instead of just warning
  - Implement retry loop with re-verification after retry

## Potential Future Improvements
- [ ] **Make MAX_PIECES_PER_PEER configurable** via CLI argument or config file
  - Currently hardcoded to 5, consider making this tunable for experimentation
  - Test with different values (2, 5, 10) to find optimal settings
- [ ] **Implement piece cancellation support** (currently ignored)
  - Handle PeerMessage::Cancel to support canceling in-flight block requests
  - May be needed for endgame mode or when switching to faster peers
- [ ] **Add metrics/observability**
  - Track bytes downloaded per peer
  - Track average download speed per peer
  - Monitor block request latency
  - Track piece task spawn/completion rates
  - Add Prometheus/OpenTelemetry integration
- [ ] **Add bandwidth throttling**
  - Limit total download/upload bandwidth
  - Implement per-peer rate limiting
- [ ] **Implement upload support** (currently download-only)
  - Handle PeerMessage::Request from other peers
  - Track upload rates and quotas
  - Implement choking algorithm

## Testability Improvements (Optional)
- [ ] **Create PeerManager trait**: Extract public interface into a trait to enable mocking PeerManager for BitTorrent Client tests. Currently BitTorrent Client is tightly coupled to concrete PeerManager implementation.
  - Define `trait PeerManager: Send + Sync` with key methods
  - Implement trait for concrete `PeerManagerImpl`
  - Create `MockPeerManager` for BitTorrent Client tests
