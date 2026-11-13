use bytes::BytesMut;
use crossbeam_queue::ArrayQueue;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, OnceLock};

pub const DEFAULT_BLOCK_CAPACITY: usize = 16 * 1024; // 16 KiB
pub const DEFAULT_POOL_CAPACITY: usize = 4096; // 64 MiB total pool capacity

/// A lock-free, zero-allocation block pool for 16 KiB BitTorrent blocks.
#[derive(Debug)]
pub struct BlockPool {
    queue: ArrayQueue<BytesMut>,
    block_capacity: usize,
}

impl BlockPool {
    pub fn new(max_blocks: usize, block_capacity: usize) -> Self {
        Self {
            queue: ArrayQueue::new(max_blocks),
            block_capacity,
        }
    }

    /// Acquires a buffer of `block_capacity` bytes. Reuses pooled memory if available,
    /// or allocates a new `BytesMut` with the required capacity.
    pub fn acquire(&self) -> BytesMut {
        if let Some(mut buf) = self.queue.pop() {
            buf.clear();
            buf
        } else {
            BytesMut::with_capacity(self.block_capacity)
        }
    }

    /// Releases a buffer back to the pool.
    pub fn release(&self, mut buf: BytesMut) {
        if buf.capacity() >= self.block_capacity {
            buf.clear();
            let _ = self.queue.push(buf);
        }
    }

    /// Number of free buffers currently sitting in the pool.
    pub fn available(&self) -> usize {
        self.queue.len()
    }

    /// Drop idle buffers to release at least `bytes` of memory back to the OS.
    /// Returns the number of bytes actually freed. Used by the memory manager
    /// when accounting pressure asks for unused chunks to be reclaimed.
    pub fn free_bytes(&self, bytes: u64) -> u64 {
        let mut freed = 0u64;
        while freed < bytes {
            match self.queue.pop() {
                Some(buf) => {
                    freed += buf.capacity() as u64;
                    drop(buf);
                }
                None => break,
            }
        }
        freed
    }

    /// Global singleton pool shared across tasks.
    pub fn global() -> &'static Arc<BlockPool> {
        static GLOBAL_POOL: OnceLock<Arc<BlockPool>> = OnceLock::new();
        GLOBAL_POOL.get_or_init(|| {
            let pool = Arc::new(BlockPool::new(
                DEFAULT_POOL_CAPACITY,
                DEFAULT_BLOCK_CAPACITY,
            ));
            let hook_pool = pool.clone();
            lt_storage::memory_manager::set_free_memory_hook(Arc::new(move |bytes| {
                hook_pool.free_bytes(bytes)
            }));
            pool
        })
    }
}

/// A RAII guard for a pooled block that returns the buffer to the pool on drop.
pub struct PooledBlock {
    buf: Option<BytesMut>,
    pool: Arc<BlockPool>,
}

impl PooledBlock {
    pub fn new(pool: Arc<BlockPool>) -> Self {
        let buf = pool.acquire();
        Self {
            buf: Some(buf),
            pool,
        }
    }

    pub fn from_global() -> Self {
        Self::new(BlockPool::global().clone())
    }

    pub fn into_inner(mut self) -> BytesMut {
        self.buf.take().expect("PooledBlock consumed")
    }

    pub fn as_bytes_mut(&mut self) -> &mut BytesMut {
        self.buf.as_mut().expect("PooledBlock not consumed")
    }
}

impl Deref for PooledBlock {
    type Target = BytesMut;

    fn deref(&self) -> &Self::Target {
        self.buf.as_ref().expect("PooledBlock not consumed")
    }
}

impl DerefMut for PooledBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buf.as_mut().expect("PooledBlock not consumed")
    }
}

impl Drop for PooledBlock {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            self.pool.release(buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_pool_acquire_and_release() {
        let pool = BlockPool::new(4, DEFAULT_BLOCK_CAPACITY);
        assert_eq!(pool.available(), 0);

        let buf1 = pool.acquire();
        assert!(buf1.capacity() >= DEFAULT_BLOCK_CAPACITY);
        pool.release(buf1);
        assert_eq!(pool.available(), 1);

        let buf2 = pool.acquire();
        assert_eq!(pool.available(), 0);
        pool.release(buf2);
        assert_eq!(pool.available(), 1);
    }

    #[test]
    fn test_pooled_block_raii() {
        let pool = Arc::new(BlockPool::new(2, DEFAULT_BLOCK_CAPACITY));
        assert_eq!(pool.available(), 0);
        {
            let mut block = PooledBlock::new(pool.clone());
            block.extend_from_slice(b"hello 16kb block");
            assert_eq!(&block[..], b"hello 16kb block");
        }
        assert_eq!(pool.available(), 1);
    }
}
