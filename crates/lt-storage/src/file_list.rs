use lt_common::bitfield::Bitfield;
use std::path::{Path, PathBuf};

use crate::chunk::Chunk;
use crate::chunk_part::{ChunkPart, MappedType};
use crate::error::StorageError;
use crate::file::File;
use crate::memory_chunk::MemoryChunk;
use crate::mmap_safe::{MapFlags, Prot};
use crate::socket_file::SocketFile;

pub const MAX_CHUNKS: u64 = 1 << 22;

#[derive(Debug, Default)]
pub struct FileList {
    files: Vec<File>,
    is_open: bool,
    is_multi_file: bool,
    torrent_size: u64,
    chunk_size: u32,
    max_file_size: u64,
    bitfield: Bitfield,
    root_dir: PathBuf,
}

impl FileList {
    pub fn new() -> Self {
        Self {
            max_file_size: u64::MAX,
            ..Default::default()
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn is_multi_file(&self) -> bool {
        self.is_multi_file
    }

    pub fn set_multi_file(&mut self, state: bool) {
        self.is_multi_file = state;
    }

    pub fn size_files(&self) -> usize {
        self.files.len()
    }

    pub fn files(&self) -> &[File] {
        &self.files
    }

    /// Depth-aware iterator over the file entries.
    pub fn iter(&self) -> crate::file_list_iterator::FileListIterator<'_> {
        crate::file_list_iterator::FileListIterator::new(&self.files)
    }

    pub fn files_mut(&mut self) -> &mut [File] {
        &mut self.files
    }

    pub fn file(&self, index: usize) -> Option<&File> {
        self.files.get(index)
    }

    pub fn file_mut(&mut self, index: usize) -> Option<&mut File> {
        self.files.get_mut(index)
    }

    pub fn size_bytes(&self) -> u64 {
        self.torrent_size
    }

    pub fn chunk_size(&self) -> u32 {
        self.chunk_size
    }

    pub fn size_chunks(&self) -> u32 {
        self.bitfield.size_bits() as u32
    }

    pub fn completed_chunks(&self) -> u32 {
        self.bitfield.size_set() as u32
    }

    pub fn is_done(&self) -> bool {
        self.size_chunks() > 0 && self.completed_chunks() == self.size_chunks()
    }

    pub fn bitfield(&self) -> &Bitfield {
        &self.bitfield
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn set_root_dir<P: AsRef<Path>>(&mut self, path: P) {
        self.root_dir = path.as_ref().to_path_buf();
    }

    pub fn max_file_size(&self) -> u64 {
        self.max_file_size
    }

    pub fn set_max_file_size(&mut self, size: u64) {
        self.max_file_size = size;
    }

    pub fn push_file(&mut self, file: File) {
        self.files.push(file);
    }

    pub fn initialize(&mut self, torrent_size: u64, chunk_size: u32) -> Result<(), StorageError> {
        if chunk_size == 0 {
            return Err(StorageError::InvalidRange {
                offset: 0,
                length: 0,
                size: torrent_size,
            });
        }

        let sum = torrent_size.checked_add((chunk_size - 1) as u64).ok_or(
            StorageError::InvalidRange {
                offset: torrent_size,
                length: chunk_size as u64,
                size: torrent_size,
            },
        )?;

        let chunks = if torrent_size == 0 {
            0
        } else {
            sum / (chunk_size as u64)
        };

        if chunks > MAX_CHUNKS {
            return Err(StorageError::InvalidRange {
                offset: torrent_size,
                length: chunk_size as u64,
                size: torrent_size,
            });
        }

        self.torrent_size = torrent_size;
        self.chunk_size = chunk_size;
        self.bitfield = Bitfield::new(chunks as usize);

        for file in &mut self.files {
            file.calculate_range(chunk_size);
        }

        Ok(())
    }

    /// Clears every completion marker so a recheck can rebuild the bitfield.
    pub fn reset_completion(&mut self) {
        self.bitfield = Bitfield::new(self.bitfield.size_bits());
        for file in &mut self.files {
            file.set_completed_chunks(0);
        }
    }

    pub fn mark_completed(&mut self, index: u32) {
        let idx = index as usize;
        if idx < self.bitfield.size_bits() && !self.bitfield.get(idx) {
            self.bitfield.set(idx);
            for file in &mut self.files {
                let (first, last) = file.range();
                if index >= first && index < last {
                    file.inc_completed_chunks();
                }
            }
        }
    }

    pub fn create_chunk_index(&self, index: u32, prot: Prot) -> Result<Chunk, StorageError> {
        if index >= self.size_chunks() {
            return Err(StorageError::InvalidRange {
                offset: (index as u64) * (self.chunk_size as u64),
                length: self.chunk_size as u64,
                size: self.torrent_size,
            });
        }

        let chunk_start = (index as u64) * (self.chunk_size as u64);
        let chunk_len =
            (self.chunk_size as u64).min(self.torrent_size.saturating_sub(chunk_start)) as u32;

        let mut chunk = Chunk::new();

        for file in &self.files {
            if file.size_bytes() == 0 {
                continue;
            }

            let file_start = file.offset();
            let file_end = file_start + file.size_bytes();

            if chunk_start < file_end && (chunk_start + (chunk_len as u64)) > file_start {
                let overlap_start = chunk_start.max(file_start);
                let overlap_end = (chunk_start + (chunk_len as u64)).min(file_end);
                let overlap_len = (overlap_end - overlap_start) as u32;

                let file_offset = overlap_start - file_start;

                let mem_chunk = if file.is_padding() {
                    SocketFile::create_padding_chunk(overlap_len, prot, MapFlags::SHARED)?
                } else if file.is_open() {
                    let socket_file = SocketFile::from_fd(file.file_descriptor());
                    let mc = socket_file.create_chunk(
                        file_offset,
                        overlap_len,
                        prot,
                        MapFlags::SHARED,
                    )?;
                    std::mem::forget(socket_file); // Don't close file fd here
                    mc
                } else {
                    // In-memory mock chunk if file fd is not open (e.g. for standalone tests)
                    MemoryChunk::from_vec(vec![0u8; overlap_len as usize], prot)
                };

                let mut part = ChunkPart::new(MappedType::Mmap, mem_chunk, 0);
                part.set_file_offset(file_offset);
                chunk.push_back(MappedType::Mmap, part.chunk().clone());
            }
        }

        Ok(chunk)
    }
}
