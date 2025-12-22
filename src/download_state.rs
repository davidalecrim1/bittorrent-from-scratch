use crate::error::{AppError, Result};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub struct DownloadState {
    pub piece_index: u32,
    piece_length: usize,
    block_size: usize,
    pub total_blocks: usize,
    expected_hash: [u8; 20],
    pub received_blocks: HashMap<u32, Vec<u8>>,
    pending_blocks: HashSet<u32>,
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
            pending_blocks: HashSet::new(),
        }
    }

    pub fn add_block(&mut self, begin: u32, data: Vec<u8>) -> Result<()> {
        if self.received_blocks.contains_key(&begin) {
            return Ok(());
        }

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

        for _ in 0..self.total_blocks {
            let block = self
                .received_blocks
                .get(&offset)
                .ok_or_else(|| anyhow::Error::from(AppError::IncompletePiece(self.piece_index)))?;

            piece_data.extend_from_slice(block);
            offset += self.block_size as u32;
        }

        piece_data.truncate(self.piece_length);
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

        for _ in 0..self.total_blocks {
            if !self.received_blocks.contains_key(&offset) && !self.pending_blocks.contains(&offset)
            {
                let remaining = self.piece_length.saturating_sub(offset as usize);
                let length = std::cmp::min(self.block_size, remaining) as u32;

                self.pending_blocks.insert(offset);
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
}
