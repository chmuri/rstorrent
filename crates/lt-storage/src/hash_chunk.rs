use lt_common::hash_string::HashString;
use lt_crypto::sha1::Digest;
use lt_crypto::sha1::Sha1;

use crate::chunk_list::ChunkHandle;

pub struct HashChunk {
    handle: ChunkHandle,
    data: Vec<u8>,
    position: usize,
    hasher: Sha1,
}

impl HashChunk {
    pub fn new(handle: ChunkHandle, data: Vec<u8>) -> Self {
        Self {
            handle,
            data,
            position: 0,
            hasher: Sha1::new(),
        }
    }

    pub fn handle(&self) -> &ChunkHandle {
        &self.handle
    }

    pub fn handle_mut(&mut self) -> &mut ChunkHandle {
        &mut self.handle
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.position)
    }

    pub fn perform(&mut self, mut length: usize) -> bool {
        let rem = self.remaining();
        length = length.min(rem);
        if length > 0 {
            self.hasher
                .update(&self.data[self.position..self.position + length]);
            self.position += length;
        }
        self.remaining() == 0
    }

    pub fn calculate_hash(&mut self) -> HashString {
        if self.remaining() > 0 {
            let rem = self.remaining();
            self.perform(rem);
        }
        let digest = self.hasher.clone().finalize();
        let mut raw = [0u8; 20];
        raw.copy_from_slice(&digest);
        HashString::new(raw)
    }
}
