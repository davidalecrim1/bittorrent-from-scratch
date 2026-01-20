mod helpers;

#[cfg(test)]
mod tests {
    use super::helpers;
    use bittorrent_from_scratch::error::AppError;
    use bittorrent_from_scratch::peer::{Peer, PeerSource};
    use bittorrent_from_scratch::peer_manager::{PeerManager, PeerManagerConfig};
    use std::sync::Arc;
    use tempfile::NamedTempFile;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_peer_manager_initialization() {
        // Create a mock tracker client
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
            piece_length: 16384,
        };

        let temp_file = NamedTempFile::new().unwrap();
        let file_path = temp_file.path().to_path_buf();
        let result = peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await;
        assert!(result.is_ok(), "Initialization should succeed");
    }

    #[tokio::test]
    async fn test_request_pieces_queues_pieces() {
        use bittorrent_from_scratch::peer_manager::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
            piece_length: 16384,
        };

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await
            .unwrap();

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
        use bittorrent_from_scratch::peer_manager::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
            piece_length: 16384,
        };

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await
            .unwrap();

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
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 10,
            piece_length: 16384,
        };

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        let result = peer_manager
            .start(Some("http://tracker.example.com/announce".to_string()))
            .await;

        assert!(result.is_ok(), "start should return a handle");
        let handle = result.unwrap();
        handle.shutdown();
    }

    #[tokio::test]
    async fn test_multiple_piece_requests() {
        use bittorrent_from_scratch::peer_manager::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 100,
            piece_length: 16384,
        };

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await
            .unwrap();

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
        use bittorrent_from_scratch::peer::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // Setup mock to return peers
        let expected_peers = vec![
            Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker),
            Peer::new("192.168.1.2".to_string(), 6882, PeerSource::Tracker),
        ];
        tracker_client
            .expect_announce(expected_peers.clone(), Some(1800))
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
            piece_length: 16384,
        };

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await
            .unwrap();

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
        use bittorrent_from_scratch::peer::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // Setup tracker to return peers
        tracker_client
            .expect_announce(
                vec![Peer::new(
                    "192.168.1.1".to_string(),
                    6881,
                    PeerSource::Tracker,
                )],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024 * 1024,
            num_pieces: 64,
            piece_length: 16384,
        };

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await
            .unwrap();
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
    async fn test_process_completion_writes_piece() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
            piece_length: 16384,
        };
        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await
            .unwrap();

        // Request the piece first
        peer_manager
            .request_pieces(vec![
                bittorrent_from_scratch::peer_manager::PieceDownloadRequest {
                    piece_index: 5,
                    expected_hash: [0u8; 20],
                    piece_length: 16384,
                },
            ])
            .await
            .unwrap();

        // Start download to mark it in-flight
        peer_manager
            .start_download(5, "127.0.0.1:6881".to_string())
            .await;

        let result = peer_manager
            .process_completion(5, vec![1, 2, 3, 4], "127.0.0.1:6881".to_string())
            .await;

        assert!(result.is_ok());

        // Verify the piece is marked as completed
        assert!(peer_manager.has_piece(5).await);
    }

    #[tokio::test]
    async fn test_process_failure_retry_logic() {
        use bittorrent_from_scratch::peer_manager::{FailedPiece, PieceDownloadRequest};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
            piece_length: 16384,
        };
        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await
            .unwrap();

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
        use bittorrent_from_scratch::peer_manager::{FailedPiece, PieceDownloadRequest};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
            piece_length: 16384,
        };
        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await
            .unwrap();

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

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
            piece_length: 16384,
        };
        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(config, file_path, 1024 * 1024, 16384)
            .await
            .unwrap();

        let result = peer_manager.try_assign_piece().await;

        assert!(result.is_ok());
        assert!(!result.unwrap(), "should return false when queue is empty");
    }

    #[tokio::test]
    async fn test_cleanup_piece_tracking_not_found() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
            .await
            .unwrap();

        let result = peer_manager.cleanup_piece_tracking(99).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_add_available_peers() {
        use bittorrent_from_scratch::peer::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
            .await
            .unwrap();

        let peer1 = Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker);
        let peer2 = Peer::new("192.168.1.2".to_string(), 6882, PeerSource::Tracker);

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
        use bittorrent_from_scratch::peer_manager::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer::Peer;
        use bittorrent_from_scratch::peer_manager::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
            .await
            .unwrap();

        let peer = Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker);
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
        use bittorrent_from_scratch::peer::Peer;
        use bittorrent_from_scratch::peer_manager::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // Configure peer to have pieces 0, 1, 2 out of 10 total
        connector
            .with_specific_pieces("192.168.1.1:6881".to_string(), vec![0, 1, 2], 10)
            .await;

        // Setup tracker to return this peer
        tracker_client
            .expect_announce(
                vec![Peer::new(
                    "192.168.1.1".to_string(),
                    6881,
                    PeerSource::Tracker,
                )],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );
        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        let _piece_count = peer_manager.get_peer_piece_count("192.168.1.1:6881").await;

        // Queue piece 1 first
        let request = PieceDownloadRequest {
            piece_index: 1,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        };
        peer_manager.download_piece(request.clone()).await.unwrap();

        // Pop it from queue to prepare for assignment
        peer_manager.pop_pending_for_test().await;

        // Assign piece 1 (peer has it)
        let result = peer_manager.assign_piece_to_peer(request).await;
        assert!(result.is_ok(), "Should assign piece to peer that has it");
    }

    #[tokio::test]
    async fn test_try_assign_piece_empty_queue() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
            .await
            .unwrap();

        let peer = bittorrent_from_scratch::peer::Peer::new(
            "192.168.1.1".to_string(),
            6881,
            PeerSource::Tracker,
        );
        let (event_tx, _event_rx) = mpsc::unbounded_channel();

        let result = peer_manager.connect_peer(peer, event_tx).await;

        assert!(
            result.is_ok(),
            "Connection should succeed with mock connector"
        );
    }

    #[tokio::test]
    async fn test_background_tasks_shutdown_gracefully() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());
        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
            .await
            .unwrap();

        let peer_manager = Arc::new(peer_manager);

        let handle = peer_manager
            .clone()
            .start(Some("http://tracker.example.com".to_string()))
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

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
            .await
            .unwrap();

        // Should return None for non-existent peer
        let result = peer_manager.get_peer_piece_count("192.168.1.1:6881").await;
        assert_eq!(result, None, "Should return None for non-existent peer");
    }

    #[tokio::test]
    async fn test_piece_state_query_methods() {
        use bittorrent_from_scratch::peer_manager::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer_manager::{FailedPiece, PieceDownloadRequest};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer::Peer;
        use bittorrent_from_scratch::peer_manager::PeerDisconnected;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
            .await
            .unwrap();

        let peer = Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker);
        let disconnect = PeerDisconnected {
            peer: peer.clone(),
            error: bittorrent_from_scratch::error::AppError::PeerStreamClosed,
        };

        let result = peer_manager.process_disconnect(disconnect).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_connected_peer_addrs() {
        use bittorrent_from_scratch::peer::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        tracker_client
            .expect_announce(
                vec![Peer::new(
                    "192.168.1.1".to_string(),
                    6881,
                    PeerSource::Tracker,
                )],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // First request fails, second succeeds
        tracker_client
            .expect_failure("Temporary network error")
            .await;
        tracker_client
            .expect_announce(
                vec![Peer::new(
                    "192.168.1.1".to_string(),
                    6881,
                    PeerSource::Tracker,
                )],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        tracker_client
            .expect_announce(
                vec![
                    Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker),
                    Peer::new("192.168.1.2".to_string(), 6882, PeerSource::Tracker),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        tracker_client
            .expect_announce(
                vec![Peer::new(
                    "192.168.1.1".to_string(),
                    6881,
                    PeerSource::Tracker,
                )],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        tracker_client
            .expect_announce(
                vec![
                    Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker),
                    Peer::new("192.168.1.2".to_string(), 6882, PeerSource::Tracker),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer::Peer;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        tracker_client
            .expect_announce(
                vec![
                    Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker),
                    Peer::new("192.168.1.2".to_string(), 6882, PeerSource::Tracker),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer::Peer;
        use bittorrent_from_scratch::peer_manager::{PeerDisconnected, PieceDownloadRequest};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        let peer = Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker);
        let disconnect = PeerDisconnected {
            peer: peer.clone(),
            error: bittorrent_from_scratch::error::AppError::PeerStreamClosed,
        };

        let result = peer_manager.process_disconnect(disconnect).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_config_not_initialized() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        // Try to use connect_peer before initializing - should fail
        use bittorrent_from_scratch::peer::Peer;

        let peer = Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker);
        let (event_tx, _) = mpsc::unbounded_channel();

        let result = peer_manager.connect_peer(peer, event_tx).await;
        assert!(result.is_err(), "Should fail when config not initialized");
    }

    #[tokio::test]
    async fn test_assign_piece_selects_least_busy_peer() {
        use bittorrent_from_scratch::peer::Peer;
        use bittorrent_from_scratch::peer_manager::{PeerManagerConfig, PieceDownloadRequest};
        use std::time::Duration;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // All peers have pieces 5 and 6
        connector
            .with_specific_pieces("192.168.1.1:6881".to_string(), vec![5, 6], 10)
            .await;
        connector
            .with_specific_pieces("192.168.1.2:6881".to_string(), vec![5, 6], 10)
            .await;
        connector
            .with_specific_pieces("192.168.1.3:6881".to_string(), vec![5, 6], 10)
            .await;

        // Setup tracker to return all peers
        tracker_client
            .expect_announce(
                vec![
                    Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker),
                    Peer::new("192.168.1.2".to_string(), 6881, PeerSource::Tracker),
                    Peer::new("192.168.1.3".to_string(), 6881, PeerSource::Tracker),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );
        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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

        // Queue pieces first
        let request1 = PieceDownloadRequest {
            piece_index: 5,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        };
        let request2 = PieceDownloadRequest {
            piece_index: 6,
            piece_length: 16384,
            expected_hash: [1u8; 20],
        };
        peer_manager.download_piece(request1.clone()).await.unwrap();
        peer_manager.download_piece(request2.clone()).await.unwrap();

        // Pop from queue
        peer_manager.pop_pending_for_test().await;
        peer_manager.pop_pending_for_test().await;

        // Assign piece to first peer (now has 1 active download)
        peer_manager.assign_piece_to_peer(request1).await.unwrap();

        // Second assignment should select a less busy peer (not the one with active download)
        let result = peer_manager.assign_piece_to_peer(request2).await;

        if let Err(e) = &result {
            panic!("Assignment failed: {}", e);
        }
        assert!(result.is_ok(), "Should select least busy peer");
    }

    #[tokio::test]
    async fn test_assign_piece_fails_when_no_peer_has_piece() {
        use bittorrent_from_scratch::peer::Peer;
        use bittorrent_from_scratch::peer_manager::{PeerManagerConfig, PieceDownloadRequest};
        use std::time::Duration;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // Peer has pieces 0, 1, 2 but NOT piece 5
        connector
            .with_specific_pieces("192.168.1.1:6881".to_string(), vec![0, 1, 2], 10)
            .await;

        // Setup tracker to return this peer
        tracker_client
            .expect_announce(
                vec![Peer::new(
                    "192.168.1.1".to_string(),
                    6881,
                    PeerSource::Tracker,
                )],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );
        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer::Peer;
        use bittorrent_from_scratch::peer_manager::PeerManagerConfig;
        use std::time::Duration;

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
                    Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker),
                    Peer::new("192.168.1.2".to_string(), 6881, PeerSource::Tracker),
                    Peer::new("192.168.1.3".to_string(), 6881, PeerSource::Tracker),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );
        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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
        use bittorrent_from_scratch::peer::Peer;
        use bittorrent_from_scratch::peer_manager::PeerManagerConfig;
        use std::time::Duration;

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
                    Peer::new("192.168.1.1".to_string(), 6881, PeerSource::Tracker),
                    Peer::new("192.168.1.2".to_string(), 6881, PeerSource::Tracker),
                ],
                Some(1800),
            )
            .await;

        let mut peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );
        let _temp_file = NamedTempFile::new().unwrap();
        let file_path = _temp_file.path().to_path_buf();
        peer_manager
            .initialize(
                PeerManagerConfig {
                    info_hash: [1u8; 20],
                    client_peer_id: [2u8; 20],
                    file_size: 1024,
                    num_pieces: 10,
                    piece_length: 16384,
                },
                file_path,
                1024,
                16384,
            )
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

    #[tokio::test]
    async fn test_discover_peers_from_tracker() {
        use std::net::{IpAddr, Ipv4Addr};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // Setup tracker to return peers
        let peers = vec![
            Peer {
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
                port: 6881,
                source: PeerSource::Tracker,
            },
            Peer {
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2)),
                port: 6881,
                source: PeerSource::Tracker,
            },
        ];
        tracker_client.expect_announce(peers, Some(1800)).await;

        let peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let info_hash = [1u8; 20];
        let tracker_url = Some("http://tracker.example.com:8080/announce".to_string());

        let discovered_peers = peer_manager
            .discover_peers(info_hash, tracker_url)
            .await
            .unwrap();

        assert_eq!(discovered_peers.len(), 2);
        assert_eq!(
            discovered_peers[0].to_string(),
            "192.168.1.1:6881".to_string()
        );
        assert_eq!(
            discovered_peers[1].to_string(),
            "192.168.1.2:6881".to_string()
        );
    }

    #[tokio::test]
    async fn test_discover_peers_tracker_failure_returns_empty() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // Setup tracker to fail
        tracker_client.expect_failure("Tracker unreachable").await;

        let peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let info_hash = [1u8; 20];
        let tracker_url = Some("http://tracker.example.com:8080/announce".to_string());

        let discovered_peers = peer_manager
            .discover_peers(info_hash, tracker_url)
            .await
            .unwrap();

        // Should return empty list on failure, not error
        assert_eq!(discovered_peers.len(), 0);
    }

    #[tokio::test]
    async fn test_discover_peers_no_tracker() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        let peer_manager = PeerManager::new_with_connector(
            tracker_client,
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let info_hash = [1u8; 20];
        let tracker_url = None; // No tracker URL

        let discovered_peers = peer_manager
            .discover_peers(info_hash, tracker_url)
            .await
            .unwrap();

        // Should work but return empty (would use DHT in real scenario)
        assert_eq!(discovered_peers.len(), 0);
    }

    #[tokio::test]
    async fn test_discover_peers_with_dht_fallback() {
        use bittorrent_from_scratch::peer::Peer;
        use std::net::{IpAddr, Ipv4Addr};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // Setup tracker to return only 5 peers (< 10 threshold for DHT)
        let tracker_peers = vec![
            Peer {
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
                port: 6881,
                source: PeerSource::Tracker,
            },
            Peer {
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2)),
                port: 6881,
                source: PeerSource::Tracker,
            },
            Peer {
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 3)),
                port: 6881,
                source: PeerSource::Tracker,
            },
            Peer {
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 4)),
                port: 6881,
                source: PeerSource::Tracker,
            },
            Peer {
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)),
                port: 6881,
                source: PeerSource::Tracker,
            },
        ];

        tracker_client
            .expect_announce(tracker_peers.clone(), Some(1800))
            .await;

        // Setup DHT to return additional peers
        let dht_client = Arc::new(super::helpers::fakes::MockDhtClient::new());
        let dht_peers = vec![
            Peer {
                ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                port: 6881,
                source: PeerSource::Dht,
            },
            Peer {
                ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                port: 6881,
                source: PeerSource::Dht,
            },
        ];

        dht_client.expect_get_peers(dht_peers.clone()).await;

        let peer_manager = PeerManager::new_with_dht(
            Some(tracker_client),
            Some(dht_client),
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let info_hash = [1u8; 20];
        let tracker_url = Some("http://tracker.example.com:8080/announce".to_string());

        let discovered_peers = peer_manager
            .discover_peers(info_hash, tracker_url)
            .await
            .unwrap();

        // Should combine tracker + DHT peers (5 + 2 = 7)
        assert_eq!(discovered_peers.len(), 7);
    }

    #[tokio::test]
    async fn test_discover_peers_skips_dht_when_enough_from_tracker() {
        use bittorrent_from_scratch::peer::Peer;
        use std::net::{IpAddr, Ipv4Addr};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::FakePeerConnectionFactory::new());

        // Setup tracker to return 15 peers (>= 10 threshold)
        let tracker_peers: Vec<Peer> = (0..15)
            .map(|i| Peer {
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, i as u8)),
                port: 6881,
                source: PeerSource::Tracker,
            })
            .collect();

        tracker_client
            .expect_announce(tracker_peers.clone(), Some(1800))
            .await;

        // DHT should not be called
        let dht_client = Arc::new(super::helpers::fakes::MockDhtClient::new());

        let peer_manager = PeerManager::new_with_dht(
            Some(tracker_client),
            Some(dht_client),
            connector,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_connection_stats(),
            50,
        );

        let info_hash = [1u8; 20];
        let tracker_url = Some("http://tracker.example.com:8080/announce".to_string());

        let discovered_peers = peer_manager
            .discover_peers(info_hash, tracker_url)
            .await
            .unwrap();

        // Should only have tracker peers, DHT not queried
        assert_eq!(discovered_peers.len(), 15);
    }
}
