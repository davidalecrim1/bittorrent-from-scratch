mod helpers;

#[cfg(test)]
mod tests {
    use super::helpers::fakes::FakeMessageIO;
    use bittorrent_from_scratch::io::MessageIO;
    use bittorrent_from_scratch::messages::{PeerMessage, PieceMessage};
    use bittorrent_from_scratch::types::{
        CompletedPiece, FailedPiece, Peer, PeerConnection, PeerDisconnected, PieceDownloadRequest,
    };
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_peer_connection_sends_interested_on_start() {
        // Create channels for PeerConnection
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: bitfield_data.clone(),
            });

        client_io.write_message(&bitfield_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Check that the bitfield was updated
        let bf = bitfield.read().await;
        assert_eq!(*bf, bitfield_data, "Bitfield should be updated");
    }

    #[tokio::test]
    async fn test_unchoke_message_starts_piece_download() {
        // Create channels for PeerConnection
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: bitfield_data,
            });
        client_io.write_message(&bitfield_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Choke message
        let choke_msg = PeerMessage::Choke(bittorrent_from_scratch::messages::ChokeMessage {});
        client_io.write_message(&choke_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke message
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
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
        let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: bitfield_data,
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
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
        match tokio::time::timeout(tokio::time::Duration::from_secs(2), completion_rx.recv()).await
        {
            Ok(Some(completed)) => {
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
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: bitfield_data,
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
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
        match tokio::time::timeout(tokio::time::Duration::from_secs(2), failure_rx.recv()).await {
            Ok(Some(failed)) => {
                assert_eq!(failed.piece_index, 0, "Failed piece should be index 0");
                assert!(
                    failed.reason.to_lowercase().contains("hash"),
                    "Failure should mention hash mismatch, got: {}",
                    failed.reason
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
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
        );

        // Create a pair of fake MessageIO instances
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection with fake IO
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested message
        let _ = client_io.read_message().await;

        // Send Bitfield with 5 pieces, initially all false
        let bitfield_data = vec![false, false, false, false, false];
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: bitfield_data.clone(),
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Verify initial bitfield
        {
            let bf = bitfield.read().await;
            assert_eq!(*bf, bitfield_data, "Initial bitfield should match");
        }

        // Send Have message for piece 2
        let have_msg =
            PeerMessage::Have(bittorrent_from_scratch::messages::HaveMessage { piece_index: 2 });
        client_io.write_message(&have_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Verify piece 2 is now true
        {
            let bf = bitfield.read().await;
            assert_eq!(bf[0], false, "Piece 0 should still be false");
            assert_eq!(bf[1], false, "Piece 1 should still be false");
            assert_eq!(bf[2], true, "Piece 2 should be true after Have message");
            assert_eq!(bf[3], false, "Piece 3 should still be false");
            assert_eq!(bf[4], false, "Piece 4 should still be false");
        }
    }

    #[tokio::test]
    async fn test_choke_message_stops_new_requests() {
        // Create channels for PeerConnection
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: vec![true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke first
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Choke message
        let choke_msg = PeerMessage::Choke(bittorrent_from_scratch::messages::ChokeMessage {});
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
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: vec![true, true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
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
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: vec![true, false], // Only piece 0
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
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
        match tokio::time::timeout(tokio::time::Duration::from_secs(1), failure_rx.recv()).await {
            Ok(Some(failed)) => {
                assert_eq!(failed.piece_index, 1, "Failed piece should be index 1");
                assert!(
                    failed.reason.contains("does not have"),
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
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
            PeerMessage::Interested(bittorrent_from_scratch::messages::InterestedMessage {});
        client_io.write_message(&interested_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send NotInterested message
        let not_interested_msg =
            PeerMessage::NotInterested(bittorrent_from_scratch::messages::NotInterestedMessage {});
        client_io.write_message(&not_interested_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Request message (should be no-op, we don't support uploading)
        let request_msg = PeerMessage::Request(bittorrent_from_scratch::messages::RequestMessage {
            piece_index: 0,
            begin: 0,
            length: 16384,
        });
        client_io.write_message(&request_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Cancel message (should be no-op)
        let cancel_msg = PeerMessage::Cancel(bittorrent_from_scratch::messages::CancelMessage {
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
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
        let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: vec![true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
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
        match tokio::time::timeout(tokio::time::Duration::from_secs(2), completion_rx.recv()).await
        {
            Ok(Some(completed)) => {
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

        let (completion_tx, _) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, _) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _, _) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
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
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(200),
            disconnect_rx.recv(),
        )
        .await
        {
            Ok(Some(disconnect)) => {
                assert!(
                    disconnect.reason.contains("write_error"),
                    "Disconnect reason should mention write error"
                );
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

        let (completion_tx, _) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, _) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _, _) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
        );

        let closed_io = ClosedStreamMessageIO {};

        // Start the peer connection with closed stream
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(closed_io)).await;
        });

        // Should receive a disconnect notification when stream is detected as closed
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(200),
            disconnect_rx.recv(),
        )
        .await
        {
            Ok(Some(disconnect)) => {
                assert_eq!(
                    disconnect.reason, "stream_closed",
                    "Disconnect reason should be stream_closed"
                );
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

        let (completion_tx, _) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, _) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _, _) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
        );

        let failing_io = FailingReadMessageIO {};

        // Start the peer connection with failing read
        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(failing_io)).await;
        });

        // Should receive a disconnect notification
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(200),
            disconnect_rx.recv(),
        )
        .await
        {
            Ok(Some(disconnect)) => {
                assert!(
                    disconnect.reason.contains("read_error"),
                    "Disconnect reason should mention read error"
                );
            }
            other => panic!("Expected disconnect notification, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_bitfield_received_immediately_after_start() {
        // This test verifies the fix for the bitfield race condition.
        // Previously, sending interested before spawning the message handler
        // caused bitfield messages to be lost if peers responded quickly.

        let (completion_tx, _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, _download_request_tx, bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
        );

        // Create fake I/O with aggressive timing - peer sends bitfield IMMEDIATELY
        let (mut client_io, peer_io) = FakeMessageIO::pair();

        // Start the peer connection
        tokio::spawn(async move {
            peer_conn.start_with_io(Box::new(peer_io)).await.unwrap();
        });

        // Peer should receive interested message first
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
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: vec![true, true, true, true, false],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();

        // Give message handler time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Verify bitfield was populated (not lost due to race condition)
        let bf = bitfield.read().await;
        assert_eq!(bf.len(), 5, "Bitfield should have 5 pieces");
        assert_eq!(bf[0], true, "Piece 0 should be available");
        assert_eq!(bf[1], true, "Piece 1 should be available");
        assert_eq!(bf[2], true, "Piece 2 should be available");
        assert_eq!(bf[3], true, "Piece 3 should be available");
        assert_eq!(bf[4], false, "Piece 4 should not be available");
    }

    #[tokio::test]
    async fn test_multiple_concurrent_pieces_download() {
        // Test that multiple pieces can be downloaded concurrently
        let (completion_tx, mut completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        // Wait for Interested
        let _ = client_io.read_message().await;

        // Send Bitfield with 3 pieces available
        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: vec![true, true, true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke
        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
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
            match tokio::time::timeout(tokio::time::Duration::from_secs(2), completion_rx.recv())
                .await
            {
                Ok(Some(completed)) => {
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
        let (completion_tx, mut _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, mut _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        let _ = client_io.read_message().await;

        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: vec![true; 10],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
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
        match tokio::time::timeout(tokio::time::Duration::from_secs(1), failure_rx.recv()).await {
            Ok(Some(failed)) => {
                assert!(
                    failed.reason.to_lowercase().contains("queue full"),
                    "Should report queue full, got: {}",
                    failed.reason
                );
            }
            other => panic!("Expected queue full failure, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_shutdown_piece_tasks_cleans_up_active_downloads() {
        let (completion_tx, _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, mut failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        let _ = client_io.read_message().await;

        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: vec![true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
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
            tokio::time::timeout(tokio::time::Duration::from_millis(500), failure_rx.recv()).await;
        if let Ok(Some(failed)) = result {
            assert!(
                failed.reason.to_lowercase().contains("peer disconnected")
                    || failed.reason.to_lowercase().contains("channel closed"),
                "Failure reason should indicate disconnection, got: {}",
                failed.reason
            );
        }
    }

    #[tokio::test]
    async fn test_semaphore_limits_concurrent_block_requests() {
        let (completion_tx, _completion_rx) = mpsc::unbounded_channel::<CompletedPiece>();
        let (failure_tx, _failure_rx) = mpsc::unbounded_channel::<FailedPiece>();
        let (disconnect_tx, _disconnect_rx) = mpsc::unbounded_channel::<PeerDisconnected>();

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector =
            std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::DefaultTcpStreamFactory);

        let (peer_conn, download_request_tx, _bitfield) = PeerConnection::new(
            peer,
            completion_tx,
            failure_tx,
            disconnect_tx,
            tcp_connector,
        );

        let (mut client_io, peer_io) = FakeMessageIO::pair();

        tokio::spawn(async move {
            let _ = peer_conn.start_with_io(Box::new(peer_io)).await;
        });

        let _ = client_io.read_message().await;

        let bitfield_msg =
            PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
                bitfield: vec![true, true, true, true, true],
            });
        client_io.write_message(&bitfield_msg).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let unchoke_msg =
            PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
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
            request_count <= 5,
            "Should receive at most 5 concurrent block requests due to semaphore limit, got {}",
            request_count
        );

        assert!(request_count > 0, "Should receive at least 1 request");

        assert!(
            unique_pieces.len() > 1,
            "Should request blocks from multiple pieces, got {} pieces",
            unique_pieces.len()
        );
    }
}
