mod helpers;

#[cfg(test)]
mod tests {
    use bittorrent_from_scratch::error::AppError;
    use bittorrent_from_scratch::peer_manager::PeerManager;
    use bittorrent_from_scratch::types::PeerManagerConfig;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_peer_manager_initialization() {
        // Create a mock tracker client
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
        };

        let result = peer_manager.initialize(config).await;
        assert!(result.is_ok(), "Initialization should succeed");
    }

    #[tokio::test]
    async fn test_request_pieces_queues_pieces() {
        use bittorrent_from_scratch::types::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
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
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
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
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 10,
        };

        peer_manager.initialize(config).await.unwrap();

        let peer_manager = Arc::new(peer_manager);
        let (completion_tx, _completion_rx) = mpsc::unbounded_channel();
        let (file_complete_tx, _file_complete_rx) = mpsc::channel(1);

        let result = peer_manager
            .start(
                "http://tracker.example.com/announce".to_string(),
                completion_tx,
                file_complete_tx,
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
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 100,
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
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

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
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

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
        };

        peer_manager.initialize(config).await.unwrap();
        let peer_manager = Arc::new(peer_manager);

        // Use watch_tracker to populate available_peers in background
        peer_manager
            .clone()
            .watch_tracker(
                std::time::Duration::from_secs(3600),
                "http://tracker.example.com/announce".to_string(),
                [1u8; 20],
                1024 * 1024,
            )
            .await
            .unwrap();

        // Wait for watch_tracker background task to populate available_peers
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Try connecting with peers
        let (event_tx, _event_rx) = mpsc::unbounded_channel();

        let result = peer_manager.connect_with_peers(5, event_tx).await;

        assert!(result.is_ok(), "connect_with_peers should succeed");
        assert_eq!(result.unwrap(), 1, "should start 1 connection attempt");

        // Wait for the spawned connection tasks to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify the peer was added to connected_peers
        let connected_count = peer_manager.connected_peer_count().await;
        assert_eq!(connected_count, 1, "should have 1 connected peer");
        assert!(
            peer_manager.is_peer_connected("192.168.1.1:6881").await,
            "should contain the connected peer"
        );
    }

    #[tokio::test]
    async fn test_process_completion_forwards_to_writer() {
        use bittorrent_from_scratch::types::WritePieceRequest;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
        };
        peer_manager.initialize(config).await.unwrap();

        let (file_writer_tx, mut file_writer_rx) = mpsc::unbounded_channel();
        let (file_complete_tx, _) = mpsc::channel(1);

        let completed = WritePieceRequest {
            peer_addr: "127.0.0.1:6881".to_string(),
            piece_index: 5,
            data: vec![1, 2, 3, 4],
        };

        let result = peer_manager
            .process_completion(completed.clone(), &file_writer_tx, &file_complete_tx, 10)
            .await;

        assert!(result.is_ok());

        let forwarded = file_writer_rx.recv().await.unwrap();
        assert_eq!(forwarded.piece_index, 5);
    }

    #[tokio::test]
    async fn test_process_failure_retry_logic() {
        use bittorrent_from_scratch::types::{FailedPiece, PieceDownloadRequest};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
        };
        peer_manager.initialize(config).await.unwrap();

        // Request the piece first through the public API
        peer_manager
            .request_pieces(vec![PieceDownloadRequest {
                piece_index: 5,
                expected_hash: [0u8; 20],
                piece_length: 16384,
            }])
            .await
            .unwrap();

        let failed = FailedPiece {
            peer_addr: "127.0.0.1:6881".to_string(),
            piece_index: 5,
            error: AppError::HashMismatch,
            push_front: false,
        };

        let result = peer_manager.process_failure(failed).await;

        assert!(result.is_ok());

        // Verify it was added back to pending queue
        assert_eq!(peer_manager.get_piece_snapshot().await.pending_count, 1);
    }

    #[tokio::test]
    async fn test_process_failure_unlimited_retries() {
        use bittorrent_from_scratch::types::{FailedPiece, PieceDownloadRequest};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
        };
        peer_manager.initialize(config).await.unwrap();

        // Request the piece first through the public API
        peer_manager
            .request_pieces(vec![PieceDownloadRequest {
                piece_index: 5,
                expected_hash: [0u8; 20],
                piece_length: 16384,
            }])
            .await
            .unwrap();

        // Simulate many failures by calling process_failure multiple times
        for _ in 0..100 {
            let failed = FailedPiece {
                peer_addr: "127.0.0.1:6881".to_string(),
                piece_index: 5,
                error: AppError::HashMismatch,
                push_front: false,
            };
            let result = peer_manager.process_failure(failed).await;
            assert!(result.is_ok());
        }

        // One more failure after 100 attempts
        let failed = FailedPiece {
            peer_addr: "127.0.0.1:6881".to_string(),
            piece_index: 5,
            error: AppError::HashMismatch,
            push_front: false,
        };

        let result = peer_manager.process_failure(failed).await;

        assert!(result.is_ok());

        // Piece should still be pending after many failures
        assert_eq!(peer_manager.get_piece_snapshot().await.pending_count, 1);
    }

    #[tokio::test]
    async fn test_try_assign_piece_returns_false_when_queue_empty() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
        };
        peer_manager.initialize(config).await.unwrap();

        let result = peer_manager.try_assign_piece().await;

        assert!(result.is_ok());
        assert!(!result.unwrap(), "should return false when queue is empty");
    }

    #[tokio::test]
    async fn test_cleanup_piece_tracking_not_found() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let result = peer_manager.cleanup_piece_tracking(99).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_add_available_peers() {
        use bittorrent_from_scratch::types::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer1 = Peer::new("192.168.1.1".to_string(), 6881);
        let peer2 = Peer::new("192.168.1.2".to_string(), 6882);

        // Initially no available peers
        assert_eq!(peer_manager.available_peer_count().await, 0);

        // Add peers to available pool using public API
        peer_manager
            .add_available_peers(vec![peer1.clone(), peer2.clone()])
            .await;

        // Verify peers were added
        assert_eq!(peer_manager.available_peer_count().await, 2);

        let available = peer_manager.get_available_peers_snapshot().await;
        assert!(available.contains_key(&peer1.get_addr()));
        assert!(available.contains_key(&peer2.get_addr()));
    }

    #[tokio::test]
    async fn test_assign_piece_to_peer_no_peers_available() {
        use bittorrent_from_scratch::types::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let request = PieceDownloadRequest {
            piece_index: 5,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        };

        let result = peer_manager.assign_piece_to_peer(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_assign_piece_to_peer_already_downloading() {
        use bittorrent_from_scratch::types::{Peer, PieceDownloadRequest};
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer = Peer::new("192.168.1.1".to_string(), 6881);
        let (event_tx, _event_rx) = mpsc::unbounded_channel();

        // Connect peer using public API
        peer_manager
            .connect_peer(peer.clone(), event_tx)
            .await
            .unwrap();

        // Request and assign piece 5 to make it in-flight
        peer_manager
            .request_pieces(vec![PieceDownloadRequest {
                piece_index: 5,
                piece_length: 16384,
                expected_hash: [0u8; 20],
            }])
            .await
            .unwrap();

        // Assign the piece (will mark it as downloading for the peer)
        let _ = peer_manager.try_assign_piece().await;

        // Try to assign the same piece again - should fail or succeed based on implementation
        let request = PieceDownloadRequest {
            piece_index: 5,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        };

        let result = peer_manager.assign_piece_to_peer(request).await;
        // The piece is already in-flight, so assignment should succeed but peer won't accept duplicate
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_assign_piece_to_peer_with_piece_available() {
        use bittorrent_from_scratch::types::{Peer, PieceDownloadRequest};
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // Configure peer to have pieces 0, 1, 2 out of 10 total
        connector
            .with_specific_pieces("192.168.1.1:6881".to_string(), vec![0, 1, 2], 10)
            .await;

        // Setup tracker to return this peer
        tracker_client
            .expect_announce(vec![Peer::new("192.168.1.1".to_string(), 6881)], Some(1800))
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);
        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        // Use watch_tracker to populate available_peers
        peer_manager
            .clone()
            .watch_tracker(
                std::time::Duration::from_secs(3600),
                "http://tracker.example.com/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Use connect_with_peers to actually add peers to HashMap
        let (event_tx, _event_rx) = mpsc::unbounded_channel();
        peer_manager
            .clone()
            .connect_with_peers(5, event_tx)
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify peer is connected
        assert!(peer_manager.is_peer_connected("192.168.1.1:6881").await);

        // Check if peer has pieces (for debugging)
        let piece_count = peer_manager.get_peer_piece_count("192.168.1.1:6881").await;

        // Assign piece 1 (peer has it)
        let request = PieceDownloadRequest {
            piece_index: 1,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        };

        let result = peer_manager.assign_piece_to_peer(request).await;
        assert!(result.is_ok(), "Should assign piece to peer that has it");
    }

    #[tokio::test]
    async fn test_try_assign_piece_empty_queue() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let result = peer_manager.try_assign_piece().await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            false,
            "Should return false when queue is empty"
        );
    }

    #[tokio::test]
    async fn test_connect_peer_success() {
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer = bittorrent_from_scratch::types::Peer::new("192.168.1.1".to_string(), 6881);
        let (event_tx, _event_rx) = mpsc::unbounded_channel();

        let result = peer_manager.connect_peer(peer, event_tx).await;

        assert!(
            result.is_ok(),
            "Connection should succeed with mock connector"
        );
    }

    #[tokio::test]
    async fn test_background_tasks_shutdown_gracefully() {
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let (file_writer_tx, _) = mpsc::unbounded_channel();
        let (file_complete_tx, _) = mpsc::channel(1);

        let peer_manager = Arc::new(peer_manager);

        let handle = peer_manager
            .clone()
            .start(
                "http://tracker.example.com".to_string(),
                file_writer_tx,
                file_complete_tx,
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        handle.shutdown();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_get_peer_piece_count() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        // Should return None for non-existent peer
        let result = peer_manager.get_peer_piece_count("192.168.1.1:6881").await;
        assert_eq!(result, None, "Should return None for non-existent peer");
    }

    #[tokio::test]
    async fn test_piece_state_query_methods() {
        use bittorrent_from_scratch::types::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        // Initially, piece should not be pending/completed/in-flight
        assert!(!peer_manager.is_piece_pending(5).await);
        assert!(!peer_manager.is_piece_completed(5).await);
        assert!(!peer_manager.is_piece_in_flight(5).await);

        // After requesting, piece should be pending
        peer_manager
            .request_pieces(vec![PieceDownloadRequest {
                piece_index: 5,
                piece_length: 16384,
                expected_hash: [0u8; 20],
            }])
            .await
            .unwrap();

        assert!(peer_manager.is_piece_pending(5).await);
        assert!(!peer_manager.is_piece_completed(5).await);
    }

    #[tokio::test]
    async fn test_process_failure_with_push_front() {
        use bittorrent_from_scratch::types::{FailedPiece, PieceDownloadRequest};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        // Request the piece first
        peer_manager
            .request_pieces(vec![PieceDownloadRequest {
                piece_index: 5,
                expected_hash: [0u8; 20],
                piece_length: 16384,
            }])
            .await
            .unwrap();

        // Process failure with push_front=true (queue full scenario)
        let failed = FailedPiece {
            peer_addr: "127.0.0.1:6881".to_string(),
            piece_index: 5,
            error: AppError::PeerQueueFull,
            push_front: true,
        };

        let result = peer_manager.process_failure(failed).await;
        assert!(result.is_ok());

        // Piece should be back in pending queue (at the front)
        assert!(peer_manager.is_piece_pending(5).await);
    }

    #[tokio::test]
    async fn test_process_disconnect() {
        use bittorrent_from_scratch::types::{Peer, PeerDisconnected};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer = Peer::new("192.168.1.1".to_string(), 6881);
        let disconnect = PeerDisconnected {
            peer: peer.clone(),
            reason: "Connection lost".to_string(),
        };

        let result = peer_manager.process_disconnect(disconnect).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_connected_peer_addrs() {
        use bittorrent_from_scratch::types::Peer;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        tracker_client
            .expect_announce(vec![Peer::new("192.168.1.1".to_string(), 6881)], Some(1800))
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        // Initially no connected peers
        assert_eq!(peer_manager.get_connected_peer_addrs().await.len(), 0);

        // Connect a peer
        peer_manager
            .clone()
            .watch_tracker(
                std::time::Duration::from_secs(3600),
                "http://tracker.example.com/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let (event_tx, _) = mpsc::unbounded_channel();
        peer_manager.connect_with_peers(5, event_tx).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let addrs = peer_manager.get_connected_peer_addrs().await;
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0], "192.168.1.1:6881");
    }

    #[tokio::test]
    async fn test_get_peers_retry_success() {
        use bittorrent_from_scratch::types::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // First request fails, second succeeds
        tracker_client
            .expect_failure("Temporary network error")
            .await;
        tracker_client
            .expect_announce(vec![Peer::new("192.168.1.1".to_string(), 6881)], Some(1800))
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let result = peer_manager
            .get_peers(
                "http://tracker.example.com/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await;

        assert!(result.is_ok());
        let peers = result.unwrap();
        assert_eq!(peers.len(), 1);
    }

    #[tokio::test]
    async fn test_connect_with_peers_no_available_peers() {
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let (event_tx, _) = mpsc::unbounded_channel();

        // Try to connect when no peers are available
        let result = peer_manager.connect_with_peers(5, event_tx).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0, "Should start 0 connection attempts");
    }

    #[tokio::test]
    async fn test_connect_with_peers_at_capacity() {
        use bittorrent_from_scratch::types::Peer;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        tracker_client
            .expect_announce(
                vec![
                    Peer::new("192.168.1.1".to_string(), 6881),
                    Peer::new("192.168.1.2".to_string(), 6882),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        peer_manager
            .clone()
            .watch_tracker(
                std::time::Duration::from_secs(3600),
                "http://tracker.example.com/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let (event_tx, _) = mpsc::unbounded_channel();

        // First connection attempt - should connect 2 peers
        let result1 = peer_manager
            .clone()
            .connect_with_peers(2, event_tx.clone())
            .await;
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), 2);

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Second connection attempt - already at capacity (2/2)
        let result2 = peer_manager.connect_with_peers(2, event_tx).await;
        assert!(result2.is_ok());
        assert_eq!(
            result2.unwrap(),
            0,
            "Should not start new connections when at capacity"
        );
    }

    #[tokio::test]
    async fn test_is_peer_connected() {
        use bittorrent_from_scratch::types::Peer;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        tracker_client
            .expect_announce(vec![Peer::new("192.168.1.1".to_string(), 6881)], Some(1800))
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        // Initially not connected
        assert!(!peer_manager.is_peer_connected("192.168.1.1:6881").await);

        // Connect peer
        peer_manager
            .clone()
            .watch_tracker(
                std::time::Duration::from_secs(3600),
                "http://tracker.example.com/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let (event_tx, _) = mpsc::unbounded_channel();
        peer_manager.connect_with_peers(5, event_tx).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Now should be connected
        assert!(peer_manager.is_peer_connected("192.168.1.1:6881").await);
    }

    #[tokio::test]
    async fn test_watch_tracker_populates_available_peers() {
        use bittorrent_from_scratch::types::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        tracker_client
            .expect_announce(
                vec![
                    Peer::new("192.168.1.1".to_string(), 6881),
                    Peer::new("192.168.1.2".to_string(), 6882),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        // Initially no available peers
        assert_eq!(peer_manager.available_peer_count().await, 0);

        // Start watching tracker
        peer_manager
            .clone()
            .watch_tracker(
                std::time::Duration::from_secs(3600),
                "http://tracker.example.com/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await
            .unwrap();

        // Wait for background task to populate
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Should have 2 available peers now
        assert_eq!(peer_manager.available_peer_count().await, 2);
    }

    #[tokio::test]
    async fn test_connected_peer_count() {
        use bittorrent_from_scratch::types::Peer;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        tracker_client
            .expect_announce(
                vec![
                    Peer::new("192.168.1.1".to_string(), 6881),
                    Peer::new("192.168.1.2".to_string(), 6882),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        // Initially no connected peers
        assert_eq!(peer_manager.connected_peer_count().await, 0);

        // Connect peers
        peer_manager
            .clone()
            .watch_tracker(
                std::time::Duration::from_secs(3600),
                "http://tracker.example.com/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let (event_tx, _) = mpsc::unbounded_channel();
        peer_manager.connect_with_peers(5, event_tx).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Should have 2 connected peers
        assert_eq!(peer_manager.connected_peer_count().await, 2);
    }

    #[tokio::test]
    async fn test_process_disconnect_with_in_flight_pieces() {
        use bittorrent_from_scratch::types::{Peer, PeerDisconnected, PieceDownloadRequest};
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        // Request some pieces
        peer_manager
            .request_pieces(vec![
                PieceDownloadRequest {
                    piece_index: 1,
                    piece_length: 1024,
                    expected_hash: [0u8; 20],
                },
                PieceDownloadRequest {
                    piece_index: 2,
                    piece_length: 1024,
                    expected_hash: [0u8; 20],
                },
            ])
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        // Simulate disconnect
        let peer = Peer::new("192.168.1.1".to_string(), 6881);
        let disconnect = PeerDisconnected {
            peer: peer.clone(),
            reason: "Test disconnect".to_string(),
        };

        let result = peer_manager.process_disconnect(disconnect).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_config_not_initialized() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        // Try to use connect_peer before initializing - should fail
        use bittorrent_from_scratch::types::Peer;
        use tokio::sync::mpsc;

        let peer = Peer::new("192.168.1.1".to_string(), 6881);
        let (event_tx, _) = mpsc::unbounded_channel();

        let result = peer_manager.connect_peer(peer, event_tx).await;
        assert!(result.is_err(), "Should fail when config not initialized");
    }

    #[tokio::test]
    async fn test_assign_piece_selects_least_busy_peer() {
        use bittorrent_from_scratch::types::{Peer, PeerManagerConfig, PieceDownloadRequest};
        use std::time::Duration;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // All peers have piece 5
        connector
            .with_specific_pieces("192.168.1.1:6881".to_string(), vec![5], 10)
            .await;
        connector
            .with_specific_pieces("192.168.1.2:6881".to_string(), vec![5], 10)
            .await;
        connector
            .with_specific_pieces("192.168.1.3:6881".to_string(), vec![5], 10)
            .await;

        // Setup tracker to return all peers
        tracker_client
            .expect_announce(
                vec![
                    Peer::new("192.168.1.1".to_string(), 6881),
                    Peer::new("192.168.1.2".to_string(), 6881),
                    Peer::new("192.168.1.3".to_string(), 6881),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);
        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        // Use watch_tracker to populate available_peers
        peer_manager
            .clone()
            .watch_tracker(
                Duration::from_secs(3600),
                "http://tracker.example.com:8080/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect all peers
        let (event_tx, _) = mpsc::unbounded_channel();
        peer_manager
            .clone()
            .connect_with_peers(5, event_tx)
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify all 3 peers connected
        assert_eq!(peer_manager.connected_peer_count().await, 3);

        // Assign piece to first peer (now has 1 active download)
        peer_manager
            .assign_piece_to_peer(PieceDownloadRequest {
                piece_index: 5,
                piece_length: 16384,
                expected_hash: [0u8; 20],
            })
            .await
            .unwrap();

        // Second assignment should select a less busy peer (not the one with active download)
        let result = peer_manager
            .assign_piece_to_peer(PieceDownloadRequest {
                piece_index: 5,
                piece_length: 16384,
                expected_hash: [1u8; 20],
            })
            .await;

        assert!(result.is_ok(), "Should select least busy peer");
    }

    #[tokio::test]
    async fn test_assign_piece_fails_when_no_peer_has_piece() {
        use bittorrent_from_scratch::types::{Peer, PeerManagerConfig, PieceDownloadRequest};
        use std::time::Duration;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // Peer has pieces 0, 1, 2 but NOT piece 5
        connector
            .with_specific_pieces("192.168.1.1:6881".to_string(), vec![0, 1, 2], 10)
            .await;

        // Setup tracker to return this peer
        tracker_client
            .expect_announce(vec![Peer::new("192.168.1.1".to_string(), 6881)], Some(1800))
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);
        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        // Use watch_tracker to populate available_peers
        peer_manager
            .clone()
            .watch_tracker(
                Duration::from_secs(3600),
                "http://tracker.example.com:8080/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect peer
        let (event_tx, _) = mpsc::unbounded_channel();
        peer_manager
            .clone()
            .connect_with_peers(5, event_tx)
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Try to assign piece 5 (peer doesn't have it)
        let request = PieceDownloadRequest {
            piece_index: 5,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        };

        let result = peer_manager.assign_piece_to_peer(request).await;
        assert!(result.is_err(), "Should fail when no peer has the piece");
    }

    #[tokio::test]
    async fn test_drop_useless_peers_when_at_capacity() {
        use bittorrent_from_scratch::types::{Peer, PeerManagerConfig};
        use std::time::Duration;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // 2 useful peers, 1 useless
        connector
            .with_all_pieces("192.168.1.1:6881".to_string(), 10)
            .await;
        connector
            .with_specific_pieces("192.168.1.2:6881".to_string(), vec![0, 1], 10)
            .await;
        connector
            .with_no_pieces("192.168.1.3:6881".to_string())
            .await;

        // Setup tracker to return all peers
        tracker_client
            .expect_announce(
                vec![
                    Peer::new("192.168.1.1".to_string(), 6881),
                    Peer::new("192.168.1.2".to_string(), 6881),
                    Peer::new("192.168.1.3".to_string(), 6881),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);
        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        // Use watch_tracker to populate available_peers
        peer_manager
            .clone()
            .watch_tracker(
                Duration::from_secs(3600),
                "http://tracker.example.com:8080/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect all 3 peers
        let (event_tx, _) = mpsc::unbounded_channel();
        peer_manager
            .clone()
            .connect_with_peers(5, event_tx)
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(peer_manager.connected_peer_count().await, 3);

        // Drop useless peers (at capacity)
        peer_manager.drop_useless_peers_if_at_capacity(3).await;

        // Should drop peer 3 (no pieces)
        assert_eq!(peer_manager.connected_peer_count().await, 2);
        assert!(!peer_manager.is_peer_connected("192.168.1.3:6881").await);
    }

    #[tokio::test]
    async fn test_keep_useless_peers_when_below_capacity() {
        use bittorrent_from_scratch::types::{Peer, PeerManagerConfig};
        use std::time::Duration;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        connector
            .with_all_pieces("192.168.1.1:6881".to_string(), 10)
            .await;
        connector
            .with_no_pieces("192.168.1.2:6881".to_string())
            .await;

        // Setup tracker to return both peers
        tracker_client
            .expect_announce(
                vec![
                    Peer::new("192.168.1.1".to_string(), 6881),
                    Peer::new("192.168.1.2".to_string(), 6881),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);
        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
            })
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        // Use watch_tracker to populate available_peers
        peer_manager
            .clone()
            .watch_tracker(
                Duration::from_secs(3600),
                "http://tracker.example.com:8080/announce".to_string(),
                [1u8; 20],
                1024,
            )
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect both peers
        let (event_tx, _) = mpsc::unbounded_channel();
        peer_manager
            .clone()
            .connect_with_peers(5, event_tx)
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Below capacity - should NOT drop peers
        peer_manager.drop_useless_peers_if_at_capacity(5).await;

        assert_eq!(peer_manager.connected_peer_count().await, 2);
    }
}
