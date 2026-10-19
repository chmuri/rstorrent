use std::ptr;
use std::sync::Arc;

use crate::error::StorageError;
use crate::mmap_safe::{system_page_size, Advice, MappedMemory, Prot, SyncFlags};

#[derive(Clone, Debug, Default)]
pub struct MemoryChunk {
    inner: Option<Arc<MappedMemory>>,
    page_align: usize,
    length: usize,
    prot: Prot,
}

impl MemoryChunk {
    /// Creates an empty invalid MemoryChunk
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct MemoryChunk wrapping a MappedMemory allocation
    pub fn from_mapped(
        inner: Arc<MappedMemory>,
        page_align: usize,
        length: usize,
        prot: Prot,
    ) -> Result<Self, StorageError> {
        let page_size = system_page_size();
        if page_align >= page_size {
            return Err(StorageError::AlignmentError {
                page_size,
                align: page_align,
            });
        }

        if page_align + length > inner.len() {
            return Err(StorageError::InvalidRange {
                offset: page_align as u64,
                length: length as u64,
                size: inner.len() as u64,
            });
        }

        Ok(Self {
            inner: Some(inner),
            page_align,
            length,
            prot,
        })
    }

    /// Helper constructor for tests or in-memory static chunks
    pub fn from_vec(vec: Vec<u8>, prot: Prot) -> Self {
        let length = vec.len();
        let inner = Arc::new(MappedMemory::from_vec(vec));
        Self {
            inner: Some(inner),
            page_align: 0,
            length,
            prot,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.inner.is_some() && self.length > 0
    }

    pub fn is_readable(&self) -> bool {
        self.prot.contains(Prot::READ)
    }

    pub fn is_writable(&self) -> bool {
        self.prot.contains(Prot::WRITE)
    }

    pub fn is_exec(&self) -> bool {
        self.prot.contains(Prot::EXEC)
    }

    pub fn has_permissions(&self, required: Prot) -> bool {
        self.prot.contains(required)
    }

    pub fn prot(&self) -> Prot {
        self.prot
    }

    pub fn size(&self) -> usize {
        self.length
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub fn page_align(&self) -> usize {
        self.page_align
    }

    pub fn page_align_offset(&self, offset: usize) -> usize {
        (offset + self.page_align) % Self::page_size()
    }

    pub fn page_size() -> usize {
        system_page_size()
    }

    pub fn is_valid_range(&self, offset: usize, length: usize) -> bool {
        length != 0 && offset.saturating_add(length) <= self.length
    }

    pub fn pages_touched(&self, offset: usize, length: usize) -> usize {
        if length == 0 {
            return 0;
        }
        let ps = Self::page_size();
        (length + self.page_align_offset(offset) + ps - 1) / ps
    }

    pub fn size_aligned(&self) -> usize {
        let total = self.page_align + self.length;
        if total == 0 {
            return 0;
        }
        let ps = Self::page_size();
        ((total + ps - 1) / ps) * ps
    }

    pub fn clear(&mut self) {
        self.inner = None;
        self.page_align = 0;
        self.length = 0;
        self.prot = Prot::NONE;
    }

    pub fn unmap(&mut self) {
        self.clear();
    }

    pub fn ptr(&self) -> *mut u8 {
        self.inner
            .as_ref()
            .map(|i| i.ptr())
            .unwrap_or(ptr::null_mut())
    }

    pub fn begin(&self) -> *mut u8 {
        let base = self.ptr();
        if base.is_null() {
            ptr::null_mut()
        } else {
            unsafe { base.add(self.page_align) }
        }
    }

    pub fn end(&self) -> *mut u8 {
        let beg = self.begin();
        if beg.is_null() {
            ptr::null_mut()
        } else {
            unsafe { beg.add(self.length) }
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        if !self.is_valid() {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.begin(), self.length) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        if !self.is_valid() || !self.is_writable() {
            return &mut [];
        }
        unsafe { std::slice::from_raw_parts_mut(self.begin(), self.length) }
    }

    pub fn align_pair(&self, offset: &mut usize, length: &mut usize) {
        let ps = Self::page_size();
        *offset += self.page_align;
        let rem = *offset % ps;
        *length += rem;
        *offset -= rem;
    }

    pub fn incore(&self, buf: &mut [u8], mut offset: usize, mut length: usize) -> Result<(), StorageError> {
        let inner = self.inner.as_ref().ok_or(StorageError::InvalidChunk)?;
        if !self.is_valid_range(offset, length) {
            return Err(StorageError::InvalidRange {
                offset: offset as u64,
                length: length as u64,
                size: self.length as u64,
            });
        }

        self.align_pair(&mut offset, &mut length);
        inner.mincore(offset, length, buf)
    }

    pub fn is_incore(&self, offset: usize, length: usize) -> Result<bool, StorageError> {
        let touched = self.pages_touched(offset, length);
        let mut buf = vec![0u8; touched];
        self.incore(&mut buf, offset, length)?;
        Ok(buf.iter().all(|&b| (b & 1) != 0 || b != 0))
    }

    pub fn advise(&self, mut offset: usize, mut length: usize, advice: Advice) -> Result<(), StorageError> {
        let inner = self.inner.as_ref().ok_or(StorageError::InvalidChunk)?;
        if !self.is_valid_range(offset, length) {
            return Err(StorageError::InvalidRange {
                offset: offset as u64,
                length: length as u64,
                size: self.length as u64,
            });
        }

        self.align_pair(&mut offset, &mut length);
        inner.madvise(offset, length, advice)
    }

    pub fn sync(&self, mut offset: usize, mut length: usize, flags: SyncFlags) -> Result<(), StorageError> {
        let inner = self.inner.as_ref().ok_or(StorageError::InvalidChunk)?;
        if !self.is_valid_range(offset, length) {
            return Err(StorageError::InvalidRange {
                offset: offset as u64,
                length: length as u64,
                size: self.length as u64,
            });
        }

        self.align_pair(&mut offset, &mut length);
        inner.msync(offset, length, flags)
    }
}
