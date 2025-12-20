mod helpers;

#[cfg(test)]
mod tests {
    use bittorrent_from_scratch::peer_manager::PeerManager;
    use bittorrent_from_scratch::types::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_cleanup_piece_tracking() {
        use tokio::sync::mpsc;

        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());
        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
                max_peers: 5,
            })
            .await
            .unwrap();

        // Setup: add a peer with an active download
        let peer = Peer::new("192.168.1.1".to_string(), 6881);
        let peer_addr = peer.get_addr();
        let (download_request_tx, _rx) = mpsc::channel(10);
        let bitfield = Arc::new(tokio::sync::RwLock::new(Vec::new()));
        let mut connected_peer = ConnectedPeer::new(peer, download_request_tx, bitfield);
        connected_peer.active_downloads.insert(5);

        {
            let mut connected_peers = peer_manager.connected_peers.write().await;
            connected_peers.insert(peer_addr.clone(), connected_peer);
        }

        {
            let mut in_flight = peer_manager.in_flight_pieces.write().await;
            in_flight.insert(5, peer_addr.clone());
        }

        // Test cleanup
        let result = peer_manager.cleanup_piece_tracking(5).await;
        assert_eq!(result, Some(peer_addr.clone()));

        // Verify it was removed from in_flight
        {
            let in_flight = peer_manager.in_flight_pieces.read().await;
            assert!(!in_flight.contains_key(&5));
        }

        // Verify it was removed from peer's active downloads
        {
            let connected_peers = peer_manager.connected_peers.read().await;
            let peer = connected_peers.get(&peer_addr).unwrap();
            assert!(!peer.active_downloads.contains(&5));
        }
    }

    #[tokio::test]
    async fn test_cleanup_piece_tracking_not_found() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());
        let mut peer_manager = PeerManager::new_with_connector(tracker_client, connector);

        peer_manager
            .initialize(PeerManagerConfig {
                info_hash: [1u8; 20],
                client_peer_id: [2u8; 20],
                file_size: 1024,
                num_pieces: 10,
                max_peers: 5,
            })
            .await
            .unwrap();

        let result = peer_manager.cleanup_piece_tracking(99).await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_get_peers_success() {
        let tracker_client = Arc::new(super::helpers::fakes::MockTrackerClient::new());
        let connector = Arc::new(super::helpers::fakes::MockPeerConnector::new());

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
                max_peers: 5,
            })
            .await
            .unwrap();

        let result = peer_manager
            .get_peers("http://tracker.example.com".to_string(), [1u8; 20], 1024)
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
    }
}
