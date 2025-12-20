mod helpers;

#[cfg(test)]
mod tests {
    use super::helpers::fakes::FakeMessageIO;
    use bittorrent_from_scratch::messages::PeerMessage;
    use bittorrent_from_scratch::traits::MessageIO;
    use bittorrent_from_scratch::types::{
        CompletedPiece, FailedPiece, Peer, PeerConnection, PeerDisconnected, PieceDownloadRequest,
    };
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_peer_connection_sends_interested_on_start() {
        // Create channels for PeerConnection
        let (completion_tx, mut _completion_rx) = mpsc::channel::<CompletedPiece>(10);
        let (failure_tx, mut _failure_rx) = mpsc::channel::<FailedPiece>(10);
        let (disconnect_tx, mut _disconnect_rx) = mpsc::channel::<PeerDisconnected>(10);

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector = std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::RealTcpConnector);

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
            client_io.read_message()
        ).await {
            Ok(Ok(Some(PeerMessage::Interested(_)))) => {
                // Success! PeerConnection sent Interested message
            }
            other => panic!("Expected Interested message, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_bitfield_message_updates_peer_bitfield() {
        // Create channels for PeerConnection
        let (completion_tx, mut _completion_rx) = mpsc::channel::<CompletedPiece>(10);
        let (failure_tx, mut _failure_rx) = mpsc::channel::<FailedPiece>(10);
        let (disconnect_tx, mut _disconnect_rx) = mpsc::channel::<PeerDisconnected>(10);

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector = std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::RealTcpConnector);

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
        let bitfield_msg = PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
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
    async fn test_unchoke_message_allows_piece_requests() {
        // Create channels for PeerConnection
        let (completion_tx, mut _completion_rx) = mpsc::channel::<CompletedPiece>(10);
        let (failure_tx, mut _failure_rx) = mpsc::channel::<FailedPiece>(10);
        let (disconnect_tx, mut _disconnect_rx) = mpsc::channel::<PeerDisconnected>(10);

        let peer = Peer::new("127.0.0.1".to_string(), 6881);
        let tcp_connector = std::sync::Arc::new(bittorrent_from_scratch::tcp_connector::RealTcpConnector);

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
        let bitfield_msg = PeerMessage::Bitfield(bittorrent_from_scratch::messages::BitfieldMessage {
            bitfield: bitfield_data,
        });
        client_io.write_message(&bitfield_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Choke message first to test unchoke behavior
        let choke_msg = PeerMessage::Choke(bittorrent_from_scratch::messages::ChokeMessage {});
        client_io.write_message(&choke_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send a piece download request (while choked)
        let piece_request = PieceDownloadRequest {
            piece_index: 0,
            expected_hash: [0u8; 20],
            piece_length: 16384,
        };
        download_request_tx.send(piece_request).await.unwrap();

        // Give it time to process the request (but peer is choked, so no Request messages yet)
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Send Unchoke message
        let unchoke_msg = PeerMessage::Unchoke(bittorrent_from_scratch::messages::UnchokeMessage {});
        client_io.write_message(&unchoke_msg).await.unwrap();

        // Give it time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // After unchoke, peer should send Request messages for blocks
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            client_io.read_message()
        ).await {
            Ok(Ok(Some(PeerMessage::Request(_)))) => {
                // Success! PeerConnection sent Request message after Unchoke
            }
            other => panic!("Expected Request message after Unchoke, got: {:?}", other),
        }
    }
}
