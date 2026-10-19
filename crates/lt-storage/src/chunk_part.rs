use crate::error::StorageError;
use crate::memory_chunk::MemoryChunk;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappedType {
    Mmap,
    Static,
}

#[derive(Clone, Debug)]
pub struct ChunkPart {
    mapped: MappedType,
    chunk: MemoryChunk,
    position: u32,
    file_offset: u64,
}

impl ChunkPart {
    pub fn new(mapped: MappedType, chunk: MemoryChunk, position: u32) -> Self {
        Self {
            mapped,
            chunk,
            position,
            file_offset: 0,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.chunk.is_valid()
    }

    pub fn is_contained(&self, p: u32) -> bool {
        p >= self.position && p < self.position.saturating_add(self.size())
    }

    pub fn has_address(&self, p: *const u8) -> bool {
        let b = self.chunk.begin();
        let e = self.chunk.end();
        !b.is_null() && !e.is_null() && (p as *mut u8) >= b && (p as *mut u8) < e
    }

    pub fn clear(&mut self) {
        if self.mapped == MappedType::Mmap {
            self.chunk.unmap();
        }
        self.chunk.clear();
    }

    pub fn mapped(&self) -> MappedType {
        self.mapped
    }

    pub fn chunk(&self) -> &MemoryChunk {
        &self.chunk
    }

    pub fn chunk_mut(&mut self) -> &mut MemoryChunk {
        &mut self.chunk
    }

    pub fn size(&self) -> u32 {
        self.chunk.size() as u32
    }

    pub fn position(&self) -> u32 {
        self.position
    }

    pub fn remaining_from(&self, pos: u32) -> u32 {
        self.size().saturating_sub(pos.saturating_sub(self.position))
    }

    pub fn file_offset(&self) -> u64 {
        self.file_offset
    }

    pub fn set_file_offset(&mut self, offset: u64) {
        self.file_offset = offset;
    }

    pub fn is_incore(&self, pos: u32, length: u32) -> Result<bool, StorageError> {
        let rem = self.remaining_from(pos);
        let len = length.min(rem);
        if pos < self.position || pos > self.position + self.size() {
            return Err(StorageError::InvalidRange {
                offset: pos as u64,
                length: len as u64,
                size: self.size() as u64,
            });
        }
        let rel_pos = pos - self.position;
        if len > self.size() || rel_pos + len > self.size() {
            return Err(StorageError::InvalidRange {
                offset: rel_pos as u64,
                length: len as u64,
                size: self.size() as u64,
            });
        }
        self.chunk.is_incore(rel_pos as usize, len as usize)
    }

    pub fn incore_length(&self, pos: u32, length: u32) -> Result<u32, StorageError> {
        let rem = self.remaining_from(pos);
        let len = length.min(rem);
        if pos < self.position || pos >= self.position + self.size() {
            return Err(StorageError::InvalidRange {
                offset: pos as u64,
                length: len as u64,
                size: self.size() as u64,
            });
        }
        let rel_pos = pos - self.position;
        let touched = self.chunk.pages_touched(rel_pos as usize, len as usize);
        let mut buf = vec![0u8; touched];
        self.chunk.incore(&mut buf, rel_pos as usize, len as usize)?;

        let mut dist = 0;
        for &b in &buf {
            if (b & 1) != 0 || b != 0 {
                dist += 1;
            } else {
                break;
            }
        }

        let ps = MemoryChunk::page_size() as u32;
        let pa = self.chunk.page_align() as u32;
        let incore_len = if dist > 0 {
            (dist * ps).saturating_sub(pa)
        } else {
            0
        };
        Ok(incore_len.min(len))
    }
}
