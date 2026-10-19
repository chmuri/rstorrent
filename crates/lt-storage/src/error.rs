use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("File is not open")]
    FileNotOpen,

    #[error("File opened with no protection flags")]
    NoProtectionFlags,

    #[error("Invalid range: offset {offset}, length {length}, file/chunk size {size}")]
    InvalidRange {
        offset: u64,
        length: u64,
        size: u64,
    },

    #[error("MemoryChunk alignment error: page size {page_size}, align {align}")]
    AlignmentError { page_size: usize, align: usize },

    #[error("Null pointer received for MemoryChunk")]
    NullPointer,

    #[error("Invalid MemoryChunk object")]
    InvalidChunk,

    #[error("System call fallocate failed: {0}")]
    FallocateFailed(String),

    #[error("System call mmap failed: {0}")]
    MmapFailed(String),

    #[error("System call munmap failed: {0}")]
    MunmapFailed(String),

    #[error("System call mincore failed: {0}")]
    MincoreFailed(String),

    #[error("System call madvise failed: {0}")]
    MadviseFailed(String),

    #[error("System call msync failed: {0}")]
    MsyncFailed(String),

    #[error("Disk thread queue is full; caller must apply backpressure")]
    DiskQueueFull,

    #[error("Disk thread is not running")]
    DiskThreadClosed,
}
