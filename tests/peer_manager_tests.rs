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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

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

        // Manually populate available_peers since get_peers doesn't do it
        {
            let mut available = peer_manager.available_peers.write().await;
            for peer in peers {
                available.insert(peer.get_addr(), peer);
            }
        }

        let peer_manager = Arc::new(peer_manager);

        // Try connecting with peers
        let (completion_tx, _completion_rx) = mpsc::unbounded_channel();
        let (failure_tx, _failure_rx) = mpsc::unbounded_channel();
        let (disconnect_tx, _disconnect_rx) = mpsc::unbounded_channel();

        let result = peer_manager
            .connect_with_peers(5, completion_tx, failure_tx, disconnect_tx)
            .await;

        assert!(result.is_ok(), "connect_with_peers should succeed");
        assert_eq!(result.unwrap(), 1, "should start 1 connection attempt");

        // Wait for the spawned connection tasks to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Verify the peer was added to connected_peers
        let connected_peers = peer_manager.connected_peers.read().await;
        assert_eq!(connected_peers.len(), 1, "should have 1 connected peer");
        assert!(
            connected_peers.contains_key("192.168.1.1:6881"),
            "should contain the connected peer"
        );
    }

    #[tokio::test]
    async fn test_process_completion_forwards_to_writer() {
        use bittorrent_from_scratch::types::CompletedPiece;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
        };
        peer_manager.initialize(config).await.unwrap();

        let (file_writer_tx, mut file_writer_rx) = mpsc::unbounded_channel();
        let (download_complete_tx, _) = mpsc::channel(1);

        let completed = CompletedPiece {
            peer_addr: "127.0.0.1:6881".to_string(),
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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
        };
        peer_manager.initialize(config).await.unwrap();

        // Simulate 9 pieces already completed
        {
            let mut completed_count = peer_manager.completed_pieces.write().await;
            *completed_count = 9;
        }

        let (file_writer_tx, _) = mpsc::unbounded_channel();
        let (download_complete_tx, mut download_complete_rx) = mpsc::channel(1);

        let completed = CompletedPiece {
            peer_addr: "127.0.0.1:6881".to_string(),
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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
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
            peer_addr: "127.0.0.1:6881".to_string(),
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
    async fn test_process_failure_unlimited_retries() {
        use bittorrent_from_scratch::types::{FailedPiece, PieceDownloadRequest};

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
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

        // Simulate many failed attempts already (e.g., 100)
        {
            let mut failed_attempts = peer_manager.failed_attempts.write().await;
            failed_attempts.insert(5, 100);
        }

        let failed = FailedPiece {
            peer_addr: "127.0.0.1:6881".to_string(),
            piece_index: 5,
            reason: "hash_mismatch".to_string(),
        };

        let result = peer_manager.process_failure(failed).await;

        assert!(result.is_ok());
        assert!(
            result.unwrap(),
            "should always requeue with unlimited retries"
        );

        // Verify attempts counter incremented
        {
            let failed_attempts = peer_manager.failed_attempts.read().await;
            assert_eq!(*failed_attempts.get(&5).unwrap(), 101);
        }
    }

    #[tokio::test]
    async fn test_process_disconnect_requeues_in_flight_pieces() {
        use bittorrent_from_scratch::types::{ConnectedPeer, Peer, PeerDisconnected};
        use std::collections::HashSet;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        let config = PeerManagerConfig {
            info_hash: [1u8; 20],
            client_peer_id: [2u8; 20],
            file_size: 1024,
            num_pieces: 10,
        };
        peer_manager.initialize(config).await.unwrap();

        // Setup a connected peer with active downloads
        let peer = Peer::new("192.168.1.1".to_string(), 6881);
        let peer_addr = peer.get_addr();
        let (download_request_tx, _rx) = mpsc::channel(10);
        let bitfield = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let mut connected_peer = ConnectedPeer::new(peer.clone(), download_request_tx, bitfield);

        // Mark pieces 1, 2, 3 as active downloads for this peer
        connected_peer.mark_downloading(1);
        connected_peer.mark_downloading(2);
        connected_peer.mark_downloading(3);

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

        let (failure_tx, mut failure_rx) = mpsc::unbounded_channel();

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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());

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
    async fn test_cleanup_piece_tracking() {
        use bittorrent_from_scratch::types::ConnectedPeer;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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
        let peer_addr = peer.get_addr();
        let (download_request_tx, _rx) = mpsc::channel(10);
        let bitfield = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let mut connected_peer = ConnectedPeer::new(peer, download_request_tx, bitfield);
        connected_peer.mark_downloading(5);

        {
            let mut connected_peers = peer_manager.connected_peers.write().await;
            connected_peers.insert(peer_addr.clone(), connected_peer);
        }

        {
            let mut in_flight = peer_manager.in_flight_pieces.write().await;
            in_flight.insert(5, peer_addr.clone());
        }

        let result = peer_manager.cleanup_piece_tracking(5).await;
        assert_eq!(result, Some(peer_addr.clone()));

        {
            let in_flight = peer_manager.in_flight_pieces.read().await;
            assert!(!in_flight.contains_key(&5));
        }

        {
            let connected_peers = peer_manager.connected_peers.read().await;
            let peer = connected_peers.get(&peer_addr).unwrap();
            assert!(!peer.is_downloading(5));
        }
    }

    #[tokio::test]
    async fn test_cleanup_piece_tracking_not_found() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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
    async fn test_assign_piece_to_peer_selects_least_busy() {
        use bittorrent_from_scratch::types::{ConnectedPeer, PieceDownloadRequest};
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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

        let peer1 = bittorrent_from_scratch::types::Peer::new("192.168.1.1".to_string(), 6881);
        let peer2 = bittorrent_from_scratch::types::Peer::new("192.168.1.2".to_string(), 6882);
        let peer1_addr = peer1.get_addr();
        let peer2_addr = peer2.get_addr();

        let (tx1, _rx1) = mpsc::channel(10);
        let (tx2, _rx2) = mpsc::channel(10);

        let mut bitfield1 = vec![false; 10];
        bitfield1[5] = true;
        let mut bitfield2 = vec![false; 10];
        bitfield2[5] = true;

        let bitfield1_arc = Arc::new(tokio::sync::RwLock::new(bitfield1));
        let bitfield2_arc = Arc::new(tokio::sync::RwLock::new(bitfield2));

        let mut connected_peer1 = ConnectedPeer::new(peer1, tx1, bitfield1_arc);
        let connected_peer2 = ConnectedPeer::new(peer2, tx2, bitfield2_arc);

        connected_peer1.mark_downloading(1);
        connected_peer1.mark_downloading(2);

        {
            let mut connected_peers = peer_manager.connected_peers.write().await;
            connected_peers.insert(peer1_addr.clone(), connected_peer1);
            connected_peers.insert(peer2_addr.clone(), connected_peer2);
        }

        let request = PieceDownloadRequest {
            piece_index: 5,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        };

        let result = peer_manager.assign_piece_to_peer(request).await;
        assert!(result.is_ok());

        {
            let in_flight = peer_manager.in_flight_pieces.read().await;
            assert_eq!(in_flight.get(&5), Some(&peer2_addr));
        }

        {
            let connected_peers = peer_manager.connected_peers.read().await;
            let peer2 = connected_peers.get(&peer2_addr).unwrap();
            assert!(peer2.is_downloading(5));
        }
    }

    #[tokio::test]
    async fn test_assign_piece_to_peer_no_peers_available() {
        use bittorrent_from_scratch::types::PieceDownloadRequest;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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
        use bittorrent_from_scratch::types::{ConnectedPeer, PieceDownloadRequest};
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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
        let peer_addr = peer.get_addr();
        let (tx, _rx) = mpsc::channel(10);

        let mut bitfield = vec![false; 10];
        bitfield[5] = true;
        let bitfield_arc = Arc::new(tokio::sync::RwLock::new(bitfield));

        let mut connected_peer = ConnectedPeer::new(peer, tx, bitfield_arc);
        connected_peer.mark_downloading(5);

        {
            let mut connected_peers = peer_manager.connected_peers.write().await;
            connected_peers.insert(peer_addr.clone(), connected_peer);
        }

        let request = PieceDownloadRequest {
            piece_index: 5,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        };

        let result = peer_manager.assign_piece_to_peer(request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_try_assign_piece_requeues_on_failure() {
        use bittorrent_from_scratch::types::PieceDownloadRequest;
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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

        {
            let mut pending = peer_manager.pending_pieces.write().await;
            pending.push_back(PieceDownloadRequest {
                piece_index: 5,
                piece_length: 16384,
                expected_hash: [0u8; 20],
            });
        }

        let result = peer_manager.try_assign_piece().await;
        assert!(result.is_err(), "Should fail when no peers available");

        {
            let pending = peer_manager.pending_pieces.read().await;
            assert_eq!(pending.len(), 1, "Piece should be re-queued after failure");
        }
    }

    #[tokio::test]
    async fn test_try_assign_piece_empty_queue() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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
        let (completion_tx, _rx1) = mpsc::unbounded_channel();
        let (failure_tx, _rx2) = mpsc::unbounded_channel();
        let (disconnect_tx, _rx3) = mpsc::unbounded_channel();

        let result = peer_manager
            .connect_peer(peer, completion_tx, failure_tx, disconnect_tx)
            .await;

        assert!(
            result.is_ok(),
            "Connection should succeed with mock connector"
        );
    }

    #[tokio::test]
    async fn test_process_completion_with_cleanup() {
        use bittorrent_from_scratch::types::{CompletedPiece, ConnectedPeer};
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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
        let peer_addr = peer.get_addr();
        let (download_request_tx, _rx) = mpsc::channel(10);
        let bitfield = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let mut connected_peer = ConnectedPeer::new(peer, download_request_tx, bitfield);
        connected_peer.mark_downloading(5);

        {
            let mut connected_peers = peer_manager.connected_peers.write().await;
            connected_peers.insert(peer_addr.clone(), connected_peer);
        }

        {
            let mut in_flight = peer_manager.in_flight_pieces.write().await;
            in_flight.insert(5, peer_addr.clone());
        }

        let (file_writer_tx, mut file_writer_rx) = mpsc::unbounded_channel();
        let (download_complete_tx, _) = mpsc::channel(1);

        let completed = CompletedPiece {
            peer_addr: "127.0.0.1:6881".to_string(),
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

        assert!(result, "Should continue processing");

        let forwarded = file_writer_rx.recv().await;
        assert!(forwarded.is_some());
        assert_eq!(forwarded.unwrap().piece_index, 5);

        {
            let in_flight = peer_manager.in_flight_pieces.read().await;
            assert!(!in_flight.contains_key(&5));
        }

        {
            let completed_count = peer_manager.completed_pieces.read().await;
            assert_eq!(*completed_count, 1);
        }
    }

    #[tokio::test]
    async fn test_process_failure_missing_request() {
        use bittorrent_from_scratch::types::FailedPiece;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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

        let failed = FailedPiece {
            peer_addr: "127.0.0.1:6881".to_string(),
            piece_index: 99,
            reason: "Test failure".to_string(),
        };

        let result = peer_manager.process_failure(failed).await;
        assert!(
            result.is_err(),
            "Should fail when original request not found"
        );
    }

    #[tokio::test]
    async fn test_process_disconnect_with_multiple_active_downloads() {
        use bittorrent_from_scratch::types::{ConnectedPeer, PeerDisconnected};
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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
        let peer_addr = peer.get_addr();
        let (download_request_tx, _rx) = mpsc::channel(10);
        let bitfield = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let mut connected_peer = ConnectedPeer::new(peer.clone(), download_request_tx, bitfield);

        connected_peer.mark_downloading(3);
        connected_peer.mark_downloading(5);
        connected_peer.mark_downloading(7);

        {
            let mut connected_peers = peer_manager.connected_peers.write().await;
            connected_peers.insert(peer_addr.clone(), connected_peer);
        }

        {
            let mut in_flight = peer_manager.in_flight_pieces.write().await;
            in_flight.insert(3, peer_addr.clone());
            in_flight.insert(5, peer_addr.clone());
            in_flight.insert(7, peer_addr.clone());
        }

        let (failure_tx, mut failure_rx) = mpsc::unbounded_channel();
        let disconnect = PeerDisconnected {
            peer,
            reason: "Connection lost".to_string(),
        };

        let result = peer_manager
            .process_disconnect(disconnect, &failure_tx)
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 3, "Should have re-queued 3 pieces");

        {
            let connected_peers = peer_manager.connected_peers.read().await;
            assert!(!connected_peers.contains_key(&peer_addr));
        }

        {
            let in_flight = peer_manager.in_flight_pieces.read().await;
            assert!(!in_flight.contains_key(&3));
            assert!(!in_flight.contains_key(&5));
            assert!(!in_flight.contains_key(&7));
        }

        let mut failures_received = 0;
        while let Ok(Some(_)) =
            tokio::time::timeout(tokio::time::Duration::from_millis(50), failure_rx.recv()).await
        {
            failures_received += 1;
        }
        assert_eq!(failures_received, 3, "Should have sent 3 failure events");
    }

    #[tokio::test]
    async fn test_process_disconnect_with_no_active_downloads() {
        use bittorrent_from_scratch::types::{ConnectedPeer, PeerDisconnected};
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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
        let peer_addr = peer.get_addr();
        let (download_request_tx, _rx) = mpsc::channel(10);
        let bitfield = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let connected_peer = ConnectedPeer::new(peer.clone(), download_request_tx, bitfield);

        {
            let mut connected_peers = peer_manager.connected_peers.write().await;
            connected_peers.insert(peer_addr.clone(), connected_peer);
        }

        let (failure_tx, mut failure_rx) = mpsc::unbounded_channel();
        let disconnect = PeerDisconnected {
            peer,
            reason: "Connection lost".to_string(),
        };

        let result = peer_manager
            .process_disconnect(disconnect, &failure_tx)
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0, "Should have re-queued 0 pieces");

        let failures_received =
            tokio::time::timeout(tokio::time::Duration::from_millis(50), failure_rx.recv()).await;
        assert!(
            failures_received.is_err(),
            "Should not have sent any failure events"
        );
    }

    #[tokio::test]
    async fn test_background_tasks_shutdown_gracefully() {
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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

        let (piece_completion_tx, _) = mpsc::unbounded_channel();
        let (download_complete_tx, _) = mpsc::channel(1);

        let peer_manager = Arc::new(peer_manager);

        let handle = peer_manager
            .clone()
            .start(
                "http://tracker.example.com".to_string(),
                piece_completion_tx,
                download_complete_tx,
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        handle.shutdown();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    #[tokio::test]
    async fn test_piece_assignment_loop_processes_queue() {
        use bittorrent_from_scratch::types::{ConnectedPeer, PieceDownloadRequest};
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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
        let peer_addr = peer.get_addr();
        let (download_request_tx, _rx) = mpsc::channel(10);

        let mut bitfield = vec![false; 10];
        bitfield[5] = true;
        let bitfield_arc = Arc::new(tokio::sync::RwLock::new(bitfield));

        let connected_peer = ConnectedPeer::new(peer, download_request_tx, bitfield_arc);

        {
            let mut connected_peers = peer_manager.connected_peers.write().await;
            connected_peers.insert(peer_addr.clone(), connected_peer);
        }

        {
            let mut pending = peer_manager.pending_pieces.write().await;
            pending.push_back(PieceDownloadRequest {
                piece_index: 5,
                piece_length: 16384,
                expected_hash: [0u8; 20],
            });
        }

        let result = peer_manager.try_assign_piece().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), true, "Should successfully assign piece");

        {
            let in_flight = peer_manager.in_flight_pieces.read().await;
            assert!(in_flight.contains_key(&5), "Piece should be in flight");
        }

        {
            let pending = peer_manager.pending_pieces.read().await;
            assert_eq!(pending.len(), 0, "Queue should be empty after assignment");
        }
    }

    #[tokio::test]
    async fn test_completed_piece_not_requeued_on_late_failure() {
        use bittorrent_from_scratch::types::{
            CompletedPiece, ConnectedPeer, FailedPiece, PieceDownloadRequest,
        };
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnectionFactory::new());
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

        // Add piece request
        let piece_request = PieceDownloadRequest {
            piece_index: 5,
            piece_length: 1024,
            expected_hash: [0u8; 20],
        };

        peer_manager
            .request_pieces(vec![piece_request])
            .await
            .unwrap();

        // Setup a connected peer
        let peer = bittorrent_from_scratch::types::Peer::new("192.168.1.1".to_string(), 6881);
        let peer_addr = peer.get_addr();
        let (download_request_tx, _rx) = mpsc::channel(10);
        let mut bitfield = vec![true; 10];
        let bitfield_arc = Arc::new(tokio::sync::RwLock::new(bitfield));
        let mut connected_peer = ConnectedPeer::new(peer, download_request_tx, bitfield_arc);
        connected_peer.mark_downloading(5);

        {
            let mut connected_peers = peer_manager.connected_peers.write().await;
            connected_peers.insert(peer_addr.clone(), connected_peer);
        }

        {
            let mut in_flight = peer_manager.in_flight_pieces.write().await;
            in_flight.insert(5, peer_addr.clone());
        }

        // Process completion for piece 5
        let (file_writer_tx, _file_writer_rx) = mpsc::unbounded_channel();
        let (download_complete_tx, _download_complete_rx) = mpsc::channel(1);

        peer_manager
            .process_completion(
                CompletedPiece {
                    peer_addr: "127.0.0.1:6881".to_string(),
                    piece_index: 5,
                    data: vec![0u8; 1024],
                },
                &file_writer_tx,
                &download_complete_tx,
                10,
            )
            .await;

        // Verify piece 5 is completed
        {
            let completed = *peer_manager.completed_pieces.read().await;
            assert_eq!(completed, 1);
        }

        // Verify it's no longer in pending or in-flight
        {
            let pending = peer_manager.pending_pieces.read().await;
            assert_eq!(
                pending.len(),
                0,
                "No pieces should be pending after completion"
            );
        }

        {
            let in_flight = peer_manager.in_flight_pieces.read().await;
            assert!(
                !in_flight.contains_key(&5),
                "Piece 5 should not be in-flight after completion"
            );
        }

        // Now simulate a late failure for piece 5 (race condition)
        peer_manager
            .process_failure(FailedPiece {
                peer_addr: "127.0.0.1:6881".to_string(),
                piece_index: 5,
                reason: "late_failure".to_string(),
            })
            .await
            .unwrap();

        // BUG: Piece 5 gets re-queued even though it's completed!
        {
            let pending = peer_manager.pending_pieces.read().await;
            assert_eq!(
                pending.len(),
                0,
                "Completed piece should NOT be re-queued on late failure"
            );
        }

        // Also verify completed count didn't change
        {
            let completed = *peer_manager.completed_pieces.read().await;
            assert_eq!(completed, 1, "Completed count should remain 1");
        }
    }
}
