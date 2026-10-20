//! Dedicated disk worker.
//!
//! All blocking file I/O for piece reads, writes, hashing and syncs is
//! funnelled through a single OS thread. Async callers submit commands over a
//! bounded channel and poll a typed reply handle instead of touching the
//! filesystem themselves. This mirrors the legacy `torrent::ThreadDisk`
//! boundary without requiring `lt-storage` to depend on Tokio.
//!
//! Every command carries a monotonically increasing correlation id. Replies
//! repeat the id so a caller that multiplexes work can never mistake one
//! completion for another.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::io::RawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;

use crate::chunk_list::ChunkHandle;
use crate::error::StorageError;
use crate::file::File;
use crate::file_manager::{FileManager, FileManagerStats, FileManagerStatsHandle};
use crate::hash_queue::HashQueue;
use crate::memory_manager::memory_manager;
use crate::mmap_safe::Prot;
use crate::socket_file::OpenFlags;
use crate::writer::TargetFileInfo;

/// Default bounded queue length for the process-wide disk thread.
pub const DEFAULT_DISK_QUEUE: usize = 256;

/// Default maximum number of simultaneously open files in the worker cache.
pub const DEFAULT_MAX_OPEN_FILES: usize = 64;

/// Idle timeout after which the worker closes cached file handles.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Hashing reads a whole piece at a time; keep the buffer size bounded.
const HASH_READ_CHUNK: usize = 1 << 20;

/// A complete disk request. Every variant carries a correlation id and a
/// bounded reply channel.
pub enum DiskCommand {
    Read {
        id: u64,
        root: PathBuf,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        block_offset: u32,
        length: u32,
        reply: SyncSender<DiskReply>,
    },
    Write {
        id: u64,
        root: PathBuf,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        block_offset: u32,
        data: Vec<u8>,
        reply: SyncSender<DiskReply>,
    },
    Sync {
        id: u64,
        reply: SyncSender<DiskReply>,
    },
    Hash {
        id: u64,
        generation: u64,
        root: PathBuf,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        piece_size: u32,
        reply: SyncSender<DiskReply>,
    },
    Open {
        id: u64,
        root: PathBuf,
        files: Arc<Vec<TargetFileInfo>>,
        reply: SyncSender<DiskReply>,
    },
    Close {
        id: u64,
        reply: SyncSender<DiskReply>,
    },
    Shutdown {
        id: u64,
        reply: SyncSender<DiskReply>,
    },
}

impl DiskCommand {
    /// Correlation id of this request.
    pub fn id(&self) -> u64 {
        match self {
            DiskCommand::Read { id, .. }
            | DiskCommand::Write { id, .. }
            | DiskCommand::Sync { id, .. }
            | DiskCommand::Hash { id, .. }
            | DiskCommand::Open { id, .. }
            | DiskCommand::Close { id, .. }
            | DiskCommand::Shutdown { id, .. } => *id,
        }
    }
}

/// A hash completion. `generation` is supplied by the caller and must be
/// compared against its current chunk generation before the result is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskHashResult {
    pub generation: u64,
    pub piece_index: u32,
    pub hash: [u8; 20],
}

/// A disk completion. Replies are matched to requests by correlation id.
#[derive(Debug)]
pub enum DiskReply {
    Read {
        id: u64,
        data: Result<Vec<u8>, StorageError>,
    },
    Write {
        id: u64,
        result: Result<(), StorageError>,
    },
    Sync {
        id: u64,
        result: Result<(), StorageError>,
    },
    Hash {
        id: u64,
        generation: u64,
        piece_index: u32,
        hash: Result<[u8; 20], StorageError>,
    },
    Open {
        id: u64,
        result: Result<(), StorageError>,
    },
    Close {
        id: u64,
        result: Result<(), StorageError>,
    },
    Shutdown {
        id: u64,
        result: Result<(), StorageError>,
    },
}

impl DiskReply {
    pub fn id(&self) -> u64 {
        match self {
            DiskReply::Read { id, .. }
            | DiskReply::Write { id, .. }
            | DiskReply::Sync { id, .. }
            | DiskReply::Hash { id, .. }
            | DiskReply::Open { id, .. }
            | DiskReply::Close { id, .. }
            | DiskReply::Shutdown { id, .. } => *id,
        }
    }
}

/// Snapshot of the worker counters exposed to statistics commands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskThreadStats {
    pub queue_capacity: usize,
    pub queued_commands: usize,
    pub in_flight: usize,
    pub max_queue_depth: usize,
    pub writes_submitted: u64,
    pub writes_completed: u64,
    pub writes_failed: u64,
    pub bytes_written: u64,
    pub reads_submitted: u64,
    pub reads_completed: u64,
    pub hashes_submitted: u64,
    pub hashes_completed: u64,
    pub syncs: u64,
    /// Number of verified pieces that are queued for, or executing in, the
    /// worker. Mirrors `pieces.sync.queue_size`.
    pub sync_queue_depth: usize,
    /// Bytes covered by the pending write set. Mirrors
    /// `pieces.memory.sync_queue`.
    pub sync_queue_bytes: u64,
}

#[derive(Default)]
struct StatsInner {
    queued: AtomicUsize,
    in_flight: AtomicUsize,
    max_queue_depth: AtomicUsize,
    writes_submitted: AtomicU64,
    writes_completed: AtomicU64,
    writes_failed: AtomicU64,
    bytes_written: AtomicU64,
    reads_submitted: AtomicU64,
    reads_completed: AtomicU64,
    hashes_submitted: AtomicU64,
    hashes_completed: AtomicU64,
    syncs: AtomicU64,
    sync_queue_depth: AtomicUsize,
    sync_queue_bytes: AtomicU64,
}

impl StatsInner {
    fn note_queued(&self, capacity: usize) {
        let depth = self.queued.fetch_add(1, Ordering::AcqRel) + 1;
        // A failed `try_send` briefly increments the counter before being
        // rolled back; never report more than the channel can hold.
        self.max_queue_depth
            .fetch_max(depth.min(capacity.max(1)), Ordering::AcqRel);
    }

    fn note_dequeued(&self) {
        let mut current = self.queued.load(Ordering::Acquire);
        while current > 0 {
            match self.queued.compare_exchange_weak(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    fn account_write(&self, bytes: u64) {
        self.writes_submitted.fetch_add(1, Ordering::AcqRel);
        self.sync_queue_depth.fetch_add(1, Ordering::AcqRel);
        self.sync_queue_bytes.fetch_add(bytes, Ordering::AcqRel);

        // The process-wide memory manager feeds `pieces.sync.queue_size` and
        // `pieces.memory.sync_queue`. Skip the global side effect while this
        // crate is being unit-tested so storage-level chunk-list tests that
        // assert exact global counters are not perturbed by unrelated worker
        // threads. Integration tests link the library normally and therefore
        // observe the same accounting as production.
        if !cfg!(test) {
            memory_manager().account_sync_queue(bytes);
        }
    }

    /// Undoes [`StatsInner::account_write`] when the command could not be
    /// enqueued (full or disconnected channel).
    fn rollback_write(&self, bytes: u64) {
        self.writes_submitted.fetch_sub(1, Ordering::AcqRel);
        self.sync_queue_depth.fetch_sub(1, Ordering::AcqRel);
        self.sync_queue_bytes.fetch_sub(bytes, Ordering::AcqRel);
        if !cfg!(test) {
            memory_manager().release_sync_queue(1, bytes as u32);
        }
    }

    fn release_write(&self, bytes: u64, succeeded: bool) {
        if succeeded {
            self.writes_completed.fetch_add(1, Ordering::AcqRel);
            self.bytes_written.fetch_add(bytes, Ordering::AcqRel);
        } else {
            self.writes_failed.fetch_add(1, Ordering::AcqRel);
        }
        let mut current = self.sync_queue_depth.load(Ordering::Acquire);
        while current > 0 {
            match self.sync_queue_depth.compare_exchange_weak(
                current,
                current - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        let mut current = self.sync_queue_bytes.load(Ordering::Acquire);
        while current > 0 {
            let next = current.saturating_sub(bytes);
            match self.sync_queue_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        if !cfg!(test) {
            memory_manager().release_sync_queue(1, bytes as u32);
        }
    }
}

/// Handle for an asynchronous piece write.
pub struct DiskWriteHandle {
    id: u64,
    rx: Receiver<DiskReply>,
}

impl DiskWriteHandle {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Non-blocking poll. Returns `None` while the write is still queued or
    /// running.
    pub fn try_finish(&self) -> Option<Result<(), StorageError>> {
        match self.rx.try_recv() {
            Ok(DiskReply::Write { id, result }) if id == self.id => Some(result),
            Ok(reply) => Some(Err(mismatched_reply(self.id, reply.id()))),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(StorageError::DiskThreadClosed)),
        }
    }

    /// Blocks until the write finishes. Intended for tests and shutdown
    /// paths, never for the async reactor.
    pub fn wait(self) -> Result<(), StorageError> {
        match self.rx.recv() {
            Ok(DiskReply::Write { id, result }) if id == self.id => result,
            Ok(reply) => Err(mismatched_reply(self.id, reply.id())),
            Err(_) => Err(StorageError::DiskThreadClosed),
        }
    }
}

/// Handle for an asynchronous block read.
pub struct DiskReadHandle {
    id: u64,
    rx: Receiver<DiskReply>,
}

impl DiskReadHandle {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn try_finish(&self) -> Option<Result<Vec<u8>, StorageError>> {
        match self.rx.try_recv() {
            Ok(DiskReply::Read { id, data }) if id == self.id => Some(data),
            Ok(reply) => Some(Err(mismatched_reply(self.id, reply.id()))),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(StorageError::DiskThreadClosed)),
        }
    }

    pub fn wait(self) -> Result<Vec<u8>, StorageError> {
        match self.rx.recv() {
            Ok(DiskReply::Read { id, data }) if id == self.id => data,
            Ok(reply) => Err(mismatched_reply(self.id, reply.id())),
            Err(_) => Err(StorageError::DiskThreadClosed),
        }
    }
}

/// Handle for an asynchronous hash command.
pub struct DiskHashHandle {
    id: u64,
    rx: Receiver<DiskReply>,
}

impl DiskHashHandle {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn try_finish(&self) -> Option<Result<DiskHashResult, StorageError>> {
        match self.rx.try_recv() {
            Ok(DiskReply::Hash {
                id,
                generation,
                piece_index,
                hash,
            }) if id == self.id => Some(hash.map(|hash| DiskHashResult {
                generation,
                piece_index,
                hash,
            })),
            Ok(reply) => Some(Err(mismatched_reply(self.id, reply.id()))),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(StorageError::DiskThreadClosed)),
        }
    }

    pub fn wait(self) -> Result<DiskHashResult, StorageError> {
        match self.rx.recv() {
            Ok(DiskReply::Hash {
                id,
                generation,
                piece_index,
                hash,
            }) if id == self.id => hash.map(|hash| DiskHashResult {
                generation,
                piece_index,
                hash,
            }),
            Ok(reply) => Err(mismatched_reply(self.id, reply.id())),
            Err(_) => Err(StorageError::DiskThreadClosed),
        }
    }
}

/// Handle for `Sync`, `Open` and `Close`, which carry no payload.
pub struct DiskOpHandle {
    id: u64,
    rx: Receiver<DiskReply>,
}

impl DiskOpHandle {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn try_finish(&self) -> Option<Result<(), StorageError>> {
        match self.rx.try_recv() {
            Ok(DiskReply::Sync { id, result })
            | Ok(DiskReply::Open { id, result })
            | Ok(DiskReply::Close { id, result })
                if id == self.id =>
            {
                Some(result)
            }
            Ok(reply) => Some(Err(mismatched_reply(self.id, reply.id()))),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(StorageError::DiskThreadClosed)),
        }
    }

    pub fn wait(self) -> Result<(), StorageError> {
        match self.rx.recv() {
            Ok(DiskReply::Sync { id, result })
            | Ok(DiskReply::Open { id, result })
            | Ok(DiskReply::Close { id, result })
                if id == self.id =>
            {
                result
            }
            Ok(reply) => Err(mismatched_reply(self.id, reply.id())),
            Err(_) => Err(StorageError::DiskThreadClosed),
        }
    }
}

fn mismatched_reply(expected: u64, actual: u64) -> StorageError {
    StorageError::Io(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("disk reply id {actual} does not match request id {expected}"),
    ))
}

/// Handle to the dedicated disk worker thread.
pub struct DiskThread {
    tx: SyncSender<DiskCommand>,
    join: Mutex<Option<JoinHandle<()>>>,
    queue_capacity: usize,
    next_id: AtomicU64,
    stats: Arc<StatsInner>,
    file_stats: FileManagerStatsHandle,
}

impl DiskThread {
    /// Spawns the disk worker with a bounded request queue.
    pub fn spawn(queue_capacity: usize) -> Result<Self, StorageError> {
        let capacity = queue_capacity.max(1);
        let (tx, rx) = mpsc::sync_channel(capacity);
        let stats = Arc::new(StatsInner::default());

        let mut manager = FileManager::new();
        manager.set_max_open_files(DEFAULT_MAX_OPEN_FILES);
        manager.set_close_idle_secs(DEFAULT_IDLE_TIMEOUT.as_secs());
        let file_stats = manager.stats_handle();

        let worker_stats = Arc::clone(&stats);
        let join = thread::Builder::new()
            .name("lt-storage-disk".to_string())
            .spawn(move || worker_loop(rx, worker_stats, manager))
            .map_err(StorageError::Io)?;

        Ok(Self {
            tx,
            join: Mutex::new(Some(join)),
            queue_capacity: capacity,
            next_id: AtomicU64::new(1),
            stats,
            file_stats,
        })
    }

    pub fn queue_capacity(&self) -> usize {
        self.queue_capacity
    }

    /// Approximate number of commands waiting in the bounded channel.
    pub fn queue_depth(&self) -> usize {
        self.stats.queued.load(Ordering::Acquire)
    }

    /// Number of commands currently executing on the worker thread.
    pub fn in_flight(&self) -> usize {
        self.stats.in_flight.load(Ordering::Acquire)
    }

    /// Number of verified pieces not yet written.
    pub fn sync_queue_depth(&self) -> usize {
        self.stats.sync_queue_depth.load(Ordering::Acquire)
    }

    /// Bytes covered by pending piece writes.
    pub fn sync_queue_bytes(&self) -> u64 {
        self.stats.sync_queue_bytes.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> DiskThreadStats {
        DiskThreadStats {
            queue_capacity: self.queue_capacity,
            queued_commands: self.stats.queued.load(Ordering::Acquire),
            in_flight: self.stats.in_flight.load(Ordering::Acquire),
            max_queue_depth: self.stats.max_queue_depth.load(Ordering::Acquire),
            writes_submitted: self.stats.writes_submitted.load(Ordering::Acquire),
            writes_completed: self.stats.writes_completed.load(Ordering::Acquire),
            writes_failed: self.stats.writes_failed.load(Ordering::Acquire),
            bytes_written: self.stats.bytes_written.load(Ordering::Acquire),
            reads_submitted: self.stats.reads_submitted.load(Ordering::Acquire),
            reads_completed: self.stats.reads_completed.load(Ordering::Acquire),
            hashes_submitted: self.stats.hashes_submitted.load(Ordering::Acquire),
            hashes_completed: self.stats.hashes_completed.load(Ordering::Acquire),
            syncs: self.stats.syncs.load(Ordering::Acquire),
            sync_queue_depth: self.stats.sync_queue_depth.load(Ordering::Acquire),
            sync_queue_bytes: self.stats.sync_queue_bytes.load(Ordering::Acquire),
        }
    }

    /// Live counters from the worker's private [`FileManager`].
    ///
    /// These are the values `system.files.opened_counter`,
    /// `system.files.closed_counter` and `system.files.failed_counter` should
    /// report. The counters are atomics shared with the worker, so they are
    /// updated before the corresponding command reply is sent.
    pub fn file_stats(&self) -> FileManagerStats {
        self.file_stats.stats()
    }

    /// Alias for [`DiskThread::file_stats`] for callers that prefer the
    /// explicit module-qualified name.
    pub fn file_manager_stats(&self) -> FileManagerStats {
        self.file_stats()
    }

    pub fn files_opened_counter(&self) -> u64 {
        self.file_stats.files_opened_counter()
    }

    pub fn files_closed_counter(&self) -> u64 {
        self.file_stats.files_closed_counter()
    }

    pub fn files_failed_counter(&self) -> u64 {
        self.file_stats.files_failed_counter()
    }

    /// Successful `posix_fadvise(POSIX_FADV_RANDOM)` calls.
    pub fn files_advised_counter(&self) -> u64 {
        self.file_stats.fadvise_counter()
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn send(&self, command: DiskCommand) -> Result<(), StorageError> {
        // Count the command before publishing it to the worker so a fast
        // dequeue cannot be observed before the enqueue.
        self.stats.note_queued(self.queue_capacity);
        match self.tx.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.stats.note_dequeued();
                Err(StorageError::DiskQueueFull)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.stats.note_dequeued();
                Err(StorageError::DiskThreadClosed)
            }
        }
    }

    fn send_blocking(&self, command: DiskCommand) -> Result<(), StorageError> {
        self.stats.note_queued(self.queue_capacity);
        match self.tx.send(command) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.stats.note_dequeued();
                Err(StorageError::DiskThreadClosed)
            }
        }
    }

    /// Submits a piece write without blocking the caller.
    ///
    /// Returns [`StorageError::DiskQueueFull`] when the bounded queue is full;
    /// the caller must apply backpressure and retry rather than dropping the
    /// piece or falling back to I/O on the caller's thread.
    pub fn submit_write(
        &self,
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        data: Vec<u8>,
    ) -> Result<DiskWriteHandle, StorageError> {
        self.submit_write_block(root, files, piece_length, piece_index, 0, data)
    }

    /// Submits a partial block write. `block_offset` is relative to the start
    /// of the piece and may be non-zero for a partial/final block.
    pub fn submit_write_block(
        &self,
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        block_offset: u32,
        data: Vec<u8>,
    ) -> Result<DiskWriteHandle, StorageError> {
        self.try_submit_write_block(root, files, piece_length, piece_index, block_offset, data)
            .map_err(|(err, _data)| err)
    }

    /// Like [`DiskThread::submit_write_block`] but returns the verified piece
    /// buffer when the bounded queue is full, so a caller applying
    /// backpressure never has to clone the data on every retry.
    pub fn try_submit_write_block(
        &self,
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        block_offset: u32,
        data: Vec<u8>,
    ) -> Result<DiskWriteHandle, (StorageError, Vec<u8>)> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let bytes = data.len() as u64;

        let command = DiskCommand::Write {
            id,
            root: root.into(),
            files,
            piece_length,
            piece_index,
            block_offset,
            data,
            reply: reply_tx,
        };

        // Account before handing the command to the worker: otherwise a very
        // fast worker could complete (and release) the write before the
        // submitting thread has incremented the counters.
        self.stats.note_queued(self.queue_capacity);
        self.stats.account_write(bytes);

        match self.tx.try_send(command) {
            Ok(()) => Ok(DiskWriteHandle { id, rx: reply_rx }),
            Err(TrySendError::Full(DiskCommand::Write { data, .. })) => {
                self.stats.note_dequeued();
                self.stats.rollback_write(bytes);
                Err((StorageError::DiskQueueFull, data))
            }
            Err(TrySendError::Disconnected(DiskCommand::Write { data, .. })) => {
                self.stats.note_dequeued();
                self.stats.rollback_write(bytes);
                Err((StorageError::DiskThreadClosed, data))
            }
            Err(_) => {
                self.stats.note_dequeued();
                self.stats.rollback_write(bytes);
                Err((StorageError::DiskThreadClosed, Vec::new()))
            }
        }
    }

    /// Writes a full piece and waits for the worker reply.
    ///
    /// The caller may block, but the filesystem syscall still executes on the
    /// disk worker. Intended for compatibility paths that explicitly disable
    /// the asynchronous queue (for example `Download::set_disk_thread(None)`).
    pub fn write_blocking(
        &self,
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        data: Vec<u8>,
    ) -> Result<(), StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        let bytes = data.len() as u64;

        self.stats.account_write(bytes);
        if let Err(err) = self.send_blocking(DiskCommand::Write {
            id,
            root: root.into(),
            files,
            piece_length,
            piece_index,
            block_offset: 0,
            data,
            reply: reply_tx,
        }) {
            self.stats.rollback_write(bytes);
            return Err(err);
        }

        match reply_rx.recv() {
            Ok(DiskReply::Write {
                id: reply_id,
                result,
            }) if reply_id == id => result,
            Ok(reply) => Err(mismatched_reply(id, reply.id())),
            Err(_) => Err(StorageError::DiskThreadClosed),
        }
    }

    /// Submits an asynchronous block read.
    pub fn submit_read(
        &self,
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        block_offset: u32,
        length: u32,
    ) -> Result<DiskReadHandle, StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);

        let command = DiskCommand::Read {
            id,
            root: root.into(),
            files,
            piece_length,
            piece_index,
            block_offset,
            length,
            reply: reply_tx,
        };

        self.stats.reads_submitted.fetch_add(1, Ordering::AcqRel);
        if let Err(err) = self.send(command) {
            self.stats.reads_submitted.fetch_sub(1, Ordering::AcqRel);
            return Err(err);
        }
        Ok(DiskReadHandle { id, rx: reply_rx })
    }

    /// Blocking convenience wrapper around [`DiskThread::submit_read`].
    ///
    /// Used by callers that already hold a non-reactor thread (or a test).
    /// The actual file I/O still happens on the disk worker.
    pub fn read_block(
        &self,
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        block_offset: u32,
        length: u32,
    ) -> Result<Vec<u8>, StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);

        let command = DiskCommand::Read {
            id,
            root: root.into(),
            files,
            piece_length,
            piece_index,
            block_offset,
            length,
            reply: reply_tx,
        };

        self.stats.reads_submitted.fetch_add(1, Ordering::AcqRel);
        if let Err(err) = self.send_blocking(command) {
            self.stats.reads_submitted.fetch_sub(1, Ordering::AcqRel);
            return Err(err);
        }

        match reply_rx.recv() {
            Ok(DiskReply::Read { id: reply_id, data }) if reply_id == id => data,
            Ok(reply) => Err(mismatched_reply(id, reply.id())),
            Err(_) => Err(StorageError::DiskThreadClosed),
        }
    }

    /// Submits a hash of a whole piece. `generation` is echoed in the reply so
    /// stale results can be discarded after a chunk is released/reallocated.
    pub fn submit_hash(
        &self,
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        piece_size: u32,
        generation: u64,
    ) -> Result<DiskHashHandle, StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);

        let command = DiskCommand::Hash {
            id,
            generation,
            root: root.into(),
            files,
            piece_length,
            piece_index,
            piece_size,
            reply: reply_tx,
        };

        self.stats.hashes_submitted.fetch_add(1, Ordering::AcqRel);
        if let Err(err) = self.send(command) {
            self.stats.hashes_submitted.fetch_sub(1, Ordering::AcqRel);
            return Err(err);
        }
        Ok(DiskHashHandle { id, rx: reply_rx })
    }

    /// Hashes a whole piece on the worker and waits for the reply.
    ///
    /// Used by synchronous recheck paths; the hash itself still executes on
    /// the disk thread, never on the caller.
    pub fn hash_blocking(
        &self,
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
        piece_length: u32,
        piece_index: u32,
        piece_size: u32,
        generation: u64,
    ) -> Result<DiskHashResult, StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);

        self.stats.hashes_submitted.fetch_add(1, Ordering::AcqRel);
        if let Err(err) = self.send_blocking(DiskCommand::Hash {
            id,
            generation,
            root: root.into(),
            files,
            piece_length,
            piece_index,
            piece_size,
            reply: reply_tx,
        }) {
            self.stats.hashes_submitted.fetch_sub(1, Ordering::AcqRel);
            return Err(err);
        }

        match reply_rx.recv() {
            Ok(DiskReply::Hash {
                id: reply_id,
                generation,
                piece_index,
                hash,
            }) if reply_id == id => hash.map(|hash| DiskHashResult {
                generation,
                piece_index,
                hash,
            }),
            Ok(reply) => Err(mismatched_reply(id, reply.id())),
            Err(_) => Err(StorageError::DiskThreadClosed),
        }
    }

    /// Asynchronously flushes cached file handles with `fsync`.
    pub fn submit_sync(&self) -> Result<DiskOpHandle, StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.send(DiskCommand::Sync {
            id,
            reply: reply_tx,
        })?;
        Ok(DiskOpHandle { id, rx: reply_rx })
    }

    /// Flushes all cached file handles with `fsync`.
    ///
    /// This blocks; use it on shutdown or explicit sync requests, not per piece.
    pub fn sync_all(&self) -> Result<(), StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.send_blocking(DiskCommand::Sync {
            id,
            reply: reply_tx,
        })?;

        match reply_rx.recv() {
            Ok(DiskReply::Sync {
                id: reply_id,
                result,
            }) if reply_id == id => result,
            Ok(reply) => Err(mismatched_reply(id, reply.id())),
            Err(_) => Err(StorageError::DiskThreadClosed),
        }
    }

    /// Asynchronously opens/creates every non-empty file in the torrent.
    pub fn submit_open(
        &self,
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
    ) -> Result<DiskOpHandle, StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.send(DiskCommand::Open {
            id,
            root: root.into(),
            files,
            reply: reply_tx,
        })?;
        Ok(DiskOpHandle { id, rx: reply_rx })
    }

    /// Opens or creates the files for a torrent and waits for completion.
    pub fn open(
        &self,
        root: impl Into<PathBuf>,
        files: Arc<Vec<TargetFileInfo>>,
    ) -> Result<(), StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.send_blocking(DiskCommand::Open {
            id,
            root: root.into(),
            files,
            reply: reply_tx,
        })?;
        match reply_rx.recv() {
            Ok(DiskReply::Open {
                id: reply_id,
                result,
            }) if reply_id == id => result,
            Ok(reply) => Err(mismatched_reply(id, reply.id())),
            Err(_) => Err(StorageError::DiskThreadClosed),
        }
    }

    /// Asynchronously flushes and closes every cached handle.
    pub fn submit_close(&self) -> Result<DiskOpHandle, StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.send(DiskCommand::Close {
            id,
            reply: reply_tx,
        })?;
        Ok(DiskOpHandle { id, rx: reply_rx })
    }

    /// Flushes and closes every cached handle, waiting for completion.
    pub fn close(&self) -> Result<(), StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.send_blocking(DiskCommand::Close {
            id,
            reply: reply_tx,
        })?;

        match reply_rx.recv() {
            Ok(DiskReply::Close {
                id: reply_id,
                result,
            }) if reply_id == id => result,
            Ok(reply) => Err(mismatched_reply(id, reply.id())),
            Err(_) => Err(StorageError::DiskThreadClosed),
        }
    }

    /// Drains queued writes, flushes file data and joins the worker.
    ///
    /// The `Shutdown` command is FIFO with every previously accepted write, so
    /// an acknowledged write can never be dropped by shutdown.
    pub fn shutdown(&self) {
        let _ = self.shutdown_result();
    }

    /// Like [`DiskThread::shutdown`] but reports the final sync/close result.
    pub fn shutdown_result(&self) -> Result<(), StorageError> {
        let id = self.next_id();
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);

        let result = match self.send_blocking(DiskCommand::Shutdown {
            id,
            reply: reply_tx,
        }) {
            Ok(()) => match reply_rx.recv() {
                Ok(DiskReply::Shutdown {
                    id: reply_id,
                    result,
                }) if reply_id == id => result,
                Ok(reply) => Err(mismatched_reply(id, reply.id())),
                Err(_) => Err(StorageError::DiskThreadClosed),
            },
            Err(err) => Err(err),
        };

        if let Ok(mut guard) = self.join.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }

        result
    }
}

impl Drop for DiskThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker_loop(rx: Receiver<DiskCommand>, stats: Arc<StatsInner>, manager: FileManager) {
    let mut worker = DiskWorker::new(manager, Arc::clone(&stats));
    let mut shutdown = false;

    while let Ok(command) = rx.recv() {
        stats.note_dequeued();
        stats.in_flight.fetch_add(1, Ordering::AcqRel);
        shutdown = worker.handle(command);
        stats.in_flight.fetch_sub(1, Ordering::AcqRel);

        if shutdown {
            break;
        }

        worker.close_idle();
    }

    // Channel closed without an explicit Shutdown: flush what we can.
    if !shutdown {
        let _ = worker.sync_all();
        worker.close_all();
    }
}

struct Segment {
    file_index: usize,
    file_offset: u64,
    data_start: usize,
    data_end: usize,
    padding: bool,
}

/// Torrent padding files are never opened. Their bytes are implicitly zero.
fn is_padding_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = match component.as_os_str().to_str() {
            Some(name) => name,
            None => return false,
        };

        name.starts_with(".pad")
            || name.starts_with("_____padding_file_")
            || name.starts_with("_____padding_file")
    })
}

/// Maps a byte range inside a piece to one or more files.
///
/// A piece may span any number of files, may start/end mid-file, and the final
/// piece may be shorter than `piece_length`.
fn range_segments(
    files: &[TargetFileInfo],
    piece_length: u32,
    piece_index: u32,
    block_offset: u32,
    length: usize,
) -> Vec<Segment> {
    let piece_start = (piece_index as u64).saturating_mul(piece_length as u64);
    let range_start = piece_start.saturating_add(block_offset as u64);
    let range_end = range_start.saturating_add(length as u64);

    let mut segments = Vec::new();
    for (file_index, file) in files.iter().enumerate() {
        if file.length == 0 {
            // Zero-length and padding files must never consume a descriptor.
            continue;
        }

        let file_start = file.offset;
        let file_end = file.offset.saturating_add(file.length);
        if range_start < file_end && range_end > file_start {
            let overlap_start = range_start.max(file_start);
            let overlap_end = range_end.min(file_end);
            if overlap_start < overlap_end {
                segments.push(Segment {
                    file_index,
                    file_offset: overlap_start - file_start,
                    data_start: (overlap_start - range_start) as usize,
                    data_end: (overlap_end - range_start) as usize,
                    padding: is_padding_path(&file.path),
                });
            }
        }
    }

    segments
}

fn resolve_path(root: &Path, file: &TargetFileInfo) -> PathBuf {
    let relative = file.path.strip_prefix(Path::new("/")).unwrap_or(&file.path);
    root.join(relative)
}

fn pwrite_all(fd: RawFd, mut data: &[u8], mut offset: u64) -> io::Result<()> {
    while !data.is_empty() {
        let written = unsafe {
            libc::pwrite(
                fd,
                data.as_ptr() as *const libc::c_void,
                data.len(),
                offset as libc::off_t,
            )
        };

        if written < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }

        if written == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "pwrite returned zero",
            ));
        }

        let written = written as usize;
        data = &data[written..];
        offset = offset.saturating_add(written as u64);
    }

    Ok(())
}

fn pread_exact(fd: RawFd, mut out: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !out.is_empty() {
        let read = unsafe {
            libc::pread(
                fd,
                out.as_mut_ptr() as *mut libc::c_void,
                out.len(),
                offset as libc::off_t,
            )
        };

        if read < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }

        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "pread reached end of file",
            ));
        }

        let read = read as usize;
        out = &mut out[read..];
        offset = offset.saturating_add(read as u64);
    }

    Ok(())
}

fn fsync_retry(fd: RawFd) -> io::Result<()> {
    loop {
        let ret = unsafe { libc::fsync(fd) };
        if ret == 0 {
            return Ok(());
        }

        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err);
    }
}

struct DiskWorker {
    manager: FileManager,
    files: HashMap<PathBuf, Box<File>>,
    last_used: HashMap<PathBuf, Instant>,
    last_close_idle_check: Instant,
    hash_queue: HashQueue,
    stats: Arc<StatsInner>,
}

impl DiskWorker {
    fn new(manager: FileManager, stats: Arc<StatsInner>) -> Self {
        Self {
            manager,
            files: HashMap::new(),
            last_used: HashMap::new(),
            last_close_idle_check: Instant::now(),
            hash_queue: HashQueue::new(),
            stats,
        }
    }

    fn handle(&mut self, command: DiskCommand) -> bool {
        match command {
            DiskCommand::Read {
                id,
                root,
                files,
                piece_length,
                piece_index,
                block_offset,
                length,
                reply,
            } => {
                let result = self.read_range(
                    &root,
                    files.as_slice(),
                    piece_length,
                    piece_index,
                    block_offset,
                    length,
                    false,
                );
                if result.is_ok() {
                    self.stats.reads_completed.fetch_add(1, Ordering::AcqRel);
                }
                let _ = reply.send(DiskReply::Read { id, data: result });
                false
            }
            DiskCommand::Write {
                id,
                root,
                files,
                piece_length,
                piece_index,
                block_offset,
                data,
                reply,
            } => {
                let bytes = data.len() as u64;
                let result = self.write_range(
                    &root,
                    files.as_slice(),
                    piece_length,
                    piece_index,
                    block_offset,
                    &data,
                );
                self.stats.release_write(bytes, result.is_ok());
                let _ = reply.send(DiskReply::Write { id, result });
                false
            }
            DiskCommand::Sync { id, reply } => {
                self.stats.syncs.fetch_add(1, Ordering::AcqRel);
                let result = self.sync_all();
                let _ = reply.send(DiskReply::Sync { id, result });
                false
            }
            DiskCommand::Hash {
                id,
                generation,
                root,
                files,
                piece_length,
                piece_index,
                piece_size,
                reply,
            } => {
                let result = self.hash_piece(
                    id,
                    &root,
                    files.as_slice(),
                    piece_length,
                    piece_index,
                    piece_size,
                );
                if result.is_ok() {
                    self.stats.hashes_completed.fetch_add(1, Ordering::AcqRel);
                }
                let _ = reply.send(DiskReply::Hash {
                    id,
                    generation,
                    piece_index,
                    hash: result,
                });
                false
            }
            DiskCommand::Open {
                id,
                root,
                files,
                reply,
            } => {
                let result = self.open_files(&root, files.as_slice());
                let _ = reply.send(DiskReply::Open { id, result });
                false
            }
            DiskCommand::Close { id, reply } => {
                let result = self.sync_all().and_then(|()| {
                    self.close_all();
                    Ok(())
                });
                let _ = reply.send(DiskReply::Close { id, result });
                false
            }
            DiskCommand::Shutdown { id, reply } => {
                // Every write accepted before this command has already been
                // executed because the request channel is FIFO.
                let result = self.sync_all().and_then(|()| {
                    self.close_all();
                    Ok(())
                });
                let _ = reply.send(DiskReply::Shutdown { id, result });
                true
            }
        }
    }

    fn purge_closed(&mut self) {
        let stale: Vec<PathBuf> = self
            .files
            .iter()
            .filter(|(_, file)| !file.is_open())
            .map(|(path, _)| path.to_path_buf())
            .collect();

        for path in stale {
            self.files.remove(&path);
            self.last_used.remove(&path);
        }
    }

    fn ensure_open(
        &mut self,
        path: &Path,
        create: bool,
        hashing: bool,
    ) -> Result<(), StorageError> {
        self.purge_closed();

        if let Some(file) = self.files.get(path) {
            let suitable = file.is_open() && (!create || file.protection().contains(Prot::WRITE));
            if suitable {
                self.last_used.insert(path.to_path_buf(), Instant::now());
                return Ok(());
            }
        }

        // A read-only handle must be replaced by a writable one (and vice
        // versa) rather than used with the wrong access mode.
        if let Some(mut file) = self.files.remove(path) {
            self.manager.close(&mut file);
        }
        self.last_used.remove(path);

        if create {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
        }

        let mut file = Box::new(File::new());
        file.set_path(path);

        let (flags, prot) = if create {
            (OpenFlags::CREATE, Prot::READ | Prot::WRITE)
        } else {
            (OpenFlags::empty(), Prot::READ)
        };

        if !self.manager.open(&mut file, hashing, prot, flags) {
            return Err(StorageError::Io(io::Error::new(
                if create {
                    io::ErrorKind::Other
                } else {
                    io::ErrorKind::NotFound
                },
                format!("could not open {}", path.display()),
            )));
        }

        self.files.insert(path.to_path_buf(), file);
        self.last_used.insert(path.to_path_buf(), Instant::now());
        Ok(())
    }

    fn open_files(&mut self, root: &Path, files: &[TargetFileInfo]) -> Result<(), StorageError> {
        for file in files {
            if file.length == 0 || is_padding_path(&file.path) {
                continue;
            }
            let path = resolve_path(root, file);
            self.ensure_open(&path, true, false)?;
        }
        Ok(())
    }

    fn close_idle(&mut self) {
        // Honour the FileManager's configured idle timeout (`system.files.close_idle`)
        // and scan at most four times per timeout, like `periodic_close_idle`.
        let idle_secs = self.manager.close_idle_secs();
        if idle_secs == 0 {
            return;
        }

        let idle = Duration::from_secs(idle_secs);
        let interval = (idle / 4).max(Duration::from_millis(1));
        let now = Instant::now();
        if now.duration_since(self.last_close_idle_check) < interval {
            return;
        }
        self.last_close_idle_check = now;

        let stale: Vec<PathBuf> = self
            .last_used
            .iter()
            .filter(|(path, last)| {
                now.duration_since(**last) >= idle
                    && self
                        .files
                        .get(*path)
                        .map(|file| !self.manager.is_in_flight(file))
                        .unwrap_or(false)
            })
            .map(|(path, _)| path.to_path_buf())
            .collect();

        for path in stale {
            if let Some(mut file) = self.files.remove(&path) {
                self.manager.close(&mut file);
            }
            self.last_used.remove(&path);
        }
    }

    fn write_range(
        &mut self,
        root: &Path,
        files: &[TargetFileInfo],
        piece_length: u32,
        piece_index: u32,
        block_offset: u32,
        data: &[u8],
    ) -> Result<(), StorageError> {
        if data.is_empty() {
            return Ok(());
        }

        if block_offset as u64 + data.len() as u64 > piece_length as u64 {
            return Err(StorageError::InvalidRange {
                offset: (piece_index as u64).saturating_mul(piece_length as u64),
                length: block_offset as u64 + data.len() as u64,
                size: piece_length as u64,
            });
        }

        let segments = range_segments(files, piece_length, piece_index, block_offset, data.len());
        let covered: usize = segments
            .iter()
            .map(|seg| seg.data_end.saturating_sub(seg.data_start))
            .sum();
        if covered != data.len() {
            return Err(StorageError::InvalidRange {
                offset: (piece_index as u64)
                    .saturating_mul(piece_length as u64)
                    .saturating_add(block_offset as u64),
                length: data.len() as u64,
                size: covered as u64,
            });
        }

        for segment in segments {
            if segment.padding {
                // Padding is virtual zeroes; never create or open a file.
                continue;
            }

            let target = &files[segment.file_index];
            let path = resolve_path(root, target);
            self.ensure_open(&path, true, false)?;

            let file = self.files.get(&path).ok_or(StorageError::FileNotOpen)?;
            let fd = file.file_descriptor();
            self.manager.begin_io(file);
            let result = pwrite_all(
                fd,
                &data[segment.data_start..segment.data_end],
                segment.file_offset,
            );
            self.manager.end_io(file);
            result?;
            self.last_used.insert(path, Instant::now());
        }

        Ok(())
    }

    fn read_range(
        &mut self,
        root: &Path,
        files: &[TargetFileInfo],
        piece_length: u32,
        piece_index: u32,
        block_offset: u32,
        length: u32,
        hashing: bool,
    ) -> Result<Vec<u8>, StorageError> {
        if block_offset as u64 + length as u64 > piece_length as u64 {
            return Err(StorageError::InvalidRange {
                offset: (piece_index as u64).saturating_mul(piece_length as u64),
                length: block_offset as u64 + length as u64,
                size: piece_length as u64,
            });
        }

        let mut out = vec![0u8; length as usize];
        if length == 0 {
            return Ok(out);
        }

        let segments = range_segments(files, piece_length, piece_index, block_offset, out.len());
        let covered: usize = segments
            .iter()
            .map(|seg| seg.data_end.saturating_sub(seg.data_start))
            .sum();
        if covered != out.len() {
            return Err(StorageError::InvalidRange {
                offset: (piece_index as u64)
                    .saturating_mul(piece_length as u64)
                    .saturating_add(block_offset as u64),
                length: out.len() as u64,
                size: covered as u64,
            });
        }

        for segment in segments {
            if segment.padding {
                // The output buffer is zero-initialised, matching padding.
                continue;
            }

            let target = &files[segment.file_index];
            let path = resolve_path(root, target);
            self.ensure_open(&path, false, hashing)?;

            let file = self.files.get(&path).ok_or(StorageError::FileNotOpen)?;
            let fd = file.file_descriptor();
            self.manager.begin_io(file);
            let result = pread_exact(
                fd,
                &mut out[segment.data_start..segment.data_end],
                segment.file_offset,
            );
            self.manager.end_io(file);
            result?;
            self.last_used.insert(path, Instant::now());
        }

        Ok(out)
    }

    fn hash_piece(
        &mut self,
        id: u64,
        root: &Path,
        files: &[TargetFileInfo],
        piece_length: u32,
        piece_index: u32,
        piece_size: u32,
    ) -> Result<[u8; 20], StorageError> {
        // Read a whole piece through the worker, in bounded reads so a very
        // large piece does not issue one enormous `pread`.
        let mut data = vec![0u8; piece_size as usize];
        let mut offset = 0u32;
        while offset < piece_size {
            let chunk = (piece_size - offset).min(HASH_READ_CHUNK as u32);
            let part =
                self.read_range(root, files, piece_length, piece_index, offset, chunk, true)?;
            data[offset as usize..offset as usize + chunk as usize].copy_from_slice(&part);
            offset += chunk;
        }

        // Hash commands travel through the same HashQueue the legacy
        // HashCheckQueue used; the callback carries the result back to the
        // requesting caller.
        let slot: Arc<Mutex<Option<HashString>>> = Arc::new(Mutex::new(None));
        let slot_for_cb = Arc::clone(&slot);
        self.hash_queue.push_back(
            ChunkHandle::new(piece_index, false, false),
            id as usize,
            data,
            move |_handle, hash| {
                if let Ok(mut slot) = slot_for_cb.lock() {
                    *slot = hash;
                }
            },
        );
        self.hash_queue.work();

        let hash = slot
            .lock()
            .map_err(|_| StorageError::InvalidChunk)?
            .take()
            .ok_or(StorageError::InvalidChunk)?;
        Ok(*hash.as_bytes())
    }

    fn sync_all(&mut self) -> Result<(), StorageError> {
        let mut first_error = None;
        for file in self.files.values() {
            if !file.is_open() {
                continue;
            }
            if let Err(err) = fsync_retry(file.file_descriptor()) {
                if first_error.is_none() {
                    first_error = Some(StorageError::Io(err));
                }
            }
        }
        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn close_all(&mut self) {
        let paths: Vec<PathBuf> = self.files.keys().cloned().collect();
        for path in paths {
            if let Some(mut file) = self.files.remove(&path) {
                self.manager.close(&mut file);
            }
        }
        self.last_used.clear();
    }
}

static GLOBAL_DISK_THREAD: OnceLock<Option<Arc<DiskThread>>> = OnceLock::new();

/// Lazily-created process-wide disk thread.
pub fn global_disk_thread() -> Option<Arc<DiskThread>> {
    GLOBAL_DISK_THREAD
        .get_or_init(|| DiskThread::spawn(DEFAULT_DISK_QUEUE).ok().map(Arc::new))
        .clone()
}

/// Snapshot of the global disk worker's [`FileManager`] counters, if the
/// worker has already been created.
///
/// Unlike [`global_disk_thread`] this never spawns a worker just to read
/// statistics, so command handlers can poll it cheaply. Public path:
/// `lt_storage::disk_thread::global_file_manager_stats`.
pub fn global_file_manager_stats() -> Option<FileManagerStats> {
    GLOBAL_DISK_THREAD
        .get()
        .and_then(|thread| thread.as_ref())
        .map(|thread| thread.file_stats())
}

/// Flushes and stops the process-wide disk thread, if it was created.
pub fn shutdown_global_disk_thread() {
    if let Some(thread) = GLOBAL_DISK_THREAD.get().and_then(|opt| opt.as_ref()) {
        thread.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rtorrent_disk_thread_{}_{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn writes_piece_and_reports_completion() {
        let root = temp_root("single");
        let files = Arc::new(vec![TargetFileInfo {
            path: PathBuf::from("payload.bin"),
            length: 8,
            offset: 0,
        }]);

        let thread = DiskThread::spawn(4).unwrap();
        let handle = thread
            .submit_write(&root, files, 4, 0, b"abcd".to_vec())
            .unwrap();

        handle.wait().unwrap();
        thread.shutdown();

        assert_eq!(fs::read(root.join("payload.bin")).unwrap(), b"abcd");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn writes_piece_spanning_files() {
        let root = temp_root("span");
        let files = Arc::new(vec![
            TargetFileInfo {
                path: PathBuf::from("a.bin"),
                length: 3,
                offset: 0,
            },
            TargetFileInfo {
                path: PathBuf::from("b.bin"),
                length: 5,
                offset: 3,
            },
        ]);

        let thread = DiskThread::spawn(4).unwrap();
        thread
            .submit_write(&root, files, 8, 0, b"abcdefgh".to_vec())
            .unwrap()
            .wait()
            .unwrap();
        thread.shutdown();

        assert_eq!(fs::read(root.join("a.bin")).unwrap(), b"abc");
        assert_eq!(fs::read(root.join("b.bin")).unwrap(), b"defgh");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn partial_block_write_only_touches_its_range() {
        let root = temp_root("partial");
        let files = Arc::new(vec![TargetFileInfo {
            path: PathBuf::from("partial.bin"),
            length: 16,
            offset: 0,
        }]);

        let thread = DiskThread::spawn(4).unwrap();
        thread
            .submit_write(&root, Arc::clone(&files), 16, 0, vec![0xAA; 16])
            .unwrap()
            .wait()
            .unwrap();
        thread
            .submit_write_block(&root, Arc::clone(&files), 16, 0, 4, b"01234567".to_vec())
            .unwrap()
            .wait()
            .unwrap();
        thread.shutdown();

        let content = fs::read(root.join("partial.bin")).unwrap();
        assert_eq!(&content[0..4], &[0xAA; 4]);
        assert_eq!(&content[4..12], b"01234567");
        assert_eq!(&content[12..16], &[0xAA; 4]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn read_after_write_returns_the_same_bytes_across_files() {
        let root = temp_root("order");
        let files = Arc::new(vec![
            TargetFileInfo {
                path: PathBuf::from("a.bin"),
                length: 5,
                offset: 0,
            },
            TargetFileInfo {
                path: PathBuf::from("b.bin"),
                length: 11,
                offset: 5,
            },
        ]);

        let thread = DiskThread::spawn(8).unwrap();
        let w0 = thread
            .submit_write(&root, Arc::clone(&files), 8, 0, b"abcdefgh".to_vec())
            .unwrap();
        let w1 = thread
            .submit_write(&root, Arc::clone(&files), 8, 1, b"ijklmnop".to_vec())
            .unwrap();
        w0.wait().unwrap();
        w1.wait().unwrap();

        let read = thread
            .read_block(&root, Arc::clone(&files), 8, 0, 3, 5)
            .unwrap();
        assert_eq!(read, b"defgh");

        let result = thread
            .submit_hash(&root, Arc::clone(&files), 8, 1, 8, 42)
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(result.generation, 42);
        assert_eq!(result.piece_index, 1);
        assert_eq!(result.hash, lt_crypto::sha1::sha1_hash(b"ijklmnop"));

        thread.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn shutdown_drains_queued_writes_and_syncs() {
        let root = temp_root("drain");
        let files = Arc::new(vec![TargetFileInfo {
            path: PathBuf::from("drain.bin"),
            length: 64,
            offset: 0,
        }]);

        let thread = DiskThread::spawn(8).unwrap();
        for index in 0..8u32 {
            thread
                .submit_write_block(
                    &root,
                    Arc::clone(&files),
                    64,
                    0,
                    index * 8,
                    vec![index as u8; 8],
                )
                .expect("write should be accepted");
        }

        // No handle is polled: shutdown must execute every queued write.
        thread.shutdown();
        let content = fs::read(root.join("drain.bin")).unwrap();
        assert_eq!(content.len(), 64);
        for index in 0..8usize {
            assert_eq!(&content[index * 8..(index + 1) * 8], &[index as u8; 8]);
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_queue_full_instead_of_dropping() {
        let root = temp_root("full");
        let files = Arc::new(vec![TargetFileInfo {
            path: PathBuf::from("x.bin"),
            length: 1,
            offset: 0,
        }]);

        let thread = DiskThread::spawn(1).unwrap();
        let mut full = false;
        for _ in 0..10_000 {
            match thread.submit_write(&root, Arc::clone(&files), 1, 0, vec![0u8]) {
                Ok(_) => {}
                Err(StorageError::DiskQueueFull) => {
                    full = true;
                    break;
                }
                Err(err) => panic!("unexpected error: {err}"),
            }
        }
        assert!(full, "expected the bounded queue to fill");

        thread.shutdown();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn correlation_ids_are_unique_and_replies_match() {
        let root = temp_root("correlation");
        let files = Arc::new(vec![TargetFileInfo {
            path: PathBuf::from("id.bin"),
            length: 4,
            offset: 0,
        }]);

        let thread = DiskThread::spawn(4).unwrap();
        let first = thread
            .submit_write(&root, Arc::clone(&files), 4, 0, b"1234".to_vec())
            .unwrap();
        let second = thread
            .submit_write(&root, Arc::clone(&files), 4, 0, b"5678".to_vec())
            .unwrap();
        assert_ne!(first.id(), second.id());
        first.wait().unwrap();
        second.wait().unwrap();
        thread.shutdown();

        let _ = fs::remove_dir_all(&root);
    }
}
