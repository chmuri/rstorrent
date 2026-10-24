//! Sharded pool of disk workers.
//!
//! Every read, write, sync and hash pass for a torrent is routed to exactly
//! one worker, chosen from the first four bytes of its info hash. That keeps
//! per-torrent FIFO ordering (the property the single worker used to provide)
//! while letting unrelated torrents use separate queues.
//!
//! Shard assignment depends only on the hash and the worker count, so it is
//! stable for the lifetime of a process. Changing `storage.disk_threads` moves
//! torrents to different workers, which is safe because ordering is only
//! required per torrent, and remapping happens only at startup.

use std::sync::Arc;
use std::time::Duration;

use lt_common::HashString;

use crate::disk_thread::{
    default_disk_threads, default_max_open_files, DiskThread, DiskThreadStats, DEFAULT_DISK_QUEUE,
    DEFAULT_IDLE_TIMEOUT,
};
use crate::error::StorageError;
use crate::file_manager::FileManagerStats;

/// Configuration resolved once at startup.
#[derive(Debug, Clone, Copy)]
pub struct DiskPoolConfig {
    pub threads: usize,
    pub queue_capacity: usize,
    pub max_open_files: usize,
    pub idle_timeout: Duration,
}

impl Default for DiskPoolConfig {
    fn default() -> Self {
        Self {
            threads: default_disk_threads(),
            queue_capacity: DEFAULT_DISK_QUEUE,
            max_open_files: default_max_open_files(),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }
}

/// Fixed set of disk workers, addressed by torrent hash.
pub struct DiskPool {
    config: DiskPoolConfig,
    workers: Vec<Arc<DiskThread>>,
}

impl DiskPool {
    pub fn new(config: DiskPoolConfig) -> Result<Arc<Self>, StorageError> {
        let threads = config.threads.max(1);
        let mut workers = Vec::with_capacity(threads);
        for _ in 0..threads {
            workers.push(Arc::new(DiskThread::spawn_with(
                config.queue_capacity,
                config.max_open_files,
                config.idle_timeout,
            )?));
        }
        Ok(Arc::new(Self { config, workers }))
    }

    pub fn config(&self) -> DiskPoolConfig {
        self.config
    }

    pub fn workers(&self) -> &[Arc<DiskThread>] {
        &self.workers
    }

    /// Worker that owns every operation for `hash`.
    pub fn thread_for(&self, hash: &HashString) -> Arc<DiskThread> {
        let bytes = hash.as_bytes();
        let value = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        Arc::clone(&self.workers[value % self.workers.len()])
    }

    /// Sum of every worker's counters. `queue_capacity` and
    /// `max_queue_depth` are totals across the pool.
    pub fn stats(&self) -> DiskThreadStats {
        let mut total = DiskThreadStats {
            queue_capacity: 0,
            queued_commands: 0,
            in_flight: 0,
            max_queue_depth: 0,
            writes_submitted: 0,
            writes_completed: 0,
            writes_failed: 0,
            bytes_written: 0,
            reads_submitted: 0,
            reads_completed: 0,
            hashes_submitted: 0,
            hashes_completed: 0,
            syncs: 0,
            sync_queue_depth: 0,
            sync_queue_bytes: 0,
        };
        for worker in &self.workers {
            let stats = worker.stats();
            total.queue_capacity += stats.queue_capacity;
            total.queued_commands += stats.queued_commands;
            total.in_flight += stats.in_flight;
            total.max_queue_depth += stats.max_queue_depth;
            total.writes_submitted += stats.writes_submitted;
            total.writes_completed += stats.writes_completed;
            total.writes_failed += stats.writes_failed;
            total.bytes_written += stats.bytes_written;
            total.reads_submitted += stats.reads_submitted;
            total.reads_completed += stats.reads_completed;
            total.hashes_submitted += stats.hashes_submitted;
            total.hashes_completed += stats.hashes_completed;
            total.syncs += stats.syncs;
            total.sync_queue_depth += stats.sync_queue_depth;
            total.sync_queue_bytes += stats.sync_queue_bytes;
        }
        total
    }

    /// Sum of every worker's file-manager counters.
    pub fn file_manager_stats(&self) -> FileManagerStats {
        let mut total = FileManagerStats::default();
        for worker in &self.workers {
            let stats = worker.file_stats();
            total.open_files += stats.open_files;
            total.in_flight += stats.in_flight;
            total.files_opened += stats.files_opened;
            total.files_closed += stats.files_closed;
            total.files_failed += stats.files_failed;
            total.fadvise += stats.fadvise;
            total.fadvise_failures += stats.fadvise_failures;
        }
        total
    }

    /// Flushes and stops every worker. Safe to call more than once.
    pub fn shutdown(&self) {
        for worker in &self.workers {
            worker.shutdown();
        }
    }
}
