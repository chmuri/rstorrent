use crate::memory_manager::{memory_manager, MemoryManager};
use std::sync::atomic::{AtomicU64, Ordering};

pub const ALLOCATE_REVERT_LOG: i32 = 1 << 0;
pub const ALLOCATE_DONT_LOG: i32 = 1 << 1;

#[derive(Debug)]
pub struct ChunkManager {
    last_try_free: AtomicU64,
}

impl Default for ChunkManager {
    fn default() -> Self {
        Self {
            last_try_free: AtomicU64::new(0),
        }
    }
}

impl ChunkManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn memory_manager(&self) -> &'static MemoryManager {
        memory_manager()
    }

    pub fn allocate(&self, size: u32, _flags: i32) -> bool {
        let mm = self.memory_manager();
        let memory_usage = mm.memory_usage();
        let max_memory_usage = mm.max_memory_usage();

        if max_memory_usage > 0 {
            if memory_usage + (size as u64) > (3 * max_memory_usage) / 4 {
                self.try_free_memory(max_memory_usage / 4);
            }

            let current_usage = mm.memory_usage();
            if current_usage + (size as u64) > max_memory_usage {
                return false;
            }
        }

        mm.account_memory_block(size as u64);
        true
    }

    pub fn deallocate(&self, size: u32, _flags: i32) {
        self.memory_manager().release_memory_block(size as u64);
    }

    pub fn try_free_memory(&self, size: u64) {
        // Reclaim unused block-pool buffers back to the OS through the engine
        // hook, then record the attempt for the periodic memory-pressure poll.
        let freed = self.memory_manager().try_free_memory(size);
        if freed > 0 {
            tracing::debug!(freed, requested = size, "reclaimed unused memory blocks");
        }
        self.last_try_free.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::Relaxed,
        );
    }
}
