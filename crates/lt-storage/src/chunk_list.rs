use bitflags::bitflags;
use std::sync::Arc;

use crate::chunk::Chunk;
use crate::chunk_manager::{ChunkManager, ALLOCATE_DONT_LOG, ALLOCATE_REVERT_LOG};
use crate::error::StorageError;
use crate::memory_manager::memory_manager;
use crate::mmap_safe::{Prot, SyncFlags as MmapSyncFlags};

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct GetFlags: i32 {
        const NONE = 0;
        const GET_WRITABLE = 1 << 0;
        const GET_BLOCKING = 1 << 1;
        const GET_NONBLOCK = 1 << 2;
        const GET_HASHING = 1 << 3;
        const GET_NOT_HASHING = 1 << 4;
        const GET_DONT_LOG = 1 << 5;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct ReleaseFlags: i32 {
        const DEFAULT = 0;
        const DONT_LOG = 1 << 0;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct ChunkSyncFlags: i32 {
        const ALL = 1 << 0;
        const FORCE = 1 << 1;
        const SAFE = 1 << 2;
        const SLOPPY = 1 << 3;
        const USE_TIMEOUT = 1 << 4;
        const IGNORE_ERROR = 1 << 5;
    }
}

pub const FLAG_ACTIVE: i32 = 1 << 0;

pub type SlotCreateChunk = Box<dyn Fn(u32, Prot) -> Result<Chunk, StorageError> + Send + Sync>;
pub type SlotStorageError = Box<dyn Fn(&str) + Send + Sync>;
pub type SlotFreeDiskspace = Box<dyn Fn() -> u64 + Send + Sync>;

#[derive(Debug, Default)]
pub struct ChunkListNode {
    index: u32,
    chunk: Option<Chunk>,
    references: usize,
    writable: usize,
    blocking: usize,
    async_triggered: bool,
    time_modified: u64,
    time_preloaded: u64,
}

impl ChunkListNode {
    pub fn new(index: u32) -> Self {
        Self {
            index,
            ..Default::default()
        }
    }

    pub fn is_valid(&self) -> bool {
        self.chunk.is_some()
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    pub fn set_index(&mut self, idx: u32) {
        self.index = idx;
    }

    pub fn chunk(&self) -> Option<&Chunk> {
        self.chunk.as_ref()
    }

    pub fn chunk_mut(&mut self) -> Option<&mut Chunk> {
        self.chunk.as_mut()
    }

    pub fn set_chunk(&mut self, c: Chunk) {
        self.chunk = Some(c);
    }

    pub fn clear_chunk(&mut self) {
        self.chunk = None;
    }

    pub fn references(&self) -> usize {
        self.references
    }

    pub fn writable(&self) -> usize {
        self.writable
    }

    pub fn blocking(&self) -> usize {
        self.blocking
    }

    pub fn sync_triggered(&self) -> bool {
        self.async_triggered
    }

    pub fn set_sync_triggered(&mut self, v: bool) {
        self.async_triggered = v;
    }

    pub fn time_modified(&self) -> u64 {
        self.time_modified
    }

    pub fn set_time_modified(&mut self, t: u64) {
        self.time_modified = t;
    }

    pub fn time_preloaded(&self) -> u64 {
        self.time_preloaded
    }

    pub fn set_time_preloaded(&mut self, t: u64) {
        self.time_preloaded = t;
    }
}

#[derive(Debug, Default, Clone)]
pub struct ChunkHandle {
    index: u32,
    valid: bool,
    writable: bool,
    blocking: bool,
    error_number: i32,
}

impl ChunkHandle {
    pub fn new(index: u32, writable: bool, blocking: bool) -> Self {
        Self {
            index,
            valid: true,
            writable,
            blocking,
            error_number: 0,
        }
    }

    pub fn from_error(err: i32) -> Self {
        Self {
            index: u32::MAX,
            valid: false,
            writable: false,
            blocking: false,
            error_number: if err == 0 { libc::ENOENT } else { err },
        }
    }

    pub fn is_valid(&self) -> bool {
        self.valid
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }

    pub fn is_blocking(&self) -> bool {
        self.blocking
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    pub fn error_number(&self) -> i32 {
        self.error_number
    }

    pub fn clear(&mut self) {
        self.valid = false;
        self.writable = false;
        self.blocking = false;
        self.error_number = 0;
    }
}

#[derive(Default)]
pub struct ChunkList {
    nodes: Vec<ChunkListNode>,
    queue: Vec<u32>,
    flags: i32,
    chunk_size: u32,
    manager: Option<Arc<ChunkManager>>,

    slot_create_chunk: Option<SlotCreateChunk>,
    slot_create_hashing_chunk: Option<SlotCreateChunk>,
    slot_storage_error: Option<SlotStorageError>,
    slot_free_diskspace: Option<SlotFreeDiskspace>,
}

impl ChunkList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn flags(&self) -> i32 {
        self.flags
    }

    pub fn set_flags(&mut self, f: i32) {
        self.flags |= f;
    }

    pub fn unset_flags(&mut self, f: i32) {
        self.flags &= !f;
    }

    pub fn chunk_size(&self) -> u32 {
        self.chunk_size
    }

    pub fn set_chunk_size(&mut self, cs: u32) {
        self.chunk_size = cs;
    }

    pub fn queue_size(&self) -> usize {
        self.queue.len()
    }

    pub fn size(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn set_manager(&mut self, m: Arc<ChunkManager>) {
        self.manager = Some(m);
    }

    pub fn set_slot_create_chunk<F>(&mut self, f: F)
    where
        F: Fn(u32, Prot) -> Result<Chunk, StorageError> + Send + Sync + 'static,
    {
        self.slot_create_chunk = Some(Box::new(f));
    }

    pub fn set_slot_create_hashing_chunk<F>(&mut self, f: F)
    where
        F: Fn(u32, Prot) -> Result<Chunk, StorageError> + Send + Sync + 'static,
    {
        self.slot_create_hashing_chunk = Some(Box::new(f));
    }

    pub fn set_slot_storage_error<F>(&mut self, f: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.slot_storage_error = Some(Box::new(f));
    }

    pub fn set_slot_free_diskspace<F>(&mut self, f: F)
    where
        F: Fn() -> u64 + Send + Sync + 'static,
    {
        self.slot_free_diskspace = Some(Box::new(f));
    }

    pub fn resize(&mut self, to_size: usize) {
        self.clear();
        self.nodes.reserve(to_size);
        for i in 0..to_size {
            self.nodes.push(ChunkListNode::new(i as u32));
        }
    }

    pub fn clear(&mut self) {
        for node in &mut self.nodes {
            if node.is_valid() {
                if let Some(ref mgr) = self.manager {
                    mgr.deallocate(self.chunk_size, 0);
                }
                node.clear_chunk();
            }
        }
        self.nodes.clear();
        self.queue.clear();
    }

    pub fn node(&self, idx: usize) -> &ChunkListNode {
        &self.nodes[idx]
    }

    pub fn node_mut(&mut self, idx: usize) -> &mut ChunkListNode {
        &mut self.nodes[idx]
    }

    pub fn get(&mut self, index: u32, flags: GetFlags) -> ChunkHandle {
        let idx = index as usize;
        if idx >= self.nodes.len() {
            return ChunkHandle::from_error(libc::EINVAL);
        }

        let is_writable_req = flags.contains(GetFlags::GET_WRITABLE);
        let is_blocking_req = flags.contains(GetFlags::GET_BLOCKING);
        let is_nonblock = flags.contains(GetFlags::GET_NONBLOCK);
        let dont_log = flags.contains(GetFlags::GET_DONT_LOG);

        let node = &self.nodes[idx];

        // Check blocking conflicts
        if is_writable_req && node.blocking > 0 {
            if is_nonblock {
                return ChunkHandle::from_error(libc::EAGAIN);
            }
        }
        if is_blocking_req && node.writable > 0 {
            if is_nonblock {
                return ChunkHandle::from_error(libc::EAGAIN);
            }
        }

        let prot = Prot::READ
            | if is_writable_req {
                Prot::WRITE
            } else {
                Prot::NONE
            };

        // If not loaded, create chunk
        if !node.is_valid() {
            let alloc_flags = if dont_log { ALLOCATE_DONT_LOG } else { 0 };

            if let Some(ref mgr) = self.manager {
                if !mgr.allocate(self.chunk_size, alloc_flags) {
                    return ChunkHandle::from_error(libc::ENOMEM);
                }
            }

            let res = if flags.contains(GetFlags::GET_HASHING) {
                if let Some(ref f) = self.slot_create_hashing_chunk {
                    f(index, prot)
                } else if let Some(ref f) = self.slot_create_chunk {
                    f(index, prot)
                } else {
                    Err(StorageError::InvalidChunk)
                }
            } else if flags.contains(GetFlags::GET_NOT_HASHING) {
                if let Some(ref f) = self.slot_create_chunk {
                    f(index, prot)
                } else {
                    Err(StorageError::InvalidChunk)
                }
            } else {
                if let Some(ref f) = self.slot_create_chunk {
                    f(index, prot)
                } else {
                    Err(StorageError::InvalidChunk)
                }
            };

            match res {
                Ok(chunk) => {
                    let n = &mut self.nodes[idx];
                    n.set_chunk(chunk);
                    n.set_time_modified(0);
                }
                Err(_) => {
                    if let Some(ref mgr) = self.manager {
                        mgr.deallocate(self.chunk_size, alloc_flags | ALLOCATE_REVERT_LOG);
                    }
                    return ChunkHandle::from_error(libc::EIO);
                }
            }
        } else if is_writable_req {
            // If already loaded as read-only, ensure writable
            let is_currently_writable = self.nodes[idx]
                .chunk()
                .map(|c| c.is_writable())
                .unwrap_or(false);

            if !is_currently_writable {
                if self.nodes[idx].blocking > 0 {
                    if is_nonblock {
                        return ChunkHandle::from_error(libc::EAGAIN);
                    }
                }
                if let Some(ref f) = self.slot_create_chunk {
                    match f(index, prot) {
                        Ok(new_chunk) => {
                            self.nodes[idx].set_chunk(new_chunk);
                            self.nodes[idx].set_time_modified(0);
                        }
                        Err(_) => {
                            return ChunkHandle::from_error(libc::EIO);
                        }
                    }
                }
            }
        }

        let n = &mut self.nodes[idx];
        n.references += 1;
        if is_writable_req {
            n.writable += 1;
            n.set_sync_triggered(false);
        }
        if is_blocking_req {
            n.blocking += 1;
        }

        ChunkHandle::new(index, is_writable_req, is_blocking_req)
    }

    pub fn release(&mut self, handle: &mut ChunkHandle, _flags: ReleaseFlags) {
        if !handle.is_valid() {
            return;
        }

        let idx = handle.index() as usize;
        if idx >= self.nodes.len() {
            return;
        }

        let is_wr = handle.is_writable();
        let is_blk = handle.is_blocking();

        let n = &mut self.nodes[idx];
        if is_blk && n.blocking > 0 {
            n.blocking -= 1;
        }

        if is_wr && n.writable > 0 {
            if n.writable == 1 {
                if !self.queue.contains(&(idx as u32)) {
                    self.queue.push(idx as u32);
                    memory_manager().account_sync_queue(self.chunk_size as u64);
                }
                n.writable -= 1;
            } else {
                n.writable -= 1;
            }
        }

        if n.references > 0 {
            n.references -= 1;
        }

        handle.clear();
    }

    pub fn find_address(&self, ptr: *const u8) -> Option<(usize, usize)> {
        for (idx, node) in self.nodes.iter().enumerate() {
            if let Some(chunk) = node.chunk() {
                if let Some(part_idx) = chunk.find_address(ptr) {
                    return Some((idx, part_idx));
                }
            }
        }
        None
    }

    pub fn sync_chunks(&mut self, flags: ChunkSyncFlags) -> u32 {
        if self.queue.is_empty() {
            return 0;
        }

        let mut failed = 0;
        let mut synced_indices = Vec::new();

        let mmap_flags = if flags.contains(ChunkSyncFlags::FORCE) || flags.contains(ChunkSyncFlags::SAFE) {
            MmapSyncFlags::SYNC
        } else {
            MmapSyncFlags::ASYNC
        };

        for &idx in &self.queue {
            if let Some(ref mut chunk) = self.nodes[idx as usize].chunk_mut() {
                match chunk.sync(mmap_flags) {
                    Ok(true) => {
                        self.nodes[idx as usize].set_sync_triggered(true);
                        synced_indices.push(idx);
                    }
                    _ => {
                        failed += 1;
                    }
                }
            } else {
                synced_indices.push(idx);
            }
        }

        // Release from sync queue
        memory_manager().release_sync_queue(synced_indices.len() as u32, self.chunk_size);
        self.queue.retain(|idx| !synced_indices.contains(idx));

        if failed > 0 && !flags.contains(ChunkSyncFlags::IGNORE_ERROR) {
            if let Some(ref f) = self.slot_storage_error {
                f("Could not sync chunk");
            }
        }

        failed
    }
}
