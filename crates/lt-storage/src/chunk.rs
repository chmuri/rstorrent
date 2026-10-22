use crate::chunk_part::{ChunkPart, MappedType};
use crate::error::StorageError;
use crate::memory_chunk::MemoryChunk;
use crate::mmap_safe::{Advice, Prot, SyncFlags};

#[derive(Debug, Default)]
pub struct Chunk {
    parts: Vec<ChunkPart>,
    chunk_size: u32,
    prot: Prot,
}

impl Chunk {
    pub fn new() -> Self {
        Self {
            parts: Vec::new(),
            chunk_size: 0,
            prot: Prot::all(),
        }
    }

    pub fn is_readable(&self) -> bool {
        self.prot.contains(Prot::READ)
    }

    pub fn is_writable(&self) -> bool {
        self.prot.contains(Prot::WRITE)
    }

    pub fn chunk_size(&self) -> u32 {
        self.chunk_size
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    pub fn parts(&self) -> &[ChunkPart] {
        &self.parts
    }

    pub fn parts_mut(&mut self) -> &mut [ChunkPart] {
        &mut self.parts
    }

    pub fn clear(&mut self) {
        for part in &mut self.parts {
            part.clear();
        }
        self.parts.clear();
        self.chunk_size = 0;
        self.prot = Prot::all();
    }

    pub fn push_back(&mut self, mapped: MappedType, chunk: MemoryChunk) {
        self.prot &= chunk.prot();
        let part_size = chunk.size() as u32;
        let part = ChunkPart::new(mapped, chunk, self.chunk_size);
        self.parts.push(part);
        self.chunk_size += part_size;
    }

    pub fn at_position(&self, pos: u32) -> Result<usize, StorageError> {
        if pos >= self.chunk_size {
            return Err(StorageError::InvalidRange {
                offset: pos as u64,
                length: 0,
                size: self.chunk_size as u64,
            });
        }

        for (idx, part) in self.parts.iter().enumerate() {
            if part.is_contained(pos) {
                if part.size() == 0 {
                    return Err(StorageError::InvalidChunk);
                }
                return Ok(idx);
            }
        }

        Err(StorageError::InvalidRange {
            offset: pos as u64,
            length: 0,
            size: self.chunk_size as u64,
        })
    }

    pub fn at_memory(&self, offset: u32, part_idx: usize) -> Result<(*mut u8, u32), StorageError> {
        let part = self.parts.get(part_idx).ok_or(StorageError::InvalidChunk)?;
        if !part.chunk().is_valid() {
            return Err(StorageError::InvalidChunk);
        }

        if offset < part.position() || offset >= part.position() + part.size() {
            return Err(StorageError::InvalidRange {
                offset: offset as u64,
                length: 0,
                size: part.size() as u64,
            });
        }

        let rel_offset = (offset - part.position()) as usize;
        let remaining = part.size() - (rel_offset as u32);
        let ptr = unsafe { part.chunk().begin().add(rel_offset) };
        Ok((ptr, remaining))
    }

    pub fn find_address(&self, ptr: *const u8) -> Option<usize> {
        self.parts.iter().position(|p| p.has_address(ptr))
    }

    pub fn is_incore(&self, mut pos: u32, length: u32) -> Result<bool, StorageError> {
        if self.is_empty() || pos >= self.chunk_size {
            return Err(StorageError::InvalidRange {
                offset: pos as u64,
                length: length as u64,
                size: self.chunk_size as u64,
            });
        }

        let mut part_idx = self.at_position(pos)?;
        let last = pos.saturating_add(length.min(self.chunk_size - pos));

        while part_idx < self.parts.len() {
            let part = &self.parts[part_idx];
            if !part.is_incore(pos, last - pos)? {
                return Ok(false);
            }

            part_idx += 1;
            if part_idx >= self.parts.len() || self.parts[part_idx].position() >= last {
                return Ok(true);
            }
            pos = self.parts[part_idx].position();
        }

        Ok(true)
    }

    pub fn incore_length(&self, mut pos: u32, length: u32) -> Result<u32, StorageError> {
        if self.is_empty() || pos >= self.chunk_size {
            return Err(StorageError::InvalidRange {
                offset: pos as u64,
                length: length as u64,
                size: self.chunk_size as u64,
            });
        }

        let mut part_idx = self.at_position(pos)?;
        let mut total = 0;
        let mut remaining = length.min(self.chunk_size - pos);

        while part_idx < self.parts.len() && remaining > 0 {
            let part = &self.parts[part_idx];
            let incore_len = part.incore_length(pos, remaining)?;
            if incore_len > remaining {
                return Err(StorageError::InvalidRange {
                    offset: pos as u64,
                    length: incore_len as u64,
                    size: remaining as u64,
                });
            }

            pos += incore_len;
            remaining -= incore_len;
            total += incore_len;

            if pos == part.position() + part.size() {
                part_idx += 1;
            } else {
                break;
            }
        }

        Ok(total)
    }

    pub fn sync(&self, flags: SyncFlags) -> Result<bool, StorageError> {
        let mut success = true;
        for part in &self.parts {
            if part.chunk().sync(0, part.chunk().size(), flags).is_err() {
                success = false;
            }
        }
        Ok(success)
    }

    pub fn preload(
        &self,
        position: u32,
        length: u32,
        use_advise: bool,
    ) -> Result<(), StorageError> {
        if position >= self.chunk_size {
            return Err(StorageError::InvalidRange {
                offset: position as u64,
                length: length as u64,
                size: self.chunk_size as u64,
            });
        }

        if length == 0 {
            return Ok(());
        }

        let end_pos = position.saturating_add(length.min(self.chunk_size - position));
        let mut curr_pos = position;

        while curr_pos < end_pos {
            let part_idx = self.at_position(curr_pos)?;
            let part = &self.parts[part_idx];
            let part_start = part.position();
            let part_len = part.size();
            let offset_in_part = curr_pos - part_start;
            let to_preload = (part_len - offset_in_part).min(end_pos - curr_pos);

            if use_advise {
                part.chunk().advise(
                    offset_in_part as usize,
                    to_preload as usize,
                    Advice::WillNeed,
                )?;
            } else {
                let slice = part.chunk().as_slice();
                let seg = &slice[offset_in_part as usize..(offset_in_part + to_preload) as usize];
                let mut p = 0;
                while p < seg.len() {
                    let _ = unsafe { std::ptr::read_volatile(&seg[p]) };
                    p += 4096;
                }
                if !seg.is_empty() {
                    let _ = unsafe { std::ptr::read_volatile(&seg[seg.len() - 1]) };
                }
            }

            curr_pos += to_preload;
        }

        Ok(())
    }

    fn check_bounds(chunk_size: u32, position: u32, length: u32) -> Result<(), StorageError> {
        let pos_u64 = position as u64;
        let len_u64 = length as u64;
        let size_u64 = chunk_size as u64;

        if pos_u64 > size_u64 || len_u64 > size_u64 - pos_u64 {
            return Err(StorageError::InvalidRange {
                offset: pos_u64,
                length: len_u64,
                size: size_u64,
            });
        }
        Ok(())
    }

    pub fn to_buffer(
        &self,
        buffer: &mut [u8],
        position: u32,
        length: u32,
    ) -> Result<(), StorageError> {
        Self::check_bounds(self.chunk_size, position, length)?;
        if length == 0 {
            return Ok(());
        }

        if buffer.len() < length as usize {
            return Err(StorageError::InvalidRange {
                offset: 0,
                length: length as u64,
                size: buffer.len() as u64,
            });
        }

        let mut remaining = length as usize;
        let mut curr_pos = position as usize;
        let mut buf_offset = 0;

        for part in &self.parts {
            if remaining == 0 {
                break;
            }
            let part_start = part.position() as usize;
            let part_len = part.size() as usize;
            let part_end = part_start + part_len;

            if curr_pos >= part_start && curr_pos < part_end {
                let offset_in_part = curr_pos - part_start;
                let to_copy = remaining.min(part_len - offset_in_part);
                let slice = part.chunk().as_slice();
                buffer[buf_offset..buf_offset + to_copy]
                    .copy_from_slice(&slice[offset_in_part..offset_in_part + to_copy]);

                buf_offset += to_copy;
                curr_pos += to_copy;
                remaining -= to_copy;
            }
        }

        Ok(())
    }

    pub fn from_buffer(
        &mut self,
        buffer: &[u8],
        position: u32,
        length: u32,
    ) -> Result<(), StorageError> {
        Self::check_bounds(self.chunk_size, position, length)?;
        if length == 0 {
            return Ok(());
        }

        if buffer.len() < length as usize {
            return Err(StorageError::InvalidRange {
                offset: 0,
                length: length as u64,
                size: buffer.len() as u64,
            });
        }

        let mut remaining = length as usize;
        let mut curr_pos = position as usize;
        let mut buf_offset = 0;

        for part in &mut self.parts {
            if remaining == 0 {
                break;
            }
            let part_start = part.position() as usize;
            let part_len = part.size() as usize;
            let part_end = part_start + part_len;

            if curr_pos >= part_start && curr_pos < part_end {
                let offset_in_part = curr_pos - part_start;
                let to_copy = remaining.min(part_len - offset_in_part);
                let slice = part.chunk_mut().as_mut_slice();
                slice[offset_in_part..offset_in_part + to_copy]
                    .copy_from_slice(&buffer[buf_offset..buf_offset + to_copy]);

                buf_offset += to_copy;
                curr_pos += to_copy;
                remaining -= to_copy;
            }
        }

        Ok(())
    }

    pub fn compare_buffer(
        &self,
        buffer: &[u8],
        position: u32,
        length: u32,
    ) -> Result<bool, StorageError> {
        Self::check_bounds(self.chunk_size, position, length)?;
        if length == 0 {
            return Ok(true);
        }

        if buffer.len() < length as usize {
            return Err(StorageError::InvalidRange {
                offset: 0,
                length: length as u64,
                size: buffer.len() as u64,
            });
        }

        let mut remaining = length as usize;
        let mut curr_pos = position as usize;
        let mut buf_offset = 0;

        for part in &self.parts {
            if remaining == 0 {
                break;
            }
            let part_start = part.position() as usize;
            let part_len = part.size() as usize;
            let part_end = part_start + part_len;

            if curr_pos >= part_start && curr_pos < part_end {
                let offset_in_part = curr_pos - part_start;
                let to_copy = remaining.min(part_len - offset_in_part);
                let slice = part.chunk().as_slice();
                if slice[offset_in_part..offset_in_part + to_copy]
                    != buffer[buf_offset..buf_offset + to_copy]
                {
                    return Ok(false);
                }

                buf_offset += to_copy;
                curr_pos += to_copy;
                remaining -= to_copy;
            }
        }

        Ok(true)
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        self.clear();
    }
}
