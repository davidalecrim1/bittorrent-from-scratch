use bittorrent_from_scratch::peer_messages::{
    BitfieldMessage, CancelMessage, ChokeMessage, HaveMessage, InterestedMessage,
    NotInterestedMessage, PeerMessage, PieceMessage, RequestMessage, UnchokeMessage,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interested_message_to_bytes() {
        let message = InterestedMessage {};
        let bytes = PeerMessage::Interested(message).to_bytes().unwrap();

        // Length (4 bytes) + message type (1 byte) = 5 bytes
        assert_eq!(bytes.len(), 5);
        // Length should be 1
        assert_eq!(bytes[0..4], [0, 0, 0, 1]);
        // Message type should be 2 (interested)
        assert_eq!(bytes[4], 2);
    }

    #[test]
    fn test_request_message_to_bytes() {
        let message = RequestMessage {
            piece_index: 5,
            begin: 16384,
            length: 16384,
        };
        let bytes = PeerMessage::Request(message).to_bytes().unwrap();

        // Length (4 bytes) + message type (1 byte) + index (4) + begin (4) + length (4) = 17 bytes
        assert_eq!(bytes.len(), 17);
        // Message type should be 6 (request)
        assert_eq!(bytes[4], 6);
    }

    #[test]
    fn test_keep_alive_from_bytes() {
        let bytes = vec![0, 0, 0, 0];
        let (size, message) = PeerMessage::from_bytes(&bytes, 0).unwrap();

        assert_eq!(size, 4);
        assert_eq!(message, PeerMessage::KeepAlive);
    }

    #[test]
    fn test_choke_message_from_bytes() {
        let bytes = vec![0, 0, 0, 1, 0]; // Length = 1, Type = 0 (choke)
        let (size, message) = PeerMessage::from_bytes(&bytes, 0).unwrap();

        assert_eq!(size, 5);
        assert!(matches!(message, PeerMessage::Choke(_)));
    }

    #[test]
    fn test_unchoke_message_from_bytes() {
        let bytes = vec![0, 0, 0, 1, 1]; // Length = 1, Type = 1 (unchoke)
        let (size, message) = PeerMessage::from_bytes(&bytes, 0).unwrap();

        assert_eq!(size, 5);
        assert!(matches!(message, PeerMessage::Unchoke(_)));
    }

    #[test]
    fn test_not_interested_message_from_bytes() {
        let bytes = vec![0, 0, 0, 1, 3]; // Length = 1, Type = 3 (not interested)
        let (size, message) = PeerMessage::from_bytes(&bytes, 0).unwrap();

        assert_eq!(size, 5);
        assert!(matches!(message, PeerMessage::NotInterested(_)));
    }

    #[test]
    fn test_have_message_from_bytes() {
        let bytes = vec![0, 0, 0, 5, 4, 0, 0, 0, 10]; // Length = 5, Type = 4 (have), piece index = 10
        let (size, message) = PeerMessage::from_bytes(&bytes, 100).unwrap();

        assert_eq!(size, 9);
        if let PeerMessage::Have(have_msg) = message {
            assert_eq!(have_msg.piece_index, 10);
        } else {
            panic!("Expected Have message");
        }
    }

    #[test]
    fn test_bitfield_message_from_bytes() {
        let bytes = vec![0, 0, 0, 2, 5, 0b10101010]; // Length = 2, Type = 5 (bitfield), data = 0b10101010
        let (size, message) = PeerMessage::from_bytes(&bytes, 8).unwrap();

        assert_eq!(size, 6);
        if let PeerMessage::Bitfield(bitfield_msg) = message {
            assert!(bitfield_msg.has_piece(0));
            assert!(!bitfield_msg.has_piece(1));
            assert!(bitfield_msg.has_piece(2));
        } else {
            panic!("Expected Bitfield message");
        }
    }

    #[test]
    fn test_piece_message_from_bytes() {
        let bytes = vec![
            0, 0, 0,
            13, // Length = 13 (1 byte msg_id + 4 bytes piece_index + 4 bytes begin + 4 bytes block)
            7,  // Type = 7 (piece)
            0, 0, 0, 5, // piece index = 5
            0, 0, 0, 100, // begin = 100
            1, 2, 3, 4, // block data = 4 bytes
        ];
        let (size, message) = PeerMessage::from_bytes(&bytes, 100).unwrap();

        // total size = 4 (length prefix) + 13 (message length) = 17
        assert_eq!(size, 17);
        if let PeerMessage::Piece(piece_msg) = message {
            assert_eq!(piece_msg.piece_index, 5);
            assert_eq!(piece_msg.begin, 100);
            assert_eq!(piece_msg.block.len(), 4);
            assert_eq!(piece_msg.block, vec![1, 2, 3, 4]);
        } else {
            panic!("Expected Piece message");
        }
    }

    #[test]
    fn test_piece_message_with_16kb_block() {
        // Simulate a real 16 KiB block download
        let block_data: Vec<u8> = (0..16384).map(|i| (i % 256) as u8).collect();
        let mut bytes = vec![
            0, 0, 0x40, 0x09, // Length = 16393 (1 + 4 + 4 + 16384)
            7,    // Type = 7 (piece)
            0, 0, 0x12, 0x34, // piece index = 4660
            0, 0, 0x40, 0x00, // begin = 16384
        ];
        bytes.extend_from_slice(&block_data);

        let (size, message) = PeerMessage::from_bytes(&bytes, 24208).unwrap();

        // total size = 4 (length prefix) + 16393 (message length) = 16397
        assert_eq!(size, 16397);
        if let PeerMessage::Piece(piece_msg) = message {
            assert_eq!(piece_msg.piece_index, 0x1234);
            assert_eq!(piece_msg.begin, 0x4000);
            assert_eq!(piece_msg.block.len(), 16384);
            assert_eq!(piece_msg.block, block_data);
        } else {
            panic!("Expected Piece message");
        }
    }

    #[test]
    fn test_piece_message_with_smaller_last_block() {
        // Test the bug fix: last block is often smaller than 16 KiB
        // For a 262144 byte piece with 16384 byte blocks:
        // - 15 full blocks of 16384 bytes
        // - 1 last block of 16384 bytes (happens to be full in this case)
        // But for a 270000 byte piece:
        // - 16 full blocks of 16384 bytes = 262144 bytes
        // - 1 last block of 7856 bytes

        let last_block_size = 7856;
        let block_data: Vec<u8> = (0..last_block_size).map(|i| (i % 256) as u8).collect();
        let mut bytes = vec![
            0, 0, 0x1e, 0xb9, // Length = 7865 (1 + 4 + 4 + 7856)
            7,    // Type = 7 (piece)
            0, 0, 0x00, 0x05, // piece index = 5
            0, 0x04, 0x00, 0x00, // begin = 262144 (offset for last block)
        ];
        bytes.extend_from_slice(&block_data);

        let (size, message) = PeerMessage::from_bytes(&bytes, 24208).unwrap();

        // total size = 4 (length prefix) + 7865 (message length) = 7869
        assert_eq!(size, 7869);
        if let PeerMessage::Piece(piece_msg) = message {
            assert_eq!(piece_msg.piece_index, 5);
            assert_eq!(piece_msg.begin, 262144);
            assert_eq!(
                piece_msg.block.len(),
                last_block_size,
                "Block should be exactly {} bytes, not 16384",
                last_block_size
            );
            assert_eq!(
                piece_msg.block, block_data,
                "Block data should match exactly without extra padding"
            );
        } else {
            panic!("Expected Piece message");
        }
    }

    #[test]
    fn test_cancel_message_from_bytes() {
        let bytes = vec![
            0, 0, 0, 13, // Length = 13
            8,  // Type = 8 (cancel)
            0, 0, 0, 5, // index = 5
            0, 0, 0, 100, // begin = 100
            0, 0, 0, 200, // length = 200
        ];
        let (size, message) = PeerMessage::from_bytes(&bytes, 100).unwrap();

        assert_eq!(size, 17);
        assert!(matches!(message, PeerMessage::Cancel(_)));
    }

    #[test]
    fn test_unknown_message_type() {
        let bytes = vec![0, 0, 0, 1, 99]; // Unknown type 99
        let (size, message) = PeerMessage::from_bytes(&bytes, 0).unwrap();

        assert_eq!(size, 5);
        // Unknown messages are treated as KeepAlive
        assert_eq!(message, PeerMessage::KeepAlive);
    }

    #[test]
    fn test_incomplete_message() {
        let bytes = vec![0, 0, 0, 5, 0]; // Says length is 5, but only 1 byte follows
        let result = PeerMessage::from_bytes(&bytes, 0);

        assert!(result.is_err());
    }

    #[test]
    fn test_message_too_short() {
        let bytes = vec![0, 0]; // Less than 4 bytes
        let result = PeerMessage::from_bytes(&bytes, 0);

        assert!(result.is_err());
    }

    #[test]
    fn test_request_message_fields() {
        let message = RequestMessage {
            piece_index: 10,
            begin: 16384,
            length: 8192,
        };

        assert_eq!(message.piece_index, 10);
        assert_eq!(message.begin, 16384);
        assert_eq!(message.length, 8192);
    }

    #[test]
    fn test_piece_message_fields() {
        let message = PieceMessage {
            piece_index: 5,
            begin: 100,
            block: vec![1, 2, 3, 4],
        };

        assert_eq!(message.piece_index, 5);
        assert_eq!(message.begin, 100);
        assert_eq!(message.block, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_cancel_message_fields() {
        let message = CancelMessage {
            piece_index: 7,
            begin: 200,
            length: 300,
        };

        assert_eq!(message.piece_index, 7);
        assert_eq!(message.begin, 200);
        assert_eq!(message.length, 300);
    }

    #[test]
    fn test_interested_message_from_bytes() {
        let bytes = vec![0, 0, 0, 1, 2]; // Length = 1, Type = 2 (interested)
        let (size, message) = PeerMessage::from_bytes(&bytes, 0).unwrap();

        assert_eq!(size, 5);
        assert!(matches!(message, PeerMessage::Interested(_)));
    }

    #[test]
    fn test_choke_message_to_bytes() {
        let message = ChokeMessage {};
        let bytes = PeerMessage::Choke(message).to_bytes().unwrap();

        assert_eq!(bytes.len(), 5);
        assert_eq!(bytes[0..4], [0, 0, 0, 1]); // Length = 1
        assert_eq!(bytes[4], 0); // Type = 0 (choke)
    }

    #[test]
    fn test_unchoke_message_to_bytes() {
        let message = UnchokeMessage {};
        let bytes = PeerMessage::Unchoke(message).to_bytes().unwrap();

        assert_eq!(bytes.len(), 5);
        assert_eq!(bytes[0..4], [0, 0, 0, 1]); // Length = 1
        assert_eq!(bytes[4], 1); // Type = 1 (unchoke)
    }

    #[test]
    fn test_not_interested_message_to_bytes() {
        let message = NotInterestedMessage {};
        let bytes = PeerMessage::NotInterested(message).to_bytes().unwrap();

        assert_eq!(bytes.len(), 5);
        assert_eq!(bytes[0..4], [0, 0, 0, 1]); // Length = 1
        assert_eq!(bytes[4], 3); // Type = 3 (not interested)
    }

    #[test]
    fn test_have_message_to_bytes() {
        let message = HaveMessage { piece_index: 42 };
        let bytes = PeerMessage::Have(message).to_bytes().unwrap();

        assert_eq!(bytes.len(), 9);
        assert_eq!(bytes[0..4], [0, 0, 0, 5]); // Length = 5
        assert_eq!(bytes[4], 4); // Type = 4 (have)
        assert_eq!(&bytes[5..9], &[0, 0, 0, 42]); // piece_index = 42
    }

    #[test]
    fn test_bitfield_message_to_bytes() {
        let message = BitfieldMessage::from_bytes(&[0b11110000], 8).unwrap();
        let bytes = PeerMessage::Bitfield(message).to_bytes().unwrap();

        assert_eq!(bytes.len(), 6);
        assert_eq!(bytes[0..4], [0, 0, 0, 2]); // Length = 2 (1 byte type + 1 byte data)
        assert_eq!(bytes[4], 5); // Type = 5 (bitfield)
        assert_eq!(bytes[5], 0b11110000); // Bitfield data
    }

    #[test]
    fn test_piece_message_to_bytes() {
        let message = PieceMessage {
            piece_index: 10,
            begin: 16384,
            block: vec![0xaa, 0xbb, 0xcc, 0xdd],
        };
        let bytes = PeerMessage::Piece(message).to_bytes().unwrap();

        assert_eq!(bytes.len(), 17); // 4 + 1 + 4 + 4 + 4
        assert_eq!(bytes[0..4], [0, 0, 0, 13]); // Length = 13 (1 + 4 + 4 + 4)
        assert_eq!(bytes[4], 7); // Type = 7 (piece)
        assert_eq!(&bytes[13..17], &[0xaa, 0xbb, 0xcc, 0xdd]); // Block data
    }

    #[test]
    fn test_cancel_message_to_bytes() {
        let message = CancelMessage {
            piece_index: 5,
            begin: 100,
            length: 200,
        };
        let bytes = PeerMessage::Cancel(message).to_bytes().unwrap();

        assert_eq!(bytes.len(), 17);
        assert_eq!(bytes[0..4], [0, 0, 0, 13]); // Length = 13
        assert_eq!(bytes[4], 8); // Type = 8 (cancel)
    }

    #[test]
    fn test_keep_alive_to_bytes() {
        let bytes = PeerMessage::KeepAlive.to_bytes().unwrap();

        assert_eq!(bytes.len(), 4);
        assert_eq!(bytes, vec![0, 0, 0, 0]); // Length = 0
    }

    #[test]
    fn test_message_length_too_short_error() {
        // Test message with length field but less than 5 bytes total
        let bytes = vec![0, 0, 0, 1]; // Says length is 1, but no type byte
        let result = PeerMessage::from_bytes(&bytes, 0);

        assert!(result.is_err());
    }
}
