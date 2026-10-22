use crate::chunk::Chunk;
use crate::error::StorageError;
use crate::memory_chunk::MemoryChunk;

pub struct ChunkIterator<'a> {
    chunk: &'a Chunk,
    part_idx: usize,
    first: u32,
    last: u32,
}

impl<'a> ChunkIterator<'a> {
    pub fn new(chunk: &'a Chunk, first: u32, last: u32) -> Result<Self, StorageError> {
        let part_idx = if first >= last || chunk.is_empty() {
            chunk.parts().len()
        } else {
            chunk.at_position(first)?
        };

        Ok(Self {
            chunk,
            part_idx,
            first,
            last,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.part_idx >= self.chunk.parts().len() || self.first >= self.last
    }

    pub fn memory_chunk(&self) -> Option<&'a MemoryChunk> {
        self.chunk.parts().get(self.part_idx).map(|p| p.chunk())
    }

    pub fn memory_chunk_first(&self) -> u32 {
        if let Some(part) = self.chunk.parts().get(self.part_idx) {
            self.first.saturating_sub(part.position())
        } else {
            0
        }
    }

    pub fn memory_chunk_last(&self) -> u32 {
        if let Some(part) = self.chunk.parts().get(self.part_idx) {
            self.last.saturating_sub(part.position())
        } else {
            0
        }
    }

    /// Slice of the current part bounded by [first, last)
    pub fn current_slice(&self) -> &'a [u8] {
        if self.is_empty() {
            return &[];
        }
        let part = &self.chunk.parts()[self.part_idx];
        let offset = self.first.saturating_sub(part.position()) as usize;
        let available = (part.size() as usize).saturating_sub(offset);
        let wanted = (self.last.saturating_sub(self.first)) as usize;
        let len = available.min(wanted);

        let slice = part.chunk().as_slice();
        if offset + len <= slice.len() {
            &slice[offset..offset + len]
        } else {
            &[]
        }
    }

    pub fn advance(&mut self) -> bool {
        if self.part_idx >= self.chunk.parts().len() {
            return false;
        }

        let part = &self.chunk.parts()[self.part_idx];
        self.first = part.position() + part.size();
        self.part_idx += 1;

        while self.part_idx < self.chunk.parts().len() {
            if self.chunk.parts()[self.part_idx].size() != 0 {
                return self.first < self.last;
            }
            self.part_idx += 1;
        }

        false
    }

    pub fn forward(&mut self, length: u32) -> bool {
        self.first = self.first.saturating_add(length);
        if self.first >= self.last {
            return false;
        }

        while self.part_idx < self.chunk.parts().len() {
            let part = &self.chunk.parts()[self.part_idx];
            if self.first < part.position() + part.size() {
                return true;
            }
            self.part_idx += 1;
        }

        false
    }
}
