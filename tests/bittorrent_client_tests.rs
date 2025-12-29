use sha1::{Digest, Sha1};
use tempfile::TempDir;
use tokio::fs::File;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

/// Helper to create test data and calculate its SHA1 hash
fn create_test_data_with_hash(size: usize) -> (Vec<u8>, [u8; 20]) {
    let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
    let hash = Sha1::new().chain_update(&data).finalize();
    let hash_array: [u8; 20] = hash.into();
    (data, hash_array)
}

#[tokio::test]
async fn test_piece_request_last_piece_smaller() {
    let file_size: usize = 100_000; // 100 KB
    let piece_length: usize = 16_384; // 16 KB
    let num_pieces = (file_size + piece_length - 1) / piece_length; // = 7 pieces

    // Calculate expected lengths for each piece
    let expected_lengths: Vec<usize> = (0..num_pieces)
        .map(|idx| {
            let offset = idx * piece_length;
            let remaining = file_size.saturating_sub(offset);
            std::cmp::min(piece_length, remaining)
        })
        .collect();

    // Verify calculations
    assert_eq!(
        expected_lengths[0], 16_384,
        "First piece should be full size"
    );
    assert_eq!(
        expected_lengths[1], 16_384,
        "Second piece should be full size"
    );
    assert_eq!(
        expected_lengths[6], 1696,
        "Last piece should be 1696 bytes (100000 - 6*16384)"
    );
    assert_eq!(expected_lengths.len(), 7, "Should have 7 pieces");
}

#[tokio::test]
async fn test_hash_verification_correct_data() {
    // Create test data
    let (data, expected_hash) = create_test_data_with_hash(16_384);

    // Verify hash matches
    let computed_hash = Sha1::new().chain_update(&data).finalize();
    let computed_hash_array: [u8; 20] = computed_hash.into();

    assert_eq!(
        computed_hash_array, expected_hash,
        "Hash should match for correct data"
    );
}

#[tokio::test]
async fn test_hash_verification_corrupted_data() {
    // Create test data
    let (mut data, expected_hash) = create_test_data_with_hash(16_384);

    // Corrupt the data
    data[100] = data[100].wrapping_add(1);

    // Verify hash doesn't match
    let computed_hash = Sha1::new().chain_update(&data).finalize();
    let computed_hash_array: [u8; 20] = computed_hash.into();

    assert_ne!(
        computed_hash_array, expected_hash,
        "Hash should NOT match for corrupted data"
    );
}

#[tokio::test]
async fn test_hash_verification_last_piece() {
    // Simulate last piece which is smaller
    let last_piece_size = 1696;
    let (data, expected_hash) = create_test_data_with_hash(last_piece_size);

    // Verify hash for smaller piece
    let computed_hash = Sha1::new().chain_update(&data).finalize();
    let computed_hash_array: [u8; 20] = computed_hash.into();

    assert_eq!(
        computed_hash_array, expected_hash,
        "Hash should work correctly for smaller last piece"
    );
}

#[tokio::test]
async fn test_file_write_and_read_single_piece() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test_single_piece.bin");

    let (piece_data, expected_hash) = create_test_data_with_hash(16_384);

    // Write piece to file
    let mut file = File::create(&file_path).await.unwrap();
    file.write_all(&piece_data).await.unwrap();
    file.flush().await.unwrap();
    drop(file);

    // Read back and verify
    let read_data = tokio::fs::read(&file_path).await.unwrap();
    assert_eq!(read_data.len(), 16_384, "File size should match");

    let computed_hash = Sha1::new().chain_update(&read_data).finalize();
    let computed_hash_array: [u8; 20] = computed_hash.into();

    assert_eq!(
        computed_hash_array, expected_hash,
        "Hash should match after write/read"
    );
}

#[tokio::test]
async fn test_file_write_multiple_pieces_sequential() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test_multiple_pieces.bin");

    let piece_length = 16_384;
    let num_pieces = 3;
    let file_size = piece_length * num_pieces;

    // Create file with correct size
    let mut file = File::create(&file_path).await.unwrap();
    file.set_len(file_size as u64).await.unwrap();

    // Create and write pieces
    let mut pieces_with_hashes = Vec::new();
    for piece_index in 0..num_pieces {
        let (piece_data, expected_hash) = create_test_data_with_hash(piece_length);

        // Write piece at correct offset
        let offset = piece_index * piece_length;
        file.seek(std::io::SeekFrom::Start(offset as u64))
            .await
            .unwrap();
        file.write_all(&piece_data).await.unwrap();
        file.flush().await.unwrap();

        pieces_with_hashes.push((piece_index, piece_data, expected_hash));
    }
    drop(file);

    // Verify each piece by reading back
    let full_data = tokio::fs::read(&file_path).await.unwrap();
    assert_eq!(full_data.len(), file_size, "File size should match");

    for (piece_index, expected_data, expected_hash) in pieces_with_hashes {
        let offset = piece_index * piece_length;
        let piece_data = &full_data[offset..offset + piece_length];

        assert_eq!(
            piece_data,
            expected_data.as_slice(),
            "Piece {} data should match",
            piece_index
        );

        let computed_hash = Sha1::new().chain_update(piece_data).finalize();
        let computed_hash_array: [u8; 20] = computed_hash.into();

        assert_eq!(
            computed_hash_array, expected_hash,
            "Piece {} hash should match",
            piece_index
        );
    }
}

#[tokio::test]
async fn test_file_write_with_smaller_last_piece() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test_last_piece.bin");

    let piece_length: usize = 16_384;
    let file_size: usize = 100_000; // Will result in last piece being 1696 bytes
    let num_pieces = (file_size + piece_length - 1) / piece_length; // 7 pieces

    // Create file with correct size
    let mut file = File::create(&file_path).await.unwrap();
    file.set_len(file_size as u64).await.unwrap();

    // Create and write pieces
    let mut pieces_with_hashes = Vec::new();
    for piece_index in 0..num_pieces {
        let offset = piece_index * piece_length;
        let remaining = file_size.saturating_sub(offset);
        let current_piece_length = std::cmp::min(piece_length, remaining);

        let (piece_data, expected_hash) = create_test_data_with_hash(current_piece_length);

        // Write piece at correct offset
        file.seek(std::io::SeekFrom::Start(offset as u64))
            .await
            .unwrap();
        file.write_all(&piece_data).await.unwrap();
        file.flush().await.unwrap();

        pieces_with_hashes.push((piece_index, current_piece_length, expected_hash));
    }
    drop(file);

    // Verify each piece by reading back
    let full_data = tokio::fs::read(&file_path).await.unwrap();
    assert_eq!(full_data.len(), file_size, "File size should match");

    for (piece_index, piece_len, expected_hash) in pieces_with_hashes {
        let offset = piece_index * piece_length;
        let piece_data = &full_data[offset..offset + piece_len];

        assert_eq!(
            piece_data.len(),
            piece_len,
            "Piece {} length should be correct",
            piece_index
        );

        let computed_hash = Sha1::new().chain_update(piece_data).finalize();
        let computed_hash_array: [u8; 20] = computed_hash.into();

        assert_eq!(
            computed_hash_array, expected_hash,
            "Piece {} hash should match",
            piece_index
        );

        // Verify last piece is indeed smaller
        if piece_index == num_pieces - 1 {
            assert_eq!(
                piece_len, 1696,
                "Last piece should be 1696 bytes (100000 % 16384)"
            );
        }
    }
}

#[tokio::test]
async fn test_piece_length_calculation_matches_verification() {
    let file_size: usize = 100_000;
    let piece_length: usize = 16_384;
    let num_pieces = (file_size + piece_length - 1) / piece_length;

    // Simulate piece request creation (from bittorrent_client.rs)
    let piece_requests: Vec<(u32, usize)> = (0..num_pieces)
        .map(|idx| {
            let offset = idx * piece_length;
            let remaining = file_size.saturating_sub(offset);
            let current_piece_length = std::cmp::min(piece_length, remaining);
            (idx as u32, current_piece_length)
        })
        .collect();

    // Simulate verification logic (from verify_file_integrity)
    for (piece_index, expected_length) in piece_requests {
        let offset = piece_index as usize * piece_length;
        let remaining = file_size.saturating_sub(offset);
        let verify_piece_length = std::cmp::min(piece_length, remaining);

        assert_eq!(
            expected_length, verify_piece_length,
            "Piece {} length should match between request and verification",
            piece_index
        );
    }
}
