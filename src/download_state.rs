use crate::error::{AppError, Result};
use log::{debug, error};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const BLOCK_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct DownloadState {
    pub piece_index: u32,
    piece_length: usize,
    block_size: usize,
    pub total_blocks: usize,
    expected_hash: [u8; 20],
    pub received_blocks: HashMap<u32, Vec<u8>>,
    pending_blocks: HashMap<u32, Instant>,
}

impl DownloadState {
    pub fn new(
        piece_index: u32,
        piece_length: usize,
        block_size: usize,
        expected_hash: [u8; 20],
    ) -> Self {
        let total_blocks = piece_length.div_ceil(block_size);

        Self {
            piece_index,
            piece_length,
            block_size,
            total_blocks,
            expected_hash,
            received_blocks: HashMap::new(),
            pending_blocks: HashMap::new(),
        }
    }

    pub fn add_block(&mut self, begin: u32, data: Vec<u8>) -> Result<()> {
        if self.received_blocks.contains_key(&begin) {
            debug!(
                "Piece {}: Duplicate block at offset {} (size {}), skipping",
                self.piece_index,
                begin,
                data.len()
            );
            return Ok(());
        }

        debug!(
            "Piece {}: Storing block at offset {} with size {}",
            self.piece_index,
            begin,
            data.len()
        );

        self.received_blocks.insert(begin, data);
        self.pending_blocks.remove(&begin);
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.received_blocks.len() == self.total_blocks
    }

    pub fn assemble_piece(&self) -> Result<Vec<u8>> {
        if !self.is_complete() {
            return Err(AppError::IncompletePiece(self.piece_index).into());
        }

        let mut piece_data = Vec::with_capacity(self.piece_length);
        let mut offset = 0u32;

        debug!(
            "Piece {}: Assembling {} blocks, expected piece length {}",
            self.piece_index, self.total_blocks, self.piece_length
        );

        for block_num in 0..self.total_blocks {
            let block = self.received_blocks.get(&offset).ok_or_else(|| {
                error!(
                    "Piece {}: Missing block at offset {} (block {})",
                    self.piece_index, offset, block_num
                );
                anyhow::Error::from(AppError::IncompletePiece(self.piece_index))
            })?;

            debug!(
                "Piece {}: Assembling block {} at offset {} with size {}",
                self.piece_index,
                block_num,
                offset,
                block.len()
            );

            piece_data.extend_from_slice(block);
            offset += self.block_size as u32;
        }

        debug!(
            "Piece {}: Assembled data length {} (expected {}), truncating to piece_length",
            self.piece_index,
            piece_data.len(),
            self.piece_length
        );

        piece_data.truncate(self.piece_length);

        debug!(
            "Piece {}: Final data length after truncate: {}",
            self.piece_index,
            piece_data.len()
        );

        Ok(piece_data)
    }

    pub fn verify_hash(&self, data: &[u8]) -> Result<bool> {
        let mut hasher = Sha1::new();
        hasher.update(data);
        let computed_hash = hasher.finalize();
        Ok(computed_hash.as_slice() == self.expected_hash)
    }

    pub fn get_next_block_to_request(&mut self) -> Option<(u32, u32)> {
        let mut offset = 0u32;
        let now = Instant::now();

        for _ in 0..self.total_blocks {
            if self.received_blocks.contains_key(&offset) {
                offset += self.block_size as u32;
                continue;
            }

            let should_request = match self.pending_blocks.get(&offset) {
                None => true,
                Some(request_time) => now.duration_since(*request_time) >= BLOCK_REQUEST_TIMEOUT,
            };

            if should_request {
                let remaining = self.piece_length.saturating_sub(offset as usize);
                let length = std::cmp::min(self.block_size, remaining) as u32;

                if self.pending_blocks.insert(offset, now).is_some() {
                    debug!(
                        "Piece {}: Retrying block at offset {} after timeout",
                        self.piece_index, offset
                    );
                }

                return Some((offset, length));
            }

            offset += self.block_size as u32;
        }

        None
    }

    pub fn expected_hash(&self) -> &[u8; 20] {
        &self.expected_hash
    }

    pub fn piece_length(&self) -> usize {
        self.piece_length
    }

    #[cfg(test)]
    fn set_pending_block_timestamp(&mut self, offset: u32, timestamp: Instant) {
        self.pending_blocks.insert(offset, timestamp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_retry_after_timeout() {
        let mut state = DownloadState::new(0, 32 * 1024, 16 * 1024, [0u8; 20]);

        let (begin, length) = state.get_next_block_to_request().unwrap();
        assert_eq!(begin, 0);
        assert_eq!(length, 16 * 1024);

        let old_time = Instant::now() - Duration::from_secs(31);
        state.set_pending_block_timestamp(0, old_time);

        let (begin, length) = state.get_next_block_to_request().unwrap();
        assert_eq!(begin, 0);
        assert_eq!(length, 16 * 1024);
    }

    #[test]
    fn test_block_not_retried_before_timeout() {
        let mut state = DownloadState::new(0, 32 * 1024, 16 * 1024, [0u8; 20]);

        state.get_next_block_to_request().unwrap();

        let (begin, _) = state.get_next_block_to_request().unwrap();
        assert_eq!(begin, 16 * 1024);
    }

    #[test]
    fn test_received_blocks_not_retried() {
        let mut state = DownloadState::new(0, 32 * 1024, 16 * 1024, [0u8; 20]);

        state.get_next_block_to_request().unwrap();
        state.add_block(0, vec![0u8; 16 * 1024]).unwrap();

        let old_time = Instant::now() - Duration::from_secs(31);
        state.set_pending_block_timestamp(0, old_time);

        let (begin, _) = state.get_next_block_to_request().unwrap();
        assert_eq!(begin, 16 * 1024);
    }

    #[test]
    fn test_multiple_blocks_with_timeout() {
        let mut state = DownloadState::new(0, 48 * 1024, 16 * 1024, [0u8; 20]);

        state.get_next_block_to_request().unwrap();
        state.get_next_block_to_request().unwrap();

        let old_time = Instant::now() - Duration::from_secs(31);
        state.set_pending_block_timestamp(0, old_time);

        let (begin, _) = state.get_next_block_to_request().unwrap();
        assert_eq!(begin, 0);
    }

    #[test]
    fn test_add_duplicate_block_is_noop() {
        let mut state = DownloadState::new(0, 32 * 1024, 16 * 1024, [0u8; 20]);

        let block_data = vec![1u8; 16 * 1024];
        state.add_block(0, block_data.clone()).unwrap();

        assert_eq!(state.received_blocks.len(), 1);

        let duplicate_data = vec![2u8; 16 * 1024];
        state.add_block(0, duplicate_data).unwrap();

        assert_eq!(state.received_blocks.len(), 1);
        assert_eq!(state.received_blocks.get(&0).unwrap(), &block_data);
    }

    #[test]
    fn test_assemble_incomplete_piece_fails() {
        let mut state = DownloadState::new(0, 32 * 1024, 16 * 1024, [0u8; 20]);

        state.add_block(0, vec![0u8; 16 * 1024]).unwrap();

        assert!(!state.is_complete());
        let result = state.assemble_piece();
        assert!(result.is_err());
    }

    #[test]
    fn test_assemble_piece_truncates_to_piece_length() {
        let piece_length = 20000;
        let block_size = 16384;
        let mut state = DownloadState::new(0, piece_length, block_size, [0u8; 20]);

        state.add_block(0, vec![1u8; block_size]).unwrap();
        state
            .add_block(block_size as u32, vec![2u8; block_size])
            .unwrap();

        assert!(state.is_complete());
        let assembled = state.assemble_piece().unwrap();

        assert_eq!(assembled.len(), piece_length);
        assert_eq!(&assembled[0..block_size], &vec![1u8; block_size]);
        assert_eq!(
            &assembled[block_size..piece_length],
            &vec![2u8; piece_length - block_size]
        );
    }

    #[test]
    fn test_verify_hash_correct() {
        use sha1::{Digest, Sha1};

        let piece_data = vec![42u8; 16384];
        let mut hasher = Sha1::new();
        hasher.update(&piece_data);
        let expected_hash: [u8; 20] = hasher.finalize().into();

        let state = DownloadState::new(0, 16384, 16384, expected_hash);
        assert!(state.verify_hash(&piece_data).unwrap());
    }

    #[test]
    fn test_verify_hash_incorrect() {
        let piece_data = vec![42u8; 16384];
        let wrong_hash = [0u8; 20];

        let state = DownloadState::new(0, 16384, 16384, wrong_hash);
        assert!(!state.verify_hash(&piece_data).unwrap());
    }

    #[test]
    fn test_expected_hash_getter() {
        let expected_hash = [42u8; 20];
        let state = DownloadState::new(0, 16384, 16384, expected_hash);
        assert_eq!(state.expected_hash(), &expected_hash);
    }

    #[test]
    fn test_piece_length_getter() {
        let piece_length = 32768;
        let state = DownloadState::new(0, piece_length, 16384, [0u8; 20]);
        assert_eq!(state.piece_length(), piece_length);
    }

    #[test]
    fn test_get_next_block_calculates_correct_length_for_last_block() {
        let piece_length = 20000;
        let block_size = 16384;
        let mut state = DownloadState::new(0, piece_length, block_size, [0u8; 20]);

        state.add_block(0, vec![0u8; block_size]).unwrap();

        let (begin, length) = state.get_next_block_to_request().unwrap();
        assert_eq!(begin, block_size as u32);
        assert_eq!(length, (piece_length - block_size) as u32);
    }
}
