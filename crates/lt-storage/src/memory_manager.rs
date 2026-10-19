use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;

#[derive(Debug)]
pub struct MemoryManager {
    memory_usage: AtomicU64,
    memory_block_count: AtomicU32,
    max_memory_usage: AtomicU64,

    sync_queue_block_count: AtomicU32,
    sync_queue_memory_usage: AtomicU64,

    safe_sync: AtomicBool,
    timeout_sync: AtomicU32,

    preload_type: AtomicU32,
    preload_min_size: AtomicU32,
    preload_required_rate: AtomicU32,

    stats_preloaded: AtomicU32,
    stats_not_preloaded: AtomicU32,
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self {
            memory_usage: AtomicU64::new(0),
            memory_block_count: AtomicU32::new(0),
            max_memory_usage: AtomicU64::new(0), // 0 means unlimited by default

            sync_queue_block_count: AtomicU32::new(0),
            sync_queue_memory_usage: AtomicU64::new(0),

            safe_sync: AtomicBool::new(false),
            timeout_sync: AtomicU32::new(600),

            preload_type: AtomicU32::new(0),
            preload_min_size: AtomicU32::new(256 << 10),
            preload_required_rate: AtomicU32::new(5 << 10),

            stats_preloaded: AtomicU32::new(0),
            stats_not_preloaded: AtomicU32::new(0),
        }
    }
}

static GLOBAL_MEMORY_MANAGER: OnceLock<MemoryManager> = OnceLock::new();

pub fn memory_manager() -> &'static MemoryManager {
    GLOBAL_MEMORY_MANAGER.get_or_init(MemoryManager::default)
}

impl MemoryManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn memory_usage(&self) -> u64 {
        self.memory_usage.load(Ordering::Acquire)
    }

    pub fn memory_block_count(&self) -> u32 {
        self.memory_block_count.load(Ordering::Acquire)
    }

    pub fn max_memory_usage(&self) -> u64 {
        self.max_memory_usage.load(Ordering::Acquire)
    }

    pub fn set_max_memory_usage(&self, bytes: u64) {
        self.max_memory_usage.store(bytes, Ordering::Release);
    }

    pub fn account_memory_block(&self, bytes: u64) {
        self.memory_usage.fetch_add(bytes, Ordering::AcqRel);
        self.memory_block_count.fetch_add(1, Ordering::AcqRel);
    }

    pub fn release_memory_block(&self, bytes: u64) {
        self.memory_usage.fetch_sub(bytes, Ordering::AcqRel);
        self.memory_block_count.fetch_sub(1, Ordering::AcqRel);
    }

    pub fn sync_queue_block_count(&self) -> u32 {
        self.sync_queue_block_count.load(Ordering::Acquire)
    }

    pub fn sync_queue_memory_usage(&self) -> u64 {
        self.sync_queue_memory_usage.load(Ordering::Acquire)
    }

    pub fn account_sync_queue(&self, bytes: u64) {
        self.sync_queue_memory_usage.fetch_add(bytes, Ordering::AcqRel);
        self.sync_queue_block_count.fetch_add(1, Ordering::AcqRel);
    }

    pub fn release_sync_queue(&self, count: u32, chunk_size: u32) {
        let total = (count as u64) * (chunk_size as u64);
        self.sync_queue_memory_usage.fetch_sub(total, Ordering::AcqRel);
        self.sync_queue_block_count.fetch_sub(count, Ordering::AcqRel);
    }

    pub fn sync_safe_free_diskspace(&self) -> u64 {
        500 << 20 // 500 MB
    }

    pub fn safe_sync(&self) -> bool {
        self.safe_sync.load(Ordering::Acquire)
    }

    pub fn set_safe_sync(&self, state: bool) {
        self.safe_sync.store(state, Ordering::Release);
    }

    pub fn timeout_sync(&self) -> u32 {
        self.timeout_sync.load(Ordering::Acquire)
    }

    pub fn set_timeout_sync(&self, seconds: u32) {
        self.timeout_sync.store(seconds, Ordering::Release);
    }

    pub fn preload_type(&self) -> u32 {
        self.preload_type.load(Ordering::Acquire)
    }

    pub fn set_preload_type(&self, t: u32) {
        self.preload_type.store(t, Ordering::Release);
    }

    pub fn preload_min_size(&self) -> u32 {
        self.preload_min_size.load(Ordering::Acquire)
    }

    pub fn set_preload_min_size(&self, bytes: u32) {
        self.preload_min_size.store(bytes, Ordering::Release);
    }

    pub fn preload_required_rate(&self) -> u32 {
        self.preload_required_rate.load(Ordering::Acquire)
    }

    pub fn set_preload_required_rate(&self, bytes: u32) {
        self.preload_required_rate.store(bytes, Ordering::Release);
    }

    pub fn stats_preloaded(&self) -> u32 {
        self.stats_preloaded.load(Ordering::Acquire)
    }

    pub fn stats_not_preloaded(&self) -> u32 {
        self.stats_not_preloaded.load(Ordering::Acquire)
    }

    pub fn increment_stats_preloaded(&self) {
        self.stats_preloaded.fetch_add(1, Ordering::AcqRel);
    }

    pub fn increment_stats_not_preloaded(&self) {
        self.stats_not_preloaded.fetch_add(1, Ordering::AcqRel);
    }
}
