use bitflags::bitflags;
use std::os::unix::io::RawFd;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::StorageError;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Prot: i32 {
        const NONE = libc::PROT_NONE;
        const READ = libc::PROT_READ;
        const WRITE = libc::PROT_WRITE;
        const EXEC = libc::PROT_EXEC;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct MapFlags: i32 {
        const SHARED = libc::MAP_SHARED;
        const PRIVATE = libc::MAP_PRIVATE;
        const ANON = libc::MAP_ANONYMOUS;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct SyncFlags: i32 {
        const SYNC = libc::MS_SYNC;
        const ASYNC = libc::MS_ASYNC;
        const INVALIDATE = libc::MS_INVALIDATE;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advice {
    Normal,
    Random,
    Sequential,
    WillNeed,
    DontNeed,
}

impl Advice {
    pub fn to_libc(self) -> i32 {
        match self {
            Advice::Normal => libc::MADV_NORMAL,
            Advice::Random => libc::MADV_RANDOM,
            Advice::Sequential => libc::MADV_SEQUENTIAL,
            Advice::WillNeed => libc::MADV_WILLNEED,
            Advice::DontNeed => libc::MADV_DONTNEED,
        }
    }
}

static CACHED_PAGE_SIZE: AtomicUsize = AtomicUsize::new(0);

pub fn system_page_size() -> usize {
    let cached = CACHED_PAGE_SIZE.load(Ordering::Relaxed);
    if cached != 0 {
        return cached;
    }
    let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let size = if ps > 0 { ps as usize } else { 4096 };
    CACHED_PAGE_SIZE.store(size, Ordering::Relaxed);
    size
}

/// Low-level mapped memory backing.
/// Manages virtual memory allocations via `mmap` / `munmap` or heap allocations.
#[derive(Debug)]
pub struct MappedMemory {
    ptr: *mut u8,
    len: usize,
    is_mmap: bool,
}

// SAFETY: `MappedMemory` is the sole owner of a single contiguous allocation —
// either an `mmap` region (`is_mmap == true`) or a heap buffer produced by
// `from_vec` (`is_mmap == false`, the `Box` is `forget`ten and reclaimed in
// `Drop`). The pointer is never aliased by another owner, and `Drop` releases it
// exactly once (nulling `ptr` afterwards), so there is no double-free.
//
// * Send: ownership of the allocation can move between threads; neither the
//   mapping nor the heap buffer carries thread-local state.
// * Sync: every `&self` method (`mincore`, `madvise`, `msync`, `len`, ...) is a
//   read-only query or a kernel syscall on the mapping, all of which are
//   thread-safe. `ptr()` merely *returns* a raw pointer (a safe operation);
//   dereferencing it for writes is `unsafe` and the caller must synchronize
//   concurrent writes, exactly as with `Vec::as_mut_ptr`. The type itself
//   exposes no interior mutability through `&self`, so sharing it is race-free.
unsafe impl Send for MappedMemory {}
unsafe impl Sync for MappedMemory {}

impl MappedMemory {
    /// Maps a region from a file descriptor or anonymous memory.
    pub fn map(
        fd: RawFd,
        offset: i64,
        length: usize,
        prot: Prot,
        flags: MapFlags,
    ) -> Result<Self, StorageError> {
        if length == 0 {
            return Err(StorageError::InvalidRange {
                offset: offset as u64,
                length: 0,
                size: 0,
            });
        }

        let ret = unsafe {
            libc::mmap(
                ptr::null_mut(),
                length,
                prot.bits(),
                flags.bits(),
                fd,
                offset as libc::off_t,
            )
        };

        if ret == libc::MAP_FAILED {
            return Err(StorageError::MmapFailed(
                std::io::Error::last_os_error().to_string(),
            ));
        }

        Ok(Self {
            ptr: ret as *mut u8,
            len: length,
            is_mmap: true,
        })
    }

    /// Creates an anonymous mapped region.
    pub fn map_anon(length: usize, prot: Prot, mut flags: MapFlags) -> Result<Self, StorageError> {
        flags.insert(MapFlags::ANON);
        if !flags.contains(MapFlags::SHARED) && !flags.contains(MapFlags::PRIVATE) {
            flags.insert(MapFlags::PRIVATE);
        }
        Self::map(-1, 0, length, prot, flags)
    }

    /// Creates a mock MappedMemory from a heap-allocated buffer for testing.
    pub fn from_vec(vec: Vec<u8>) -> Self {
        let mut boxed = vec.into_boxed_slice();
        let len = boxed.len();
        let ptr = boxed.as_mut_ptr();
        std::mem::forget(boxed);
        Self {
            ptr,
            len,
            is_mmap: false,
        }
    }

    pub fn ptr(&self) -> *mut u8 {
        self.ptr
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Query page residency in memory using `mincore`.
    pub fn mincore(
        &self,
        offset: usize,
        length: usize,
        vec: &mut [u8],
    ) -> Result<(), StorageError> {
        if !self.is_mmap {
            // Heap buffer is always in core
            vec.fill(1);
            return Ok(());
        }

        if offset.saturating_add(length) > self.len {
            return Err(StorageError::InvalidRange {
                offset: offset as u64,
                length: length as u64,
                size: self.len as u64,
            });
        }

        let target_ptr = unsafe { self.ptr.add(offset) as *mut libc::c_void };
        // Linux takes `*mut c_uchar`; the BSD/macOS signature takes `*mut c_char`.
        #[cfg(target_os = "linux")]
        let resident_ptr = vec.as_mut_ptr();
        #[cfg(not(target_os = "linux"))]
        let resident_ptr = vec.as_mut_ptr() as *mut libc::c_char;
        let ret = unsafe { libc::mincore(target_ptr, length, resident_ptr) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            return Err(StorageError::MincoreFailed(err.to_string()));
        }
        Ok(())
    }

    /// Advise kernel on memory access patterns using `madvise`.
    pub fn madvise(
        &self,
        offset: usize,
        length: usize,
        advice: Advice,
    ) -> Result<(), StorageError> {
        if !self.is_mmap {
            return Ok(());
        }

        if offset.saturating_add(length) > self.len {
            return Err(StorageError::InvalidRange {
                offset: offset as u64,
                length: length as u64,
                size: self.len as u64,
            });
        }

        let target_ptr = unsafe { self.ptr.add(offset) as *mut libc::c_void };
        let ret = unsafe { libc::madvise(target_ptr, length, advice.to_libc()) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            let raw_errno = err.raw_os_error().unwrap_or(0);
            if raw_errno == libc::EINVAL
                || (raw_errno == libc::ENOMEM && advice != Advice::WillNeed)
                || raw_errno == libc::EBADF
            {
                return Err(StorageError::MadviseFailed(err.to_string()));
            }
        }
        Ok(())
    }

    /// Synchronize mapped memory to disk using `msync`.
    pub fn msync(
        &self,
        offset: usize,
        length: usize,
        flags: SyncFlags,
    ) -> Result<(), StorageError> {
        if !self.is_mmap {
            return Ok(());
        }

        if offset.saturating_add(length) > self.len {
            return Err(StorageError::InvalidRange {
                offset: offset as u64,
                length: length as u64,
                size: self.len as u64,
            });
        }

        let target_ptr = unsafe { self.ptr.add(offset) as *mut libc::c_void };
        let ret = unsafe { libc::msync(target_ptr, length, flags.bits()) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            return Err(StorageError::MsyncFailed(err.to_string()));
        }
        Ok(())
    }
}

impl Drop for MappedMemory {
    fn drop(&mut self) {
        if !self.ptr.is_null() && self.len > 0 {
            if self.is_mmap {
                unsafe {
                    libc::munmap(self.ptr as *mut libc::c_void, self.len);
                }
            } else {
                unsafe {
                    let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(self.ptr, self.len));
                }
            }
            self.ptr = ptr::null_mut();
            self.len = 0;
        }
    }
}

/// Safe disk space allocation using `fallocate` or `posix_fallocate`.
pub fn allocate_file(fd: RawFd, size: u64) -> Result<(), StorageError> {
    if size == 0 {
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        // Attempt Linux fallocate (mode 0 = allocate and extend file size)
        let ret = unsafe { libc::fallocate(fd, 0, 0, size as libc::off_t) };
        if ret == 0 {
            return Ok(());
        }

        let err = std::io::Error::last_os_error();
        let raw_errno = err.raw_os_error().unwrap_or(0);

        // If fallocate is unsupported on this filesystem, fallback to posix_fallocate
        if raw_errno == libc::EOPNOTSUPP || raw_errno == libc::ENOSYS {
            let ret_posix = unsafe { libc::posix_fallocate(fd, 0, size as libc::off_t) };
            if ret_posix == 0 {
                return Ok(());
            }
            return Err(StorageError::FallocateFailed(
                std::io::Error::from_raw_os_error(ret_posix).to_string(),
            ));
        }

        Err(StorageError::FallocateFailed(err.to_string()))
    }

    #[cfg(not(target_os = "linux"))]
    {
        // macOS/BSD have no fallocate; extend the file instead (sparse).
        let ret = unsafe { libc::ftruncate(fd, size as libc::off_t) };
        if ret == 0 {
            return Ok(());
        }

        Err(StorageError::FallocateFailed(
            std::io::Error::last_os_error().to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::io::AsRawFd;
    use std::sync::Arc;
    use std::thread;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rtorrent_mmap_unit_{}_{}.bin",
            tag,
            std::process::id()
        ))
    }

    #[test]
    fn from_vec_heap_buffer_queries_and_drop() {
        let m = MappedMemory::from_vec(vec![0u8; 4096]);
        assert_eq!(m.len(), 4096);
        assert!(!m.is_empty());
        assert!(!m.ptr().is_null());

        // Heap buffers are reported as fully resident and advise/sync are no-ops.
        let mut resident = vec![0u8; 1];
        m.mincore(0, 4096, &mut resident).unwrap();
        assert_eq!(resident, vec![1]);
        m.madvise(0, 4096, Advice::Sequential).unwrap();
        m.msync(0, 4096, SyncFlags::SYNC).unwrap();

        unsafe {
            std::ptr::write_bytes(m.ptr(), 0x5a, 4096);
            assert_eq!(*m.ptr(), 0x5a);
            assert_eq!(*m.ptr().add(4095), 0x5a);
        }
        // Drop reclaims the forgotten Box without leaking or double-freeing.
    }

    #[test]
    fn anonymous_mapping_read_write_and_range_guard() {
        let m = MappedMemory::map_anon(4096, Prot::READ | Prot::WRITE, MapFlags::PRIVATE).unwrap();
        assert_eq!(m.len(), 4096);
        unsafe {
            std::ptr::write_bytes(m.ptr(), 0xab, 4096);
            assert_eq!(*m.ptr(), 0xab);
        }
        m.madvise(0, 4096, Advice::Normal).unwrap();

        let pages = 4096usize.div_ceil(system_page_size());
        let mut resident = vec![0u8; pages];
        m.mincore(0, 4096, &mut resident).unwrap();

        // Out-of-bounds queries are rejected before touching the pointer.
        let mut bad = vec![0u8; pages + 1];
        assert!(m.mincore(0, 8192, &mut bad).is_err());
        assert!(m.madvise(0, 8192, Advice::Normal).is_err());
        assert!(m.msync(0, 8192, SyncFlags::SYNC).is_err());
    }

    #[test]
    fn zero_length_mapping_is_rejected() {
        assert!(MappedMemory::map_anon(0, Prot::READ, MapFlags::PRIVATE).is_err());
    }

    #[test]
    fn file_backed_mapping_persists_writes_via_msync() {
        let path = temp_path("file");
        let _ = fs::remove_file(&path);
        let original = vec![0x11u8; 8192];
        fs::write(&path, &original).unwrap();

        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let m = MappedMemory::map(
            file.as_raw_fd(),
            0,
            8192,
            Prot::READ | Prot::WRITE,
            MapFlags::SHARED,
        )
        .unwrap();

        assert_eq!(
            unsafe { std::slice::from_raw_parts(m.ptr(), 8192) },
            &original[..]
        );
        unsafe {
            *m.ptr() = 0x22;
        }
        m.msync(0, 8192, SyncFlags::SYNC).unwrap();

        let pages = 8192usize.div_ceil(system_page_size());
        let mut resident = vec![0u8; pages];
        m.mincore(0, 8192, &mut resident).unwrap();
        m.madvise(0, 8192, Advice::WillNeed).unwrap();

        drop(m);
        let after = fs::read(&path).unwrap();
        assert_eq!(after[0], 0x22);
        assert_eq!(&after[1..], &original[1..]);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn mapped_memory_is_send() {
        let m = MappedMemory::from_vec(vec![1, 2, 3, 4]);
        let handle = thread::spawn(move || {
            assert_eq!(m.len(), 4);
            unsafe { *m.ptr() }
        });
        assert_eq!(handle.join().unwrap(), 1);
    }

    #[test]
    fn mapped_memory_is_sync() {
        let m = Arc::new(MappedMemory::from_vec(vec![9u8; 16]));
        let shared = Arc::clone(&m);
        let handle = thread::spawn(move || shared.len());
        assert_eq!(handle.join().unwrap(), 16);
        assert_eq!(m.len(), 16);
    }
}
