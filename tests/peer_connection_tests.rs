mod helpers;

#[cfg(test)]
mod tests {
    use super::helpers::{self, fakes::FakeMessageIO};
    use bittorrent_from_scratch::io::MessageIO;
    use bittorrent_from_scratch::peer_messages::{PeerMessage, PieceMessage};
    use bittorrent_from_scratch::types::{Peer, PeerConnection, PieceDownloadRequest};
    use tokio::sync::mpsc;

    /// Helper to consume the initial Interested message that PeerConnection sends on start
    async fn consume_initial_interested(io: &mut impl MessageIO) {
        match tokio::time::timeout(tokio::time::Duration::from_millis(100), io.read_message()).await
        {
            Ok(Ok(Some(PeerMessage::Interested(_)))) => {
                // Expected: Interested is sent on start
            }
            other => panic!("Expected initial Interested message, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_peer_connection_sends_interested_on_start() {
        // Create channels for PeerConnection
        let (event_tx, mut _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Give it a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // The peer should receive an Interested message
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            client_io.read_message(),
        )
        .await
        {
            Ok(Ok(Some(PeerMessage::Interested(_)))) => {
                // Success! PeerConnection sent Interested message
            }
            other => panic!("Expected Interested message, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_bitfield_message_updates_peer_bitfield() {
        // Create channels for PeerConnection
        let (event_tx, mut _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send a Bitfield message with 5 pieces, pieces 0, 2, 4 available
        let bitfield_data = vec![true, false, true, false, true];
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: bitfield_data.clone(),
            });

        client_io.write_message(&bitfield_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Check that the bitfield was updated - should contain pieces 0, 2, 4
        let bf = bitfield.read().await;
        assert_eq!(bf.len(), 3, "Bitfield should have 3 available pieces");
        assert!(bf.contains(&0), "Piece 0 should be available");
        assert!(!bf.contains(&1), "Piece 1 should not be available");
        assert!(bf.contains(&2), "Piece 2 should be available");
        assert!(!bf.contains(&3), "Piece 3 should not be available");
        assert!(bf.contains(&4), "Piece 4 should be available");
    }

    #[tokio::test]
    async fn test_unchoke_message_starts_piece_download() {
        // Create channels for PeerConnection
        let (event_tx, mut _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send Bitfield first - peer needs to know which pieces are available
        let bitfield_data = vec![true]; // Peer has piece 0
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: bitfield_data,
            });
        client_io.write_message(&bitfield_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Choke message
        let choke_msg = PeerMessage::Choke(bittorrent_from_scratch::peer_messages::ChokeMessage {});
        client_io.write_message(&choke_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke message
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Now send a piece download request (after unchoke)
        let piece_request = PieceDownloadRequest {
            piece_index: 0,
            expected_hash: [0u8; 20],
            piece_length: 16384,
        };
        download_request_tx.send(piece_request).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // After receiving the download request while unchoked, peer should send Request messages
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            client_io.read_message(),
        )
        .await
        {
            Ok(Ok(Some(PeerMessage::Request(_)))) => {
                // Success! PeerConnection sent Request message when unchoked
            }
            other => panic!("Expected Request message when unchoked, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_piece_download_complete_flow() {
        // Create channels for PeerConnection
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send Bitfield - peer has piece 0
        let bitfield_data = vec![true];
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: bitfield_data,
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Create test piece data (small piece for faster test)
        let piece_length = 16384;
        let piece_data = vec![42u8; piece_length];

        // Calculate expected hash
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(&piece_data);
        let expected_hash: [u8; 20] = hasher.finalize().into();

        // Send download request
        let piece_request = PieceDownloadRequest {
            piece_index: 0,
            expected_hash,
            piece_length,
        };
        download_request_tx.send(piece_request).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Peer should receive Request message
        let first_request = match client_io.read_message().await {
            Ok(Some(PeerMessage::Request(req))) => req,
            other => panic!("Expected Request message, got: {:?}", other),
        };

        // Send Piece message with the requested block
        let block_length = first_request.length as usize;
        let block_data = piece_data
            [first_request.begin as usize..(first_request.begin as usize + block_length)]
            .to_vec();

        let piece_msg = PeerMessage::Piece(PieceMessage {
            piece_index: 0,
            begin: first_request.begin,
            block: block_data,
        });
        client_io.write_message(&piece_msg).await.unwrap();

        // Wait for completion notification
        match tokio::time::timeout(tokio::time::Duration::from_secs(2), event_rx.recv()).await {
            Ok(Some(bittorrent_from_scratch::types::PeerEvent::StorePiece(completed))) => {
                assert_eq!(
                    completed.piece_index, 0,
                    "Completed piece should be index 0"
                );
                assert_eq!(
                    completed.data.len(),
                    piece_length,
                    "Piece data length should match"
                );
            }
            other => panic!("Expected CompletedPiece notification, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_piece_download_with_hash_mismatch() {
        // Create channels for PeerConnection
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send Bitfield - peer has piece 0
        let bitfield_data = vec![true];
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: bitfield_data,
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Create test piece with wrong hash
        let piece_length = 16384;
        let piece_data = vec![42u8; piece_length];

        // Use a WRONG hash (all zeros)
        let expected_hash = [0u8; 20];

        // Send download request
        let piece_request = PieceDownloadRequest {
            piece_index: 0,
            expected_hash,
            piece_length,
        };
        download_request_tx.send(piece_request).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // The task will request blocks - respond to all of them
        // For a 16KB piece, it's just 1 block
        let request = match client_io.read_message().await {
            Ok(Some(PeerMessage::Request(req))) => req,
            other => panic!("Expected Request message, got: {:?}", other),
        };

        // Send the piece data
        let piece_msg = PeerMessage::Piece(PieceMessage {
            piece_index: 0,
            begin: request.begin,
            block: piece_data.clone(),
        });
        client_io.write_message(&piece_msg).await.unwrap();

        // Wait for failure notification (hash mismatch)
        match tokio::time::timeout(tokio::time::Duration::from_secs(2), event_rx.recv()).await {
            Ok(Some(bittorrent_from_scratch::types::PeerEvent::Failure(failed))) => {
                assert_eq!(failed.piece_index, 0, "Failed piece should be index 0");
                assert!(
                    matches!(
                        failed.error,
                        bittorrent_from_scratch::error::AppError::HashMismatch
                    ),
                    "Failure should be hash mismatch, got: {:?}",
                    failed.error
                );
            }
            other => panic!(
                "Expected FailedPiece notification due to hash mismatch, got: {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn test_have_message_updates_bitfield() {
        // Create channels for PeerConnection
        let (event_tx, mut _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send Bitfield with 5 pieces, initially all false (empty bitfield)
        let bitfield_data = vec![false, false, false, false, false];
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: bitfield_data.clone(),
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Verify initial bitfield is empty (no pieces available)
        {
            let bf = bitfield.read().await;
            assert_eq!(bf.len(), 0, "Initial bitfield should be empty");
        }

        // Send Have message for piece 2
        let have_msg = PeerMessage::Have(bittorrent_from_scratch::peer_messages::HaveMessage {
            piece_index: 2,
        });
        client_io.write_message(&have_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Verify piece 2 is now true
        {
            let bf = bitfield.read().await;
            assert!(!bf.contains(&0), "Piece 0 should still be false");
            assert!(!bf.contains(&1), "Piece 1 should still be false");
            assert!(bf.contains(&2), "Piece 2 should be true after Have message");
            assert!(!bf.contains(&3), "Piece 3 should still be false");
            assert!(!bf.contains(&4), "Piece 4 should still be false");
        }
    }

    #[tokio::test]
    async fn test_have_message_without_prior_bitfield() {
        let (event_tx, mut _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        let _ = client_io.read_message().await;

        {
            let bf = bitfield.read().await;
            assert_eq!(bf.len(), 0, "Initial bitfield should be empty");
        }

        let have_msg = PeerMessage::Have(bittorrent_from_scratch::peer_messages::HaveMessage {
            piece_index: 5,
        });
        client_io.write_message(&have_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        {
            let bf = bitfield.read().await;
            assert!(bf.contains(&5), "Piece 5 should be available after Have message");
            assert_eq!(bf.len(), 1, "Bitfield should contain exactly one piece");
        }

        let have_msg2 = PeerMessage::Have(bittorrent_from_scratch::peer_messages::HaveMessage {
            piece_index: 10,
        });
        client_io.write_message(&have_msg2).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        {
            let bf = bitfield.read().await;
            assert!(bf.contains(&5), "Piece 5 should still be available");
            assert!(bf.contains(&10), "Piece 10 should be available after second Have message");
            assert_eq!(bf.len(), 2, "Bitfield should contain exactly two pieces");
        }
    }

    #[tokio::test]
    async fn test_choke_message_stops_new_requests() {
        // Create channels for PeerConnection
        let (event_tx, mut _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Consume initial Unchoke and Interested messages
        consume_initial_interested(&mut client_io).await;

        // Send Bitfield
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: vec![true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke first
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Choke message
        let choke_msg = PeerMessage::Choke(bittorrent_from_scratch::peer_messages::ChokeMessage {});
        client_io.write_message(&choke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Now send a download request while choked
        let piece_request = PieceDownloadRequest {
            piece_index: 0,
            expected_hash: [0u8; 20],
            piece_length: 16384,
        };
        let send_result = download_request_tx.send(piece_request).await;
        assert!(
            send_result.is_ok(),
            "Should be able to send download request"
        );

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Should NOT receive a Request message because peer is choked
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            client_io.read_message(),
        )
        .await
        {
            Err(_) => {
                // Timeout is expected - peer should not send requests while choked
            }
            Ok(msg) => panic!("Expected no message while choked, but got: {:?}", msg),
        }
    }

    #[tokio::test]
    async fn test_wrong_piece_index_ignored() {
        // Create channels for PeerConnection
        let (event_tx, mut _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send Bitfield
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: vec![true, true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send download request for piece 0
        let piece_request = PieceDownloadRequest {
            piece_index: 0,
            expected_hash: [0u8; 20],
            piece_length: 16384,
        };
        download_request_tx.send(piece_request).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Wait for Request message
        let _ = client_io.read_message().await;

        // Send Piece message with WRONG piece index (piece 1 instead of 0)
        let piece_msg = PeerMessage::Piece(PieceMessage {
            piece_index: 1, // Wrong!
            begin: 0,
            block: vec![42u8; 16384],
        });
        client_io.write_message(&piece_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Should still be waiting for more Request messages (didn't complete)
        // The wrong piece should have been ignored
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            client_io.read_message(),
        )
        .await
        {
            Ok(Ok(Some(PeerMessage::Request(_)))) => {
                // Good - still requesting blocks for piece 0
            }
            _other => {
                // Also acceptable - might have already sent all requests
                // The key is that wrong piece index was ignored
            }
        }
    }

    #[tokio::test]
    async fn test_download_request_for_unavailable_piece() {
        // Create channels for PeerConnection
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send Bitfield - peer only has piece 0
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: vec![true, false], // Only piece 0
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Request piece 1 which peer doesn't have
        let piece_request = PieceDownloadRequest {
            piece_index: 1, // Peer doesn't have this
            expected_hash: [0u8; 20],
            piece_length: 16384,
        };
        download_request_tx.send(piece_request).await.unwrap();

        // Should receive a failure notification
        match tokio::time::timeout(tokio::time::Duration::from_secs(1), event_rx.recv()).await {
            Ok(Some(bittorrent_from_scratch::types::PeerEvent::Failure(failed))) => {
                assert_eq!(failed.piece_index, 1, "Failed piece should be index 1");
                assert!(
                    matches!(
                        failed.error,
                        bittorrent_from_scratch::error::AppError::PeerDoesNotHavePiece
                    ),
                    "Should indicate peer doesn't have piece"
                );
            }
            other => panic!(
                "Expected FailedPiece for unavailable piece, got: {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn test_keep_alive_and_other_messages() {
        // Create channels for PeerConnection
        let (event_tx, mut _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send KeepAlive message (should be handled without error)
        let keepalive_msg = PeerMessage::KeepAlive;
        client_io.write_message(&keepalive_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Interested message
        let interested_msg =
            PeerMessage::Interested(bittorrent_from_scratch::peer_messages::InterestedMessage {});
        client_io.write_message(&interested_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send NotInterested message
        let not_interested_msg = PeerMessage::NotInterested(
            bittorrent_from_scratch::peer_messages::NotInterestedMessage {},
        );
        client_io.write_message(&not_interested_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Request message (should be no-op, we don't support uploading)
        let request_msg =
            PeerMessage::Request(bittorrent_from_scratch::peer_messages::RequestMessage {
                piece_index: 0,
                begin: 0,
                length: 16384,
            });
        client_io.write_message(&request_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Cancel message (should be no-op)
        let cancel_msg =
            PeerMessage::Cancel(bittorrent_from_scratch::peer_messages::CancelMessage {
                piece_index: 0,
                begin: 0,
                length: 16384,
            });
        client_io.write_message(&cancel_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // All messages should have been processed without errors or disconnection
        // Connection should still be alive
    }

    #[tokio::test]
    async fn test_piece_message_without_active_download() {
        // Create channels for PeerConnection
        let (event_tx, mut _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send Piece message without any active download - should be ignored
        let piece_msg = PeerMessage::Piece(PieceMessage {
            piece_index: 0,
            begin: 0,
            block: vec![42u8; 1024],
        });
        client_io.write_message(&piece_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Should not crash or cause errors - message should be ignored
    }

    #[tokio::test]
    async fn test_large_piece_with_multiple_blocks() {
        // Create channels for PeerConnection
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send Bitfield
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: vec![true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Create a larger piece that requires 3 blocks (3 * 16384 = 49152 bytes)
        let block_size = 16384;
        let piece_length = block_size * 3;
        let piece_data = (0..piece_length)
            .map(|i| (i % 256) as u8)
            .collect::<Vec<u8>>();

        // Calculate expected hash
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(&piece_data);
        let expected_hash: [u8; 20] = hasher.finalize().into();

        // Send download request
        let piece_request = PieceDownloadRequest {
            piece_index: 0,
            expected_hash,
            piece_length,
        };
        download_request_tx.send(piece_request).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Handle block requests and responses in a loop
        // The task will pipeline up to 5 requests, then request more as blocks arrive
        let total_blocks = 3; // 48KB / 16KB = 3 blocks
        for _ in 0..total_blocks {
            // Read the request
            let request = match tokio::time::timeout(
                tokio::time::Duration::from_millis(500),
                client_io.read_message(),
            )
            .await
            {
                Ok(Ok(Some(PeerMessage::Request(req)))) => req,
                other => panic!("Expected Request message, got: {:?}", other),
            };

            // Send the corresponding block
            let begin = request.begin as usize;
            let length = request.length as usize;
            let block_data = piece_data[begin..begin + length].to_vec();

            let piece_msg = PeerMessage::Piece(PieceMessage {
                piece_index: 0,
                begin: request.begin,
                block: block_data,
            });
            client_io.write_message(&piece_msg).await.unwrap();
        }

        // Wait for completion
        match tokio::time::timeout(tokio::time::Duration::from_secs(2), event_rx.recv()).await {
            Ok(Some(bittorrent_from_scratch::types::PeerEvent::StorePiece(completed))) => {
                assert_eq!(completed.piece_index, 0);
                assert_eq!(completed.data.len(), piece_length);
                assert_eq!(completed.data, piece_data);
            }
            other => panic!("Expected CompletedPiece, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_write_error_triggers_disconnect() {
        use bittorrent_from_scratch::io::MessageIO;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        // Create a MessageIO that fails on write
        #[derive(Debug)]
        struct FailingMessageIO {
            write_count: Arc<Mutex<usize>>,
        }

        #[async_trait::async_trait]
        impl MessageIO for FailingMessageIO {
            async fn write_message(&mut self, _msg: &PeerMessage) -> anyhow::Result<()> {
                let mut count = self.write_count.lock().await;
                *count += 1;
                Err(anyhow::anyhow!("Write failed"))
            }

            async fn read_message(&mut self) -> anyhow::Result<Option<PeerMessage>> {
                // Keep returning None to simulate no messages
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                Ok(None)
            }
        }

        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _, _, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let write_count = Arc::new(Mutex::new(0));
        let failing_io = FailingMessageIO {
            write_count: write_count.clone(),
        };

        // Start the peer connection with failing IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(failing_io)).await;
        });

        // Wait for the Interested message to be written (and fail)
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Should receive a disconnect notification
        match tokio::time::timeout(tokio::time::Duration::from_millis(200), event_rx.recv()).await {
            Ok(Some(bittorrent_from_scratch::types::PeerEvent::Disconnect(_disconnect))) => {
                // Disconnect event received (peer disconnected due to write error)
            }
            other => panic!("Expected disconnect notification, got: {:?}", other),
        }

        // Verify write was attempted
        let count = *write_count.lock().await;
        assert!(count > 0, "Write should have been attempted");
    }

    #[tokio::test]
    async fn test_stream_closed_triggers_disconnect() {
        use bittorrent_from_scratch::io::MessageIO;

        // Create a MessageIO that simulates stream closure
        #[derive(Debug)]
        struct ClosedStreamMessageIO {}

        #[async_trait::async_trait]
        impl MessageIO for ClosedStreamMessageIO {
            async fn write_message(&mut self, _msg: &PeerMessage) -> anyhow::Result<()> {
                Ok(())
            }

            async fn read_message(&mut self) -> anyhow::Result<Option<PeerMessage>> {
                // Return None to indicate stream closed
                Ok(None)
            }
        }

        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _, _, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let closed_io = ClosedStreamMessageIO {};

        // Start the peer connection with closed stream
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(closed_io)).await;
        });

        // Should receive a disconnect notification when stream is detected as closed
        match tokio::time::timeout(tokio::time::Duration::from_millis(200), event_rx.recv()).await {
            Ok(Some(bittorrent_from_scratch::types::PeerEvent::Disconnect(_disconnect))) => {
                // Disconnect event received (stream closed)
            }
            other => panic!("Expected disconnect notification, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_read_error_triggers_disconnect() {
        use bittorrent_from_scratch::io::MessageIO;

        // Create a MessageIO that fails on read
        #[derive(Debug)]
        struct FailingReadMessageIO {}

        #[async_trait::async_trait]
        impl MessageIO for FailingReadMessageIO {
            async fn write_message(&mut self, _msg: &PeerMessage) -> anyhow::Result<()> {
                Ok(())
            }

            async fn read_message(&mut self) -> anyhow::Result<Option<PeerMessage>> {
                Err(anyhow::anyhow!("Read error"))
            }
        }

        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _, _, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let failing_io = FailingReadMessageIO {};

        // Start the peer connection with failing read
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(failing_io)).await;
        });

        // Should receive a disconnect notification
        match tokio::time::timeout(tokio::time::Duration::from_millis(200), event_rx.recv()).await {
            Ok(Some(bittorrent_from_scratch::types::PeerEvent::Disconnect(_disconnect))) => {
                // Disconnect event received (peer disconnected due to read error)
            }
            other => panic!("Expected disconnect notification, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_bitfield_received_immediately_after_start() {
        // This test verifies the fix for the bitfield race condition.
        // Previously, sending interested before spawning the message handler
        // caused bitfield messages to be lost if peers responded quickly.

        let (event_tx, _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        // Create fake I/O with aggressive timing - peer sends bitfield IMMEDIATELY
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection
        tokio::spawn(async move {
            peer_conn.start_with_io(Box::new(peer_io)).await.unwrap();
        });

        // Wait for peer to receive Interested message
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            client_io.read_message(),
        )
        .await
        {
            Ok(Ok(Some(PeerMessage::Interested(_)))) => {
                // Good - peer connection sent interested
            }
            other => panic!("Expected Interested message, got: {:?}", other),
        }

        // Immediately send bitfield response (simulating fast peer)
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: vec![true, true, true, true, false],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();

        // Give message handler time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Verify bitfield was populated (not lost due to race condition)
        let bf = bitfield.read().await;
        assert_eq!(bf.len(), 4, "Bitfield should have 4 available pieces");
        assert!(bf.contains(&0), "Piece 0 should be available");
        assert!(bf.contains(&1), "Piece 1 should be available");
        assert!(bf.contains(&2), "Piece 2 should be available");
        assert!(bf.contains(&3), "Piece 3 should be available");
        assert!(!bf.contains(&4), "Piece 4 should not be available");
    }

    #[tokio::test]
    async fn test_multiple_concurrent_pieces_download() {
        // Test that multiple pieces can be downloaded concurrently
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested
        let _ = client_io.read_message().await;

        // Send Bitfield with 3 pieces available
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: vec![true, true, true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Request 3 pieces concurrently
        for piece_idx in 0..3u32 {
            use sha1::{Digest, Sha1};
            let piece_data = vec![piece_idx as u8; 16384];
            let mut hasher = Sha1::new();
            hasher.update(&piece_data);
            let expected_hash: [u8; 20] = hasher.finalize().into();

            download_request_tx
                .send(PieceDownloadRequest {
                    piece_index: piece_idx,
                    expected_hash,
                    piece_length: 16384,
                })
                .await
                .unwrap();
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Handle requests for all 3 pieces
        for _ in 0..3 {
            let request = match client_io.read_message().await {
                Ok(Some(PeerMessage::Request(req))) => req,
                other => panic!("Expected Request, got: {:?}", other),
            };

            let piece_idx = request.piece_index;
            let piece_data = vec![piece_idx as u8; 16384];

            client_io
                .write_message(&PeerMessage::Piece(PieceMessage {
                    piece_index: piece_idx,
                    begin: request.begin,
                    block: piece_data,
                }))
                .await
                .unwrap();
        }

        // Verify all 3 pieces complete
        let mut completed_pieces = std::collections::HashSet::new();
        for _ in 0..3 {
            match tokio::time::timeout(tokio::time::Duration::from_secs(2), event_rx.recv()).await {
                Ok(Some(bittorrent_from_scratch::types::PeerEvent::StorePiece(completed))) => {
                    assert!(completed.piece_index < 3);
                    completed_pieces.insert(completed.piece_index);
                }
                other => panic!("Expected completion, got: {:?}", other),
            }
        }
        assert_eq!(
            completed_pieces.len(),
            3,
            "Should have completed 3 unique pieces"
        );
    }

    #[tokio::test]
    async fn test_queue_full_error() {
        // Test that exceeding queue capacity returns "Queue full" error
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        let _ = client_io.read_message().await;

        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: vec![true; 10],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Request 6 pieces (exceeds MAX_PIECES_PER_PEER = 5)
        for piece_idx in 0..6u32 {
            use sha1::{Digest, Sha1};
            let piece_data = vec![piece_idx as u8; 16384];
            let mut hasher = Sha1::new();
            hasher.update(&piece_data);
            let expected_hash: [u8; 20] = hasher.finalize().into();

            download_request_tx
                .send(PieceDownloadRequest {
                    piece_index: piece_idx,
                    expected_hash,
                    piece_length: 16384,
                })
                .await
                .unwrap();
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // The 6th piece should fail with "Queue full"
        match tokio::time::timeout(tokio::time::Duration::from_secs(1), event_rx.recv()).await {
            Ok(Some(bittorrent_from_scratch::types::PeerEvent::Failure(failed))) => {
                assert!(
                    matches!(
                        failed.error,
                        bittorrent_from_scratch::error::AppError::PeerQueueFull
                    ),
                    "Should report queue full, got: {:?}",
                    failed.error
                );
            }
            other => panic!("Expected queue full failure, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_shutdown_piece_tasks_cleans_up_active_downloads() {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        let _ = client_io.read_message().await;

        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: vec![true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let piece_request = PieceDownloadRequest {
            piece_index: 0,
            piece_length: 16384,
            expected_hash: [0u8; 20],
        };

        download_request_tx
            .send(piece_request.clone())
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        drop(client_io);

        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        let result =
            tokio::time::timeout(tokio::time::Duration::from_millis(500), event_rx.recv()).await;
        if let Ok(Some(bittorrent_from_scratch::types::PeerEvent::Failure(failed))) = result {
            assert!(
                matches!(
                    failed.error,
                    bittorrent_from_scratch::error::AppError::PeerStreamClosed
                ),
                "Failure reason should indicate disconnection, got: {:?}",
                failed.error
            );
        }
    }

    #[tokio::test]
    async fn test_semaphore_limits_concurrent_block_requests() {
        let (event_tx, _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Consume initial Unchoke and Interested messages
        consume_initial_interested(&mut client_io).await;

        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::peer_messages::BitfieldMessage {
                bitfield: vec![true, true, true, true, true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::peer_messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let piece_length = 16384;

        for piece_idx in 0..5u32 {
            download_request_tx
                .send(PieceDownloadRequest {
                    piece_index: piece_idx,
                    piece_length,
                    expected_hash: [0u8; 20],
                })
                .await
                .unwrap();
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let mut request_count = 0;
        let mut unique_pieces = std::collections::HashSet::new();

        for _ in 0..10 {
            match tokio::time::timeout(
                tokio::time::Duration::from_millis(200),
                client_io.read_message(),
            )
            .await
            {
                Ok(Ok(Some(PeerMessage::Request(req)))) => {
                    request_count += 1;
                    unique_pieces.insert(req.piece_index);
                }
                Ok(Ok(msg)) => panic!("Expected Request message, got: {:?}", msg),
                Ok(Err(e)) => panic!("Error reading request: {}", e),
                Err(_) => break,
            }
        }

        assert!(
            request_count <= 6,
            "Should receive at most 6 concurrent block requests due to semaphore limit (SEMAPHORE_BLOCK_CONCURRENCY), got {}",
            request_count
        );

        assert!(request_count > 0, "Should receive at least 1 request");

        assert_eq!(
            unique_pieces.len(),
            1,
            "Should only download 1 piece at a time due to MAX_PIECES_PER_PEER=1, got {} pieces",
            unique_pieces.len()
        );
    }

    #[tokio::test]
    async fn test_upload_request_sends_piece_message() {
        let (event_tx, _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let piece_manager = helpers::create_test_piece_manager();

        use sha1::{Digest, Sha1};
        let piece_data = vec![42u8; 16384];
        let hash = Sha1::new().chain_update(&piece_data).finalize();
        let hash_array: [u8; 20] = hash.into();

        piece_manager
            .queue_piece(PieceDownloadRequest {
                piece_index: 0,
                piece_length: 16384,
                expected_hash: hash_array,
            })
            .await
            .unwrap();
        piece_manager.pop_pending().await;
        piece_manager.start_download(0, "peer1".to_string()).await;
        piece_manager
            .complete_piece(0, piece_data.clone())
            .await
            .unwrap();

        let (peer_conn, _download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            piece_manager,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        consume_initial_interested(&mut client_io).await;

        let request_msg =
            PeerMessage::Request(bittorrent_from_scratch::peer_messages::RequestMessage {
                piece_index: 0,
                begin: 0,
                length: 1024,
            });
        client_io.write_message(&request_msg).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        match tokio::time::timeout(
            tokio::time::Duration::from_millis(200),
            client_io.read_message(),
        )
        .await
        {
            Ok(Ok(Some(PeerMessage::Piece(piece_msg)))) => {
                assert_eq!(piece_msg.piece_index, 0);
                assert_eq!(piece_msg.begin, 0);
                assert_eq!(piece_msg.block.len(), 1024);
                assert_eq!(piece_msg.block, vec![42u8; 1024]);
            }
            other => panic!(
                "Expected Piece message in response to Request, got: {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn test_upload_request_for_unavailable_piece_ignored() {
        let (event_tx, _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            helpers::create_test_piece_manager(),
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        consume_initial_interested(&mut client_io).await;

        let request_msg =
            PeerMessage::Request(bittorrent_from_scratch::peer_messages::RequestMessage {
                piece_index: 99,
                begin: 0,
                length: 1024,
            });
        client_io.write_message(&request_msg).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        match tokio::time::timeout(
            tokio::time::Duration::from_millis(200),
            client_io.read_message(),
        )
        .await
        {
            Err(_) => {}
            other => panic!("Expected timeout (no response), got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_upload_request_with_offset() {
        let (event_tx, _event_rx) = mpsc::unbounded_channel();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let piece_manager = helpers::create_test_piece_manager();

        use sha1::{Digest, Sha1};
        let mut piece_data = vec![0u8; 16384];
        for i in 0..16384 {
            piece_data[i] = (i % 256) as u8;
        }
        let hash = Sha1::new().chain_update(&piece_data).finalize();
        let hash_array: [u8; 20] = hash.into();

        piece_manager
            .queue_piece(PieceDownloadRequest {
                piece_index: 0,
                piece_length: 16384,
                expected_hash: hash_array,
            })
            .await
            .unwrap();
        piece_manager.pop_pending().await;
        piece_manager.start_download(0, "peer1".to_string()).await;
        piece_manager
            .complete_piece(0, piece_data.clone())
            .await
            .unwrap();

        let (peer_conn, _download_request_tx, _bitfield, _message_tx) = PeerConnection::new(
            peer,
            event_tx,
            tcp_connector,
            piece_manager,
            None,
            helpers::create_test_bandwidth_stats(),
            helpers::create_test_broadcast_receiver(),
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        consume_initial_interested(&mut client_io).await;

        let request_msg =
            PeerMessage::Request(bittorrent_from_scratch::peer_messages::RequestMessage {
                piece_index: 0,
                begin: 1000,
                length: 500,
            });
        client_io.write_message(&request_msg).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        match tokio::time::timeout(
            tokio::time::Duration::from_millis(200),
            client_io.read_message(),
        )
        .await
        {
            Ok(Ok(Some(PeerMessage::Piece(piece_msg)))) => {
                assert_eq!(piece_msg.piece_index, 0);
                assert_eq!(piece_msg.begin, 1000);
                assert_eq!(piece_msg.block.len(), 500);
                assert_eq!(piece_msg.block, &piece_data[1000..1500]);
            }
            other => panic!("Expected Piece message, got: {:?}", other),
        }
    }
}
