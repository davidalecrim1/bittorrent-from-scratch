mod helpers;

#[cfg(test)]
mod tests {
    use bittorrent_from_scratch::peer_manager::PeerManager;
    use bittorrent_from_scratch::types::PeerManagerConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_peer_manager_initialization() {
        // Create a mock tracker client
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
            max_peers: 5,
        };

        let result = peer_manager.initialize(config).await;
        assert!(result.is_ok(), "Initialization should succeed");
    }

    #[tokio::test]
    async fn test_request_pieces_queues_pieces() {
        use bittorrent_from_scratch::types::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
            max_peers: 5,
        };

        peer_manager.initialize(config).await.unwrap();

        // Request some pieces
        let requests = vec![
            PieceDownloadRequest {
                piece_index: 0,
                expected_hash: [0u8; 20],
                piece_length: 16384,
            },
            PieceDownloadRequest {
                piece_index: 1,
                expected_hash: [1u8; 20],
                piece_length: 16384,
            },
        ];

        let result = peer_manager.request_pieces(requests).await;
        assert!(result.is_ok(), "request_pieces should succeed");
    }

    #[tokio::test]
    async fn test_download_piece() {
        use bittorrent_from_scratch::types::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
            max_peers: 5,
        };

        peer_manager.initialize(config).await.unwrap();

        let request = PieceDownloadRequest {
            piece_index: 0,
            expected_hash: [0u8; 20],
            piece_length: 16384,
        };

        let result = peer_manager.download_piece(request).await;
        assert!(result.is_ok(), "download_piece should succeed");
    }

    #[tokio::test]
    async fn test_start_returns_handle() {
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 10,
            max_peers: 5,
        };

        peer_manager.initialize(config).await.unwrap();

        let peer_manager = Arc::new(peer_manager);
        let (completion_tx, _completion_rx) = mpsc::channel(100);
        let (download_complete_tx, _download_complete_rx) = mpsc::channel(1);

        let result = peer_manager
            .start(
                "http://tracker.example.com/announce".to_string(),
                completion_tx,
                download_complete_tx,
            )
            .await;

        assert!(result.is_ok(), "start should return a handle");
        let handle = result.unwrap();
        handle.shutdown();
    }

    #[tokio::test]
    async fn test_multiple_piece_requests() {
        use bittorrent_from_scratch::types::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 100,
            max_peers: 5,
        };

        peer_manager.initialize(config).await.unwrap();

        // Request many pieces at once
        let requests: Vec<PieceDownloadRequest> = (0..50)
            .map(|i| PieceDownloadRequest {
                piece_index: i,
                expected_hash: [i as u8; 20],
                piece_length: 16384,
            })
            .collect();

        let result = peer_manager.request_pieces(requests).await;
        assert!(result.is_ok(), "requesting multiple pieces should succeed");
    }

    #[tokio::test]
    async fn test_get_peers_from_tracker() {
        use bittorrent_from_scratch::types::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        // Setup mock to return peers
        let expected_peers = vec![
            Peer::new("192.168.1.1".to_string(), 6881),
            Peer::new("192.168.1.2".to_string(), 6882),
        ];
        tracker_client
            .expect_announce(expected_peers.clone(), Some(1800))
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
            max_peers: 5,
        };

        peer_manager.initialize(config).await.unwrap();

        let result = peer_manager
            .get_peers(
                "http://tracker.example.com/announce".to_string(),
                [1u8; 20],
                1024 * 1024,
            )
            .await;

        assert!(result.is_ok(), "get_peers should succeed");
        let peers = result.unwrap();
        assert_eq!(peers.len(), 2, "should return 2 peers");
    }

    #[tokio::test]
    async fn test_connect_with_peers() {
        use bittorrent_from_scratch::types::Peer;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        // Setup tracker to return peers
        tracker_client
            .expect_announce(vec![Peer::new("192.168.1.1".to_string(), 6881)], Some(1800))
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
            max_peers: 5,
        };

        peer_manager.initialize(config).await.unwrap();

        // Get peers first
        let peers = peer_manager
            .get_peers(
                "http://tracker.example.com/announce".to_string(),
                [1u8; 20],
                1024 * 1024,
            )
            .await
            .unwrap();

        assert_eq!(peers.len(), 1, "should have 1 peer");

        let peer_manager = Arc::new(peer_manager);

        // Try connecting with peers
        let (completion_tx, _completion_rx) = mpsc::channel(100);
        let (failure_tx, _failure_rx) = mpsc::channel(100);
        let (disconnect_tx, _disconnect_rx) = mpsc::channel(100);

        let result = peer_manager
            .connect_with_peers(5, completion_tx, failure_tx, disconnect_tx)
            .await;

        assert!(result.is_ok(), "connect_with_peers should succeed");
    }

    #[tokio::test]
    async fn test_process_completion_forwards_to_writer() {
        use bittorrent_from_scratch::types::CompletedPiece;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
            max_peers: 5,
        };
        peer_manager.initialize(config).await.unwrap();

        let (file_writer_tx, mut file_writer_rx) = mpsc::channel(10);
        let (download_complete_tx, _) = mpsc::channel(1);

        let completed = CompletedPiece {
            piece_index: 5,
            data: vec![1, 2, 3, 4],
        };

        let result = peer_manager
            .process_completion(
                completed.clone(),
                &file_writer_tx,
                &download_complete_tx,
                10,
            )
            .await;

        assert!(result, "should continue processing");

        let forwarded = file_writer_rx.recv().await.unwrap();
        assert_eq!(forwarded.piece_index, 5);
    }

    #[tokio::test]
    async fn test_process_completion_sends_download_complete() {
        use bittorrent_from_scratch::types::CompletedPiece;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
            max_peers: 5,
        };
        peer_manager.initialize(config).await.unwrap();

        // Simulate 9 pieces already completed
        {
            let mut completed_count = peer_manager.completed_pieces.write().await;
            *completed_count = 9;
        }

        let (file_writer_tx, _) = mpsc::channel(10);
        let (download_complete_tx, mut download_complete_rx) = mpsc::channel(1);

        let completed = CompletedPiece {
            piece_index: 9,
            data: vec![1, 2, 3, 4],
        };

        let result = peer_manager
            .process_completion(completed, &file_writer_tx, &download_complete_tx, 10)
            .await;

        assert!(!result, "should signal completion");
        assert!(download_complete_rx.recv().await.is_some());
    }

    #[tokio::test]
    async fn test_process_failure_retry_logic() {
        use bittorrent_from_scratch::types::{FailedPiece, PieceDownloadRequest};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
            max_peers: 5,
        };
        peer_manager.initialize(config).await.unwrap();

        // Store the piece request first
        {
            let mut piece_requests = peer_manager.piece_requests.write().await;
            piece_requests.insert(
                5,
                PieceDownloadRequest {
                    piece_index: 5,
                    expected_hash: [0u8; 20],
                    piece_length: 16384,
                },
            );
        }

        let failed = FailedPiece {
            piece_index: 5,
            reason: "hash_mismatch".to_string(),
        };

        let result = peer_manager.process_failure(failed).await;

        assert!(result.is_ok());
        assert!(result.unwrap(), "piece should be requeued");

        // Verify it was added back to pending queue
        let pending = peer_manager.pending_pieces.read().await;
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn test_process_failure_max_retries_exceeded() {
        use bittorrent_from_scratch::types::{FailedPiece, PieceDownloadRequest};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
            max_peers: 5,
        };
        peer_manager.initialize(config).await.unwrap();

        // Store the piece request
        {
            let mut piece_requests = peer_manager.piece_requests.write().await;
            piece_requests.insert(
                5,
                PieceDownloadRequest {
                    piece_index: 5,
                    expected_hash: [0u8; 20],
                    piece_length: 16384,
                },
            );
        }

        // Simulate 10 failed attempts already
        {
            let mut failed_attempts = peer_manager.failed_attempts.write().await;
            failed_attempts.insert(5, 10);
        }

        let failed = FailedPiece {
            piece_index: 5,
            reason: "hash_mismatch".to_string(),
        };

        let result = peer_manager.process_failure(failed).await;

        assert!(result.is_ok());
        assert!(!result.unwrap(), "should not requeue after max retries");
    }

    #[tokio::test]
    async fn test_process_disconnect_requeues_in_flight_pieces() {
        use bittorrent_from_scratch::types::{ConnectedPeer, Peer, PeerDisconnected};
        use std::collections::HashSet;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
            max_peers: 5,
        };
        peer_manager.initialize(config).await.unwrap();

        // Setup a connected peer with active downloads
        let peer = Peer::new("192.168.1.1".to_string(), 6881);
        let peer_addr = peer.get_addr();
        let (download_request_tx, _rx) = mpsc::channel(10);
        let bitfield = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let mut connected_peer = ConnectedPeer::new(peer.clone(), download_request_tx, bitfield);

        // Mark pieces 1, 2, 3 as active downloads for this peer
        connected_peer.active_downloads.insert(1);
        connected_peer.active_downloads.insert(2);
        connected_peer.active_downloads.insert(3);

        {
            let mut connected_peers = peer_manager.connected_peers.write().await;
            connected_peers.insert(peer_addr.clone(), connected_peer);
        }

        // Add to in-flight tracking
        {
            let mut in_flight = peer_manager.in_flight_pieces.write().await;
            in_flight.insert(1, peer_addr.clone());
            in_flight.insert(2, peer_addr.clone());
            in_flight.insert(3, peer_addr.clone());
        }

        let (failure_tx, mut failure_rx) = mpsc::channel(10);

        let disconnect = PeerDisconnected {
            peer,
            reason: "connection_lost".to_string(),
        };

        let result = peer_manager
            .process_disconnect(disconnect, &failure_tx)
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 3, "should requeue 3 pieces");

        // Verify failures were sent
        let mut failed_pieces = HashSet::new();
        while let Ok(failed) = failure_rx.try_recv() {
            failed_pieces.insert(failed.piece_index);
        }
        assert_eq!(failed_pieces.len(), 3);
        assert!(failed_pieces.contains(&1));
        assert!(failed_pieces.contains(&2));
        assert!(failed_pieces.contains(&3));
    }

    #[tokio::test]
    async fn test_try_assign_piece_returns_false_when_queue_empty() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
            max_peers: 5,
        };
        peer_manager.initialize(config).await.unwrap();

        let result = peer_manager.try_assign_piece().await;

        assert!(result.is_ok());
        assert!(!result.unwrap(), "should return false when queue is empty");
    }
}
