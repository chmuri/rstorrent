use bitflags::bitflags;
use std::ffi::CString;
use std::os::unix::io::RawFd;
use std::path::Path;
use std::sync::Arc;

use crate::error::StorageError;
use crate::memory_chunk::MemoryChunk;
use crate::mmap_safe::{allocate_file, system_page_size, MapFlags, MappedMemory, Prot};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct OpenFlags: i32 {
        const CREATE = libc::O_CREAT;
        const TRUNCATE = libc::O_TRUNC;
        const NONBLOCK = libc::O_NONBLOCK;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct FallocateFlags: i32 {
        const NONE = 0;
        const FALLOCATE = 1 << 0;
        const BLOCKING = 1 << 1;
    }
}

pub const INVALID_FD: RawFd = -1;

#[derive(Debug)]
pub struct SocketFile {
    fd: RawFd,
}

impl Default for SocketFile {
    fn default() -> Self {
        Self { fd: INVALID_FD }
    }
}

impl SocketFile {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_fd(fd: RawFd) -> Self {
        Self { fd }
    }

    pub fn is_open(&self) -> bool {
        self.fd != INVALID_FD
    }

    pub fn fd(&self) -> RawFd {
        self.fd
    }

    pub fn open<P: AsRef<Path>>(
        &mut self,
        path: P,
        prot: Prot,
        flags: OpenFlags,
        mode: Option<u32>,
    ) -> Result<(), StorageError> {
        self.close();

        let mut access_flag = 0;
        if prot.contains(Prot::READ) && prot.contains(Prot::WRITE) {
            access_flag |= libc::O_RDWR;
        } else if prot.contains(Prot::READ) {
            access_flag |= libc::O_RDONLY;
        } else if prot.contains(Prot::WRITE) {
            access_flag |= libc::O_WRONLY;
        } else {
            return Err(StorageError::NoProtectionFlags);
        }

        let full_flags = access_flag | flags.bits();
        let file_mode = mode.unwrap_or(0o666) as libc::mode_t;

        let c_path = CString::new(path.as_ref().to_string_lossy().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

        let ret = unsafe { libc::open(c_path.as_ptr(), full_flags, file_mode) };
        if ret == INVALID_FD {
            return Err(StorageError::Io(std::io::Error::last_os_error()));
        }

        self.fd = ret;
        Ok(())
    }

    pub fn close(&mut self) {
        if self.is_open() {
            unsafe {
                libc::close(self.fd);
            }
            self.fd = INVALID_FD;
        }
    }

    pub fn size(&self) -> Result<u64, StorageError> {
        if !self.is_open() {
            return Err(StorageError::FileNotOpen);
        }

        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::fstat(self.fd, &mut stat) };
        if ret != 0 {
            return Err(StorageError::Io(std::io::Error::last_os_error()));
        }

        Ok(stat.st_size as u64)
    }

    pub fn set_size(&self, size: u64) -> Result<(), StorageError> {
        if !self.is_open() {
            return Err(StorageError::FileNotOpen);
        }

        let ret = unsafe { libc::ftruncate(self.fd, size as libc::off_t) };
        if ret != 0 {
            return Err(StorageError::Io(std::io::Error::last_os_error()));
        }

        Ok(())
    }

    pub fn allocate(&self, size: u64, _flags: FallocateFlags) -> Result<(), StorageError> {
        if !self.is_open() {
            return Err(StorageError::FileNotOpen);
        }

        allocate_file(self.fd, size)
    }

    pub fn create_padding_chunk(
        length: u32,
        prot: Prot,
        flags: MapFlags,
    ) -> Result<MemoryChunk, StorageError> {
        if length == 0 {
            return Ok(MemoryChunk::new());
        }

        let inner = MappedMemory::map_anon(length as usize, prot, flags)?;
        MemoryChunk::from_mapped(Arc::new(inner), 0, length as usize, prot)
    }

    pub fn create_chunk(
        &self,
        offset: u64,
        length: u32,
        prot: Prot,
        flags: MapFlags,
    ) -> Result<MemoryChunk, StorageError> {
        if !self.is_open() {
            return Err(StorageError::FileNotOpen);
        }

        let current_size = self.size()?;
        if length == 0 || offset > current_size || offset.saturating_add(length as u64) > current_size {
            return Ok(MemoryChunk::new());
        }

        let page_size = system_page_size();
        let align = (offset % (page_size as u64)) as usize;

        let map_offset = (offset - (align as u64)) as i64;
        let map_len = (length as usize) + align;

        let inner = MappedMemory::map(self.fd, map_offset, map_len, prot, flags)?;
        MemoryChunk::from_mapped(Arc::new(inner), align, length as usize, prot)
    }
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        self.close();
    }
}
