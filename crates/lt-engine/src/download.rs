use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use lt_common::bitfield::Bitfield;
use lt_common::collections::PartialQueue;
use lt_common::hash_string::HashString;
use lt_common::rate::Rate;
use lt_crypto::sha1::sha1_hash;
use lt_protocol::connection::PeerConnection;
pub use lt_protocol::connection::{PeerDirection, PeerInfo};
use lt_protocol::initial_seed::InitialSeeding;
use lt_protocol::metadata::MetadataDownload;
use lt_protocol::peer_list::ConnectionList;
use lt_storage::chunk_list::ChunkList;
use lt_storage::chunk_manager::ChunkManager;
use lt_storage::disk_thread::{
    global_disk_thread, DiskHashHandle, DiskThread, DiskThreadStats, DiskWriteHandle,
};
use lt_storage::file::File;
use lt_storage::file_list::FileList;
use lt_storage::hash_check_queue::{
    HashCheckJob, HashCheckQueue, HashCheckQueueStats, FLAG_NEED, FLAG_WILL_NEED,
};
use lt_storage::hash_torrent::{HashResult, HashTorrent};
use lt_storage::piece::Piece;
use lt_storage::resume::{
    ResumeData, MTIME_DOWNLOADING, MTIME_RECHECK, MTIME_SKIPPED, MTIME_UNCHECKED,
};
use lt_storage::StorageError;
use lt_tracker::controller::TrackerController;
use lt_tracker::list::TrackerList;
use thiserror::Error;

use crate::chunk_selector::ChunkSelector;
use crate::chunk_statistics::ChunkStatistics;
use crate::client_config::ClientConfig;
use crate::constructor::{DownloadConstructor, TorrentMetadata};
use crate::delegator::Delegator;

/// Lifecycle state of an engine download.
///
/// The state machine follows rTorrent's download lifecycle:
///
/// ```text
/// Closed <-> Stopped <-> Checking
///                |  \        |
///                |   \       v
///                v    +-> Downloading <-> Seeding
///              Paused <-------+  |
///                |              |
///                +--------------+
/// ```
///
/// `open`/`close` control the `Closed` boundary, while `start`/`stop` move
/// between `Stopped`, `Downloading`, `Seeding` and `Paused`. Hash checking is
/// represented by `Checking` and always returns to the state it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadState {
    /// Files/metadata have been released. Start is refused.
    Closed,
    /// Open but not running.
    Stopped,
    /// Rebuilding the completed bitfield from disk.
    Checking,
    /// Open and transferring (leeching).
    Downloading,
    /// Open, complete and transferring (seeding).
    Seeding,
    /// Open but intentionally not running.
    Paused,
    /// Open but failed. The message is diagnostic only.
    Error(String),
}

impl DownloadState {
    /// Whether a transition from `self` to `next` is legal.
    ///
    /// Re-applying the same state is always legal so callers can be
    /// idempotent; the engine treats it as a no-op and does not bump counters
    /// or emit events.
    pub fn can_transition_to(&self, next: &DownloadState) -> bool {
        use DownloadState::*;

        if self == next {
            return true;
        }

        matches!(
            (self, next),
            // Opening/closing the download.
            (Closed, Stopped)
                | (Closed, Error(_))
                | (Stopped, Closed)
                | (Checking, Closed)
                | (Downloading, Closed)
                | (Seeding, Closed)
                | (Paused, Closed)
                | (Error(_), Closed)
                // Starting/stopping.
                | (Stopped, Checking)
                | (Downloading, Checking)
                | (Seeding, Checking)
                | (Paused, Checking)
                | (Stopped, Downloading)
                | (Stopped, Seeding)
                | (Paused, Downloading)
                | (Paused, Seeding)
                | (Downloading, Seeding)
                | (Seeding, Downloading)
                | (Downloading, Paused)
                | (Seeding, Paused)
                | (Downloading, Stopped)
                | (Seeding, Stopped)
                | (Paused, Stopped)
                | (Checking, Stopped)
                | (Checking, Downloading)
                | (Checking, Seeding)
                | (Checking, Paused)
                // Failure handling.
                | (Stopped, Error(_))
                | (Checking, Error(_))
                | (Downloading, Error(_))
                | (Seeding, Error(_))
                | (Paused, Error(_))
                | (Error(_), Stopped)
                | (Error(_), Checking)
                | (Error(_), Error(_))
        )
    }

    pub fn is_closed(&self) -> bool {
        matches!(self, DownloadState::Closed)
    }

    pub fn is_checking(&self) -> bool {
        matches!(self, DownloadState::Checking)
    }

    pub fn is_active(&self) -> bool {
        matches!(self, DownloadState::Downloading | DownloadState::Seeding)
    }

    pub fn is_stopped(&self) -> bool {
        matches!(self, DownloadState::Stopped)
    }

    pub fn is_paused(&self) -> bool {
        matches!(self, DownloadState::Paused)
    }

    pub fn is_error(&self) -> bool {
        matches!(self, DownloadState::Error(_))
    }

    /// rTorrent-compatible `d.state` value: 0 stopped, 1 started.
    pub fn as_rpc_value(&self) -> i64 {
        if self.is_active() {
            1
        } else {
            0
        }
    }
}

/// Reason a peer was refused admission to a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PeerAddError {
    #[error("maximum peer count reached")]
    MaxPeers,
    #[error("maximum connection limit reached")]
    ConnectionLimit,
    #[error("maximum seeder peer count reached")]
    MaxSeedPeers,
    #[error("seeder peers are disabled")]
    SeedersDisabled,
    #[error("peer is already connected")]
    DuplicatePeer,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn file_mtime_seconds(metadata: &std::fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

pub struct Download {
    metadata: TorrentMetadata,
    state: DownloadState,
    is_open: bool,
    state_changed: u64,
    state_counter: u64,
    timestamp_started: u64,
    timestamp_finished: u64,
    load_date: u64,
    file_list: FileList,
    chunk_list: ChunkList,
    chunk_selector: ChunkSelector,
    chunk_statistics: ChunkStatistics,
    delegator: Delegator,
    connection_list: ConnectionList,
    tracker_controller: TrackerController,
    peer_caches: HashMap<usize, PartialQueue>,
    piece_data_cache: HashMap<u32, Vec<u8>>,

    rate_up: Rate,
    rate_down: Rate,
    uploaded_bytes: u64,
    downloaded_bytes: u64,
    save_directory: std::path::PathBuf,

    target_files: Arc<Vec<lt_storage::TargetFileInfo>>,
    disk_thread: Option<Arc<DiskThread>>,
    pending_writes: Vec<PendingDiskWrite>,
    /// Verified pieces that could not be submitted because the bounded disk
    /// queue was full. They are retried from `poll_disk_completions`; the
    /// reactor never falls back to synchronous file I/O.
    queued_writes: VecDeque<QueuedDiskWrite>,
    pending_hashes: Vec<PendingDiskHash>,
    /// Priority queue for recheck and live verification work. Entries are
    /// deduplicated and remain here until their async result is harvested.
    hash_check_queue: HashCheckQueue,
    /// Incremented whenever the chunk generation changes. Hash replies from an
    /// older generation are discarded rather than applied to the new chunks.
    disk_generation: u64,
    discarded_hash_results: u64,
    metadata_download: Option<MetadataDownload>,
    discovered_peers: HashSet<SocketAddr>,
    client_config: ClientConfig,
    accepting_seeders: bool,
    initial_seeding: Option<InitialSeeding>,
    resume_mtimes: Vec<i64>,
    resume_completed_bytes: u64,
    /// Pieces marked complete from a disk write that has not been fsynced
    /// since. Serialized into the resume extras so a crash cannot make the
    /// next process trust page-cache-only data.
    unsynced_pieces: BTreeSet<u32>,
    hash_checked: bool,
    hash_failed: bool,
    hash_failed_pieces: Vec<u32>,
    pending_hash_pieces: Vec<u32>,
    chunks_hashed: Arc<AtomicU32>,
}

struct PendingDiskWrite {
    piece_index: u32,
    handle: DiskWriteHandle,
}

struct QueuedDiskWrite {
    piece_index: u32,
    data: Vec<u8>,
}

struct PendingDiskHash {
    piece_index: u32,
    generation: u64,
    handle: DiskHashHandle,
    /// Token of the [`HashCheckJob`] this command was submitted for. Results
    /// for a removed/re-enqueued chunk are discarded by the queue.
    token: u64,
}

impl Download {
    pub fn new(metadata: TorrentMetadata) -> Self {
        let num_pieces = metadata.num_pieces;
        let piece_length = metadata.piece_length;

        let mut file_list = FileList::new();
        file_list.set_multi_file(metadata.is_multi_file);

        if !metadata.files.is_empty() {
            for f_entry in &metadata.files {
                let mut f = File::new();
                f.set_path(f_entry.path.as_string());
                f.set_size_bytes(f_entry.length);
                f.set_offset(f_entry.offset);
                file_list.push_file(f);
            }
            let _ = file_list.initialize(metadata.total_size, piece_length);
        }

        let chunk_manager = Arc::new(ChunkManager::new());
        let mut chunk_list = ChunkList::new();
        chunk_list.set_chunk_size(piece_length);
        chunk_list.set_manager(chunk_manager);
        chunk_list.resize(num_pieces);

        let chunk_selector = ChunkSelector::new(num_pieces);
        let chunk_statistics = ChunkStatistics::new(num_pieces);
        let delegator = Delegator::new();
        let connection_list = ConnectionList::new();

        let mut tracker_list = TrackerList::new();
        for (g_idx, group) in metadata.announce_list.iter().enumerate() {
            for tracker_url in group {
                tracker_list.insert_url(g_idx as u32, tracker_url, false);
            }
        }
        if tracker_list.is_empty() {
            if let Some(ref ann) = metadata.announce {
                tracker_list.insert_url(0, ann, false);
            }
        }
        let tracker_controller = TrackerController::new(tracker_list);

        let metadata_is_metadata = metadata.is_meta_download;
        let metadata_info_hash = metadata.info_hash;

        let target_files: Arc<Vec<lt_storage::TargetFileInfo>> = Arc::new(
            metadata
                .files
                .iter()
                .map(|f| lt_storage::TargetFileInfo {
                    // Torrent paths are stored as "/dir/file"; strip the leading
                    // slash so they remain relative to the save directory.
                    path: std::path::PathBuf::from(f.path.as_string().trim_start_matches('/')),
                    length: f.length,
                    offset: f.offset,
                })
                .collect(),
        );

        let now = unix_now();
        Self {
            metadata,
            // Like rTorrent, a newly constructed download must be opened
            // before it is usable. `start()` retains a legacy compatibility
            // path that opens a never-opened download automatically.
            state: DownloadState::Stopped,
            is_open: false,
            state_changed: now,
            state_counter: 0,
            timestamp_started: 0,
            timestamp_finished: 0,
            load_date: now,
            file_list,
            chunk_list,
            chunk_selector,
            chunk_statistics,
            delegator,
            connection_list,
            tracker_controller,
            peer_caches: HashMap::new(),
            piece_data_cache: HashMap::new(),

            rate_up: Rate::new(20),
            rate_down: Rate::new(20),
            uploaded_bytes: 0,
            downloaded_bytes: 0,
            save_directory: std::path::PathBuf::new(),

            target_files,
            disk_thread: global_disk_thread(),
            pending_writes: Vec::new(),
            queued_writes: VecDeque::new(),
            pending_hashes: Vec::new(),
            hash_check_queue: HashCheckQueue::new(),
            disk_generation: 0,
            discarded_hash_results: 0,
            metadata_download: if metadata_is_metadata {
                Some(MetadataDownload::new(metadata_info_hash))
            } else {
                None
            },
            discovered_peers: HashSet::new(),
            client_config: ClientConfig::default(),
            accepting_seeders: true,
            initial_seeding: None,
            resume_mtimes: Vec::new(),
            resume_completed_bytes: 0,
            unsynced_pieces: BTreeSet::new(),
            hash_checked: false,
            hash_failed: false,
            hash_failed_pieces: Vec::new(),
            pending_hash_pieces: Vec::new(),
            chunks_hashed: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Returns the disk worker used for asynchronous piece writes.
    pub fn disk_thread(&self) -> Option<&Arc<DiskThread>> {
        self.disk_thread.as_ref()
    }

    /// Overrides the disk worker. Passing `None` disables the asynchronous
    /// queue; verified pieces are then written through the process-wide worker
    /// and awaited by the caller, so completion is visible immediately without
    /// issuing file I/O on the caller's thread.
    pub fn set_disk_thread(&mut self, thread: Option<Arc<DiskThread>>) {
        self.disk_thread = thread;
    }

    /// Number of pieces verified but not yet durably written by the worker.
    pub fn pending_disk_writes(&self) -> usize {
        self.pending_writes.len() + self.queued_writes.len()
    }

    /// Number of write commands accepted by the worker but not yet completed.
    pub fn disk_sync_queue_depth(&self) -> usize {
        self.queued_writes.len()
            + self
                .disk_thread
                .as_ref()
                .map(|thread| thread.sync_queue_depth())
                .unwrap_or(0)
    }

    /// Worker queue and I/O counters, if a disk thread is configured.
    pub fn disk_stats(&self) -> Option<DiskThreadStats> {
        self.disk_thread.as_ref().map(|thread| thread.stats())
    }

    /// Submits as many backpressured writes as the bounded queue accepts.
    fn retry_queued_writes(&mut self) {
        let Some(thread) = self.disk_thread.clone() else {
            return;
        };

        while let Some(queued) = self.queued_writes.pop_front() {
            match thread.try_submit_write_block(
                self.save_directory.clone(),
                Arc::clone(&self.target_files),
                self.metadata.piece_length,
                queued.piece_index,
                0,
                queued.data,
            ) {
                Ok(handle) => self.pending_writes.push(PendingDiskWrite {
                    piece_index: queued.piece_index,
                    handle,
                }),
                Err((StorageError::DiskQueueFull, data)) => {
                    // Keep the piece buffered and apply backpressure until the
                    // next poll. Losing it here would corrupt the torrent.
                    self.queued_writes.push_front(QueuedDiskWrite {
                        piece_index: queued.piece_index,
                        data,
                    });
                    break;
                }
                Err((err, _data)) => {
                    tracing::error!(
                        piece = queued.piece_index,
                        error = %err,
                        "Could not retry queued disk write; piece will be requested again"
                    );
                    self.chunk_selector.not_using_index(queued.piece_index);
                    let _ = self.delegator.transfers_mut().erase(queued.piece_index);
                }
            }
        }
    }

    /// Polls the disk worker for finished writes.
    ///
    /// This is intentionally non-blocking and must be called from the network
    /// supervisor (or any periodic tick). It returns the number of pieces that
    /// became complete during this call.
    pub fn poll_disk_completions(&mut self) -> usize {
        self.retry_queued_writes();

        let mut completed = 0;
        let mut index = 0;

        while index < self.pending_writes.len() {
            let result = self.pending_writes[index].handle.try_finish();

            match result {
                None => {
                    index += 1;
                }
                Some(Ok(())) => {
                    let pending = self.pending_writes.remove(index);
                    // The worker completed the write but did not fsync it; the
                    // next resume snapshot must not trust this piece.
                    self.unsynced_pieces.insert(pending.piece_index);
                    self.mark_piece_complete(pending.piece_index);
                    completed += 1;
                }
                Some(Err(err)) => {
                    let pending = self.pending_writes.remove(index);
                    tracing::error!(
                        piece = pending.piece_index,
                        error = %err,
                        "Disk write failed; piece will be requested again"
                    );
                    self.chunk_selector.not_using_index(pending.piece_index);
                    let _ = self.delegator.transfers_mut().erase(pending.piece_index);
                }
            }
        }

        if completed > 0 {
            // A completed write may have freed queue capacity.
            self.retry_queued_writes();
        }

        completed
    }

    /// Flushes all pending writes and cached file handles. Blocking; use on
    /// shutdown or explicit sync, never per piece.
    pub fn flush_disk(&mut self) -> Result<(), StorageError> {
        if !self.queued_writes.is_empty() && self.disk_thread.is_none() {
            return Err(StorageError::DiskThreadClosed);
        }

        // Wait for queued writes to drain first.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !self.pending_writes.is_empty() || !self.queued_writes.is_empty() {
            self.poll_disk_completions();

            if self.pending_writes.is_empty() && self.queued_writes.is_empty() {
                break;
            }

            if std::time::Instant::now() >= deadline {
                return Err(StorageError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timed out waiting for pending disk writes",
                )));
            }

            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        match self.disk_thread.clone().or_else(global_disk_thread) {
            Some(thread) => {
                thread.sync_all()?;
                // Everything written before this sync is now durable and can
                // be trusted by a future resume load.
                self.unsynced_pieces.clear();
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Current generation used to reject stale hash completions.
    pub fn disk_generation(&self) -> u64 {
        self.disk_generation
    }

    /// Invalidates every hash request submitted before this call.
    pub fn bump_disk_generation(&mut self) -> u64 {
        self.disk_generation = self.disk_generation.wrapping_add(1);
        self.disk_generation
    }

    /// Number of hash results dropped because their generation was stale.
    pub fn discarded_hash_results(&self) -> u64 {
        self.discarded_hash_results
    }

    /// Number of outstanding asynchronous hash checks.
    pub fn pending_disk_hashes(&self) -> usize {
        self.pending_hashes.len()
    }

    /// Priority verification queue used by recheck and live hash checks.
    pub fn hash_check_queue(&self) -> &HashCheckQueue {
        &self.hash_check_queue
    }

    pub fn hash_check_queue_mut(&mut self) -> &mut HashCheckQueue {
        &mut self.hash_check_queue
    }

    /// Number of queued and in-flight hash checks. Suitable for
    /// `pieces.hash.queue_size`.
    pub fn hash_queue_size(&self) -> usize {
        self.hash_check_queue.pending_size()
    }

    /// Alias for [`Download::hash_queue_size`].
    pub fn hash_queue_len(&self) -> usize {
        self.hash_queue_size()
    }

    /// Queue depths and lifetime counters.
    pub fn hash_queue_stats(&self) -> HashCheckQueueStats {
        self.hash_check_queue.stats()
    }

    /// Alias for [`Download::hash_queue_stats`].
    pub fn hash_queue_counters(&self) -> HashCheckQueueStats {
        self.hash_queue_stats()
    }

    pub fn hash_queue_in_flight(&self) -> usize {
        self.hash_check_queue.in_flight_size()
    }

    /// Number of pieces checked so far during hash check.
    pub fn chunks_hashed(&self) -> u32 {
        self.chunks_hashed.load(Ordering::Relaxed)
    }

    /// Arc clone for accessing chunks_hashed counter concurrently.
    pub fn chunks_hashed_arc(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.chunks_hashed)
    }

    /// Queues hash checks for `pieces` on the disk worker at need priority.
    ///
    /// Returns the number of chunks newly enqueued; duplicates (including
    /// chunks already in flight) are ignored. Results are applied by
    /// [`Download::poll_disk_hash_completions`]. The current disk generation is
    /// captured for every request so a result that arrives after a
    /// close/recheck/metadata change is discarded.
    pub fn submit_hash_check(&mut self, pieces: &[u32]) -> Result<usize, String> {
        self.submit_hash_check_prioritized(pieces, &[])
    }

    /// Queues hash checks with explicit need / will-need priorities.
    ///
    /// Need chunks are verified first; will-need chunks are held in the lower
    /// tier with FIFO ordering and fairness aging so they cannot starve.
    pub fn submit_hash_check_prioritized(
        &mut self,
        need: &[u32],
        will_need: &[u32],
    ) -> Result<usize, String> {
        if self.disk_thread.is_none() {
            return Err("no disk thread configured".to_string());
        }

        let mut inserted = 0;
        for &piece_index in need.iter().chain(will_need.iter()) {
            let flags = if need.contains(&piece_index) {
                FLAG_NEED
            } else {
                FLAG_WILL_NEED
            };
            if piece_index as usize >= self.metadata.num_pieces {
                continue;
            }
            if self.hash_check_queue.insert(piece_index, flags) {
                inserted += 1;
            }
        }

        self.pump_hash_queue();
        Ok(inserted)
    }

    /// Cancels a queued or in-flight hash check.
    ///
    /// An in-flight chunk stays accounted for until the worker replies, but
    /// its result is discarded by [`Download::poll_disk_hash_completions`].
    pub fn remove_hash_check(&mut self, piece_index: u32) -> bool {
        self.hash_check_queue.remove(piece_index)
    }

    /// Enqueues `pieces` at need priority and drains them in queue order.
    ///
    /// The returned jobs are still marked in flight; the caller must finish or
    /// cancel each token once the corresponding hash has been processed.
    fn queue_hash_jobs(&mut self, pieces: &[u32]) -> Vec<HashCheckJob> {
        for &piece_index in pieces {
            if piece_index as usize >= self.metadata.num_pieces {
                continue;
            }
            self.hash_check_queue.insert(piece_index, FLAG_NEED);
        }

        let mut jobs = Vec::with_capacity(self.hash_check_queue.size());
        while let Some(job) = self.hash_check_queue.pop_next_job() {
            jobs.push(job);
        }
        jobs
    }

    /// Clears every queued/in-flight hash and invalidates worker replies.
    fn clear_hash_check_queue(&mut self) {
        self.hash_check_queue.clear();
        self.pending_hashes.clear();
        self.bump_disk_generation();
    }

    /// Submits queued hash jobs while the bounded disk queue has capacity.
    fn pump_hash_queue(&mut self) {
        let Some(thread) = self.disk_thread.clone() else {
            return;
        };

        while let Some(job) = self.hash_check_queue.pop_next_job() {
            let piece_index = job.index;
            if piece_index as usize >= self.metadata.num_pieces {
                self.hash_check_queue.fail_token(piece_index, job.token);
                continue;
            }

            let generation = self.disk_generation;
            let piece_size = self.chunk_size(piece_index);
            match thread.submit_hash(
                self.save_directory.clone(),
                Arc::clone(&self.target_files),
                self.metadata.piece_length,
                piece_index,
                piece_size,
                generation,
            ) {
                Ok(handle) => self.pending_hashes.push(PendingDiskHash {
                    piece_index,
                    generation,
                    handle,
                    token: job.token,
                }),
                Err(StorageError::DiskQueueFull) => {
                    // Keep the job at the front of its tier and retry on the
                    // next poll; dropping it would silently skip a piece.
                    self.hash_check_queue.requeue(job);
                    break;
                }
                Err(err) => {
                    self.hash_check_queue.fail_token(piece_index, job.token);
                    tracing::error!(
                        piece = piece_index,
                        error = %err,
                        "Could not queue disk hash; piece will be requested again"
                    );
                    self.chunk_selector.not_using_index(piece_index);
                    let _ = self.delegator.transfers_mut().erase(piece_index);
                }
            }
        }
    }

    /// Applies finished asynchronous hash checks.
    ///
    /// Returns the number of results that matched the current generation and
    /// were applied to the completed bitfield. Results for a chunk that was
    /// removed while in flight, or whose disk generation changed, are counted
    /// in [`Download::discarded_hash_results`] and never mutate download state.
    pub fn poll_disk_hash_completions(&mut self) -> usize {
        let mut applied = 0;
        let mut index = 0;

        while index < self.pending_hashes.len() {
            let result = self.pending_hashes[index].handle.try_finish();

            match result {
                None => {
                    index += 1;
                }
                Some(Ok(result)) => {
                    let pending = self.pending_hashes.remove(index);
                    // The queue rejects results for chunks removed while the
                    // hash was running and for older remove/re-enqueue cycles.
                    if !self
                        .hash_check_queue
                        .complete_token(pending.piece_index, pending.token)
                    {
                        self.discarded_hash_results = self.discarded_hash_results.saturating_add(1);
                        continue;
                    }

                    if result.generation != self.disk_generation
                        || pending.generation != self.disk_generation
                    {
                        self.discarded_hash_results = self.discarded_hash_results.saturating_add(1);
                        continue;
                    }

                    let piece_index = result.piece_index;
                    let expected = self.metadata.piece_hash(piece_index as usize);
                    match expected {
                        Some(expected) if expected == result.hash => {
                            self.mark_piece_complete(piece_index);
                            applied += 1;
                        }
                        Some(_) => {
                            self.chunk_selector.not_using_index(piece_index);
                            let _ = self.delegator.transfers_mut().erase(piece_index);
                            applied += 1;
                        }
                        None => {
                            self.discarded_hash_results =
                                self.discarded_hash_results.saturating_add(1);
                        }
                    }
                }
                Some(Err(err)) => {
                    let pending = self.pending_hashes.remove(index);
                    let live = self
                        .hash_check_queue
                        .fail_token(pending.piece_index, pending.token);
                    if !live {
                        self.discarded_hash_results = self.discarded_hash_results.saturating_add(1);
                        continue;
                    }
                    tracing::error!(
                        piece = pending.piece_index,
                        error = %err,
                        "Disk hash failed; piece will be requested again"
                    );
                    self.chunk_selector.not_using_index(pending.piece_index);
                    let _ = self.delegator.transfers_mut().erase(pending.piece_index);
                }
            }
        }

        // A completion may have freed worker queue capacity.
        self.pump_hash_queue();

        applied
    }

    pub fn save_directory(&self) -> &std::path::Path {
        &self.save_directory
    }

    pub fn set_save_directory<P: AsRef<std::path::Path>>(&mut self, path: P) {
        self.save_directory = path.as_ref().to_path_buf();
    }

    pub fn info_hash(&self) -> &HashString {
        &self.metadata.info_hash
    }

    pub fn name(&self) -> &str {
        &self.metadata.name
    }

    pub fn metadata(&self) -> &TorrentMetadata {
        &self.metadata
    }

    /// Checks if any non-empty target files already exist in `save_directory`.
    pub fn has_existing_files(&self) -> bool {
        if self.save_directory.as_os_str().is_empty() || self.target_files.is_empty() {
            return false;
        }

        for target in self.target_files.iter() {
            if target.length == 0 {
                continue;
            }
            let relative = target
                .path
                .strip_prefix(std::path::Path::new("/"))
                .unwrap_or(&target.path);
            let path = self.save_directory.join(relative);
            if path.exists() {
                if let Ok(meta) = std::fs::metadata(&path) {
                    if meta.len() > 0 {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Queues all pieces for hash verification if non-empty files already exist on disk.
    pub fn queue_recheck_if_files_exist(&mut self) -> bool {
        if self.has_existing_files() {
            self.pending_hash_pieces = (0..self.metadata.num_pieces as u32).collect();
            self.hash_checked = false;
            true
        } else {
            false
        }
    }

    /// Applies persisted resume data (bitfield, priorities, mtimes).
    ///
    /// The stored bitfield is only trusted for files whose size and mtime
    /// still match. Files marked with a sentinel (or whose metadata changed)
    /// have their chunk range cleared, and the affected piece indices are
    /// exposed through [`Download::take_pending_hash_pieces`] so the caller can
    /// run a partial recheck. The download is considered hash-checked only
    /// when nothing is pending.
    pub fn apply_resume(&mut self, resume: &ResumeData) -> Result<usize, String> {
        self.apply_resume_with_uncertain(resume, &[])
    }

    /// Applies resume data and re-queues pieces whose data may not have been
    /// fsynced when the snapshot was taken.
    ///
    /// `uncertain` comes from the `uncertain_pieces` resume extra. Those pieces
    /// are cleared from the persisted bitfield and exposed through
    /// [`Download::take_pending_hash_pieces`], so the first start re-hashes
    /// them instead of trusting page-cache contents that may not have reached
    /// the disk.
    pub fn apply_resume_with_uncertain(
        &mut self,
        resume: &ResumeData,
        uncertain: &[u32],
    ) -> Result<usize, String> {
        let num_pieces = self.metadata.num_pieces;

        if let Some(bitfield) = &resume.bitfield {
            if bitfield.size_bits() != num_pieces {
                return Err(format!(
                    "resume bitfield has {} bits, expected {}",
                    bitfield.size_bits(),
                    num_pieces
                ));
            }
        }

        for (index, priority) in resume.file_priorities.iter().enumerate() {
            if let Some(file) = self.file_list.files_mut().get_mut(index) {
                file.set_priority(*priority);
            }
        }

        self.unsynced_pieces.clear();

        if let Some(base_bitfield) = &resume.bitfield {
            let (verified, pending) =
                self.validate_resume_bitfield(resume, base_bitfield, uncertain);

            // Apply the trusted part of the bitfield now. Pieces belonging to
            // changed files are left unset and exposed through
            // `take_pending_hash_pieces` so the caller can queue a partial
            // recheck without discarding the rest of the progress.
            self.pending_hash_pieces = pending;
            self.hash_checked = self.pending_hash_pieces.is_empty();
            self.hash_failed = false;
            self.hash_failed_pieces.clear();

            self.apply_completed_bitfield(&verified);
        } else {
            self.pending_hash_pieces.clear();
            self.hash_checked = false;
            self.hash_failed = false;
            self.hash_failed_pieces.clear();
        }

        self.resume_mtimes = resume.file_mtimes.clone();
        self.resume_completed_bytes = resume.completed_bytes;

        // A complete torrent becomes a seeder as soon as resume data is
        // applied. This preserves the restored lifecycle state across session
        // commits; an incomplete torrent remains stopped until started.
        if self.is_seeder() && !self.state.is_closed() {
            self.transition_to(DownloadState::Seeding);
        }

        Ok(self.chunk_selector.completed().size_set())
    }

    /// Replaces the completed bitfield and mirrors it into the file list.
    fn apply_completed_bitfield(&mut self, bitfield: &Bitfield) {
        let num_pieces = self.metadata.num_pieces;
        let high = self.chunk_selector.high_priority().clone();
        let normal = self.chunk_selector.normal_priority().clone();
        let mut selector = ChunkSelector::new(num_pieces);
        selector.set_sequential(self.chunk_selector.is_sequential());
        selector.set_priorities(high, normal);

        self.file_list.reset_completion();
        for index in 0..num_pieces {
            if bitfield.get(index) {
                selector.mark_completed(index as u32);
                self.file_list.mark_completed(index as u32);
            }
        }

        self.chunk_selector = selector;
    }

    /// Checks persisted file metadata and decides which pieces to rehash.
    ///
    /// Returns the bitfield with untrusted ranges cleared plus the sorted
    /// list of affected piece indices.
    fn validate_resume_bitfield(
        &self,
        resume: &ResumeData,
        base: &Bitfield,
        uncertain: &[u32],
    ) -> (Bitfield, Vec<u32>) {
        let mut bitfield = base.clone();

        if self.save_directory.as_os_str().is_empty() || self.target_files.is_empty() {
            return (bitfield, Vec::new());
        }

        let mut recheck = Vec::new();

        for (index, target) in self.target_files.iter().enumerate() {
            // Zero-length files are always complete and occupy no chunks.
            if target.length == 0 {
                continue;
            }

            let (first, last) = Self::file_chunk_range(
                target,
                self.metadata.piece_length,
                self.metadata.num_pieces,
            );
            if first >= last {
                continue;
            }

            let mtime = resume
                .file_mtimes
                .get(index)
                .copied()
                .unwrap_or(MTIME_UNCHECKED);

            let relative = target
                .path
                .strip_prefix(std::path::Path::new("/"))
                .unwrap_or(&target.path);
            let path = self.save_directory.join(relative);

            // `~3` means the resume was written while the file was actively
            // downloading. The persisted bitfield is trusted; only chunks in
            // the uncertain-piece list need verification.
            if mtime == MTIME_DOWNLOADING {
                continue;
            }

            let needs_recheck = match std::fs::metadata(&path) {
                Err(_) => true,
                Ok(metadata) => {
                    if metadata.len() != target.length {
                        // A file whose size differs invalidates the resume
                        // entry for every chunk it overlaps.
                        true
                    } else {
                        match mtime {
                            MTIME_RECHECK | MTIME_UNCHECKED | MTIME_SKIPPED => true,
                            value if value >= 0 => {
                                file_mtime_seconds(&metadata) != Some(value as u64)
                            }
                            _ => true,
                        }
                    }
                }
            };

            if needs_recheck {
                bitfield.unset_range(first as usize, last as usize);
                recheck.extend(first..last);
            }
        }

        // Pieces written but not fsynced when the snapshot was taken are never
        // trusted, even when the file mtime looks valid (or the file is marked
        // as actively downloading).
        for index in uncertain {
            let index = *index as usize;
            if index < self.metadata.num_pieces {
                bitfield.unset(index);
                recheck.push(index as u32);
            }
        }

        recheck.sort_unstable();
        recheck.dedup();
        (bitfield, recheck)
    }

    /// Chunk range `[first, last)` covered by a torrent file.
    fn file_chunk_range(
        file: &lt_storage::TargetFileInfo,
        piece_length: u32,
        num_pieces: usize,
    ) -> (u32, u32) {
        if file.length == 0 || piece_length == 0 || num_pieces == 0 {
            return (0, 0);
        }

        let first = file.offset / piece_length as u64;
        let end = file.offset.saturating_add(file.length);
        let last = end.saturating_add(piece_length as u64 - 1) / piece_length as u64;

        (
            first.min(num_pieces as u64) as u32,
            last.min(num_pieces as u64) as u32,
        )
    }

    /// Captures the current state for `.libtorrent_resume`.
    pub fn to_resume_data(&self) -> ResumeData {
        let completed = self.chunk_selector.completed().clone();

        let priorities: Vec<_> = self
            .file_list
            .files()
            .iter()
            .map(|file| file.priority())
            .collect();

        let mtimes: Vec<i64> = self
            .target_files
            .iter()
            .enumerate()
            .map(|(index, target)| {
                if self.save_directory.as_os_str().is_empty() {
                    return MTIME_UNCHECKED;
                }

                let priority = self
                    .file_list
                    .files()
                    .get(index)
                    .map(|file| file.priority())
                    .unwrap_or(lt_storage::file::Priority::Normal);

                let relative = target
                    .path
                    .strip_prefix(std::path::Path::new("/"))
                    .unwrap_or(&target.path);
                let path = self.save_directory.join(relative);

                match std::fs::metadata(&path) {
                    Ok(metadata) => {
                        // A file that is still being downloaded may have
                        // unsynced chunks; force a recheck instead of
                        // trusting its mtime. Completed torrents and stopped
                        // downloads can safely persist the real mtime.
                        if self.state.is_active() && !self.is_seeder() {
                            MTIME_DOWNLOADING
                        } else {
                            file_mtime_seconds(&metadata)
                                .map(|seconds| seconds as i64)
                                .unwrap_or(MTIME_UNCHECKED)
                        }
                    }
                    Err(_) => {
                        if priority == lt_storage::file::Priority::Off {
                            MTIME_SKIPPED
                        } else {
                            MTIME_UNCHECKED
                        }
                    }
                }
            })
            .collect();

        let mut completed_bytes = 0u64;
        for index in 0..self.metadata.num_pieces {
            if completed.get(index) {
                completed_bytes += self.chunk_size(index as u32) as u64;
            }
        }

        ResumeData {
            bitfield: Some(completed),
            file_priorities: priorities,
            file_mtimes: mtimes,
            completed_bytes,
        }
    }

    pub fn resume_mtimes(&self) -> &[i64] {
        &self.resume_mtimes
    }

    pub fn resume_completed_bytes(&self) -> u64 {
        self.resume_completed_bytes
    }

    pub fn client_config(&self) -> &ClientConfig {
        &self.client_config
    }

    pub fn client_config_mut(&mut self) -> &mut ClientConfig {
        &mut self.client_config
    }

    pub fn is_metadata_download(&self) -> bool {
        self.metadata.is_meta_download
    }

    pub fn is_sequential(&self) -> bool {
        self.chunk_selector.is_sequential()
    }

    pub fn set_sequential(&mut self, value: bool) {
        self.chunk_selector.set_sequential(value);
    }

    pub fn metadata_download(&self) -> Option<&MetadataDownload> {
        self.metadata_download.as_ref()
    }

    pub fn metadata_download_mut(&mut self) -> Option<&mut MetadataDownload> {
        self.metadata_download.as_mut()
    }

    /// Queues an address discovered through PEX or another out-of-band source.
    pub fn add_discovered_peer(&mut self, address: SocketAddr) {
        self.discovered_peers.insert(address);
    }

    /// Drains discovered peer addresses for the connection scheduler.
    pub fn take_discovered_peers(&mut self) -> Vec<SocketAddr> {
        self.discovered_peers.drain().collect()
    }

    pub fn discovered_peer_count(&self) -> usize {
        self.discovered_peers.len()
    }

    /// Replaces the magnet placeholder with the real torrent metadata.
    ///
    /// Rebuilds the file/chunk layers, selector and delegator, preserving the
    /// save directory, disk worker and peer connections.
    pub fn apply_metadata(&mut self, metadata: TorrentMetadata) -> Result<(), String> {
        if !self.metadata.is_meta_download {
            return Err("download already has metadata".into());
        }

        let num_pieces = metadata.num_pieces;
        let piece_length = metadata.piece_length;
        let sequential = self.chunk_selector.is_sequential();

        let mut file_list = FileList::new();
        file_list.set_multi_file(metadata.is_multi_file);
        for entry in &metadata.files {
            let mut file = File::new();
            file.set_path(entry.path.as_string());
            file.set_size_bytes(entry.length);
            file.set_offset(entry.offset);
            file_list.push_file(file);
        }
        if !metadata.files.is_empty() {
            file_list
                .initialize(metadata.total_size, piece_length)
                .map_err(|err| err.to_string())?;
        }

        let chunk_manager = Arc::new(ChunkManager::new());
        let mut chunk_list = ChunkList::new();
        chunk_list.set_chunk_size(piece_length);
        chunk_list.set_manager(chunk_manager);
        chunk_list.resize(num_pieces);

        let target_files: Arc<Vec<lt_storage::TargetFileInfo>> = Arc::new(
            metadata
                .files
                .iter()
                .map(|file| lt_storage::TargetFileInfo {
                    path: std::path::PathBuf::from(file.path.as_string().trim_start_matches('/')),
                    length: file.length,
                    offset: file.offset,
                })
                .collect(),
        );

        let mut tracker_list = TrackerList::new();
        for (group_index, group) in metadata.announce_list.iter().enumerate() {
            for url in group {
                tracker_list.insert_url(group_index as u32, url, false);
            }
        }
        if tracker_list.is_empty() {
            if let Some(ref announce) = metadata.announce {
                tracker_list.insert_url(0, announce, false);
            }
        }

        // Persist anything already verified against the old metadata before
        // swapping in the new file/chunk mapping. This also drains buffered
        // (backpressured) writes, which still refer to the old piece layout.
        let _ = self.flush_disk();
        self.clear_hash_check_queue();

        self.metadata = metadata;
        self.file_list = file_list;
        self.chunk_list = chunk_list;
        self.chunk_selector = ChunkSelector::new(num_pieces);
        self.chunk_selector.set_sequential(sequential);
        self.chunk_statistics = ChunkStatistics::new(num_pieces);
        self.delegator = Delegator::new();
        self.target_files = target_files;
        for peer in self.connection_list.iter_mut() {
            peer.bitfield = Bitfield::new(num_pieces);
        }

        self.piece_data_cache.clear();
        self.pending_writes.clear();
        self.queued_writes.clear();
        self.peer_caches.clear();
        self.metadata_download = None;
        self.discovered_peers.clear();
        self.resume_mtimes.clear();
        self.resume_completed_bytes = 0;
        self.hash_checked = false;
        self.hash_failed = false;
        self.hash_failed_pieces.clear();
        self.pending_hash_pieces.clear();

        // A magnet download may have been added for a payload that is already
        // on disk (for example an ISO downloaded earlier). Metadata is the
        // first point at which the file layout is known, so queue those pieces
        // for verification before any peer can overwrite them.
        let _ = self.queue_recheck_if_files_exist();

        if !tracker_list.is_empty() {
            self.tracker_controller = TrackerController::new(tracker_list);
        }

        if !matches!(
            self.state,
            DownloadState::Stopped | DownloadState::Closed | DownloadState::Downloading
        ) {
            self.transition_to(DownloadState::Downloading);
        }

        Ok(())
    }

    /// Parses verified metadata bytes and applies them.
    pub fn apply_metadata_bytes(
        &mut self,
        info_bytes: &[u8],
        info_hash: HashString,
        announce: Option<String>,
        announce_list: Vec<Vec<String>>,
    ) -> Result<(), String> {
        let metadata =
            DownloadConstructor::from_info_bytes(info_bytes, info_hash, announce, announce_list)
                .map_err(|err| err.to_string())?;

        self.apply_metadata(metadata)
    }

    pub fn state(&self) -> &DownloadState {
        &self.state
    }

    /// Applies a legal state transition and updates lifecycle metadata.
    ///
    /// Returns `false` if the transition is illegal or already active. A
    /// successful transition bumps `state_counter` exactly once.
    pub fn transition_to(&mut self, next: DownloadState) -> bool {
        if self.state == next {
            return false;
        }

        if !self.state.can_transition_to(&next) {
            return false;
        }

        let now = unix_now();
        if matches!(&next, DownloadState::Downloading | DownloadState::Seeding)
            && self.timestamp_started == 0
        {
            self.timestamp_started = now;
        }
        if matches!(&next, DownloadState::Seeding) && self.timestamp_finished == 0 {
            self.timestamp_finished = now;
        }

        self.state = next;
        self.state_changed = now;
        self.state_counter = self.state_counter.wrapping_add(1);
        true
    }

    /// Records a fatal error state without closing the download.
    pub fn set_error(&mut self, message: impl Into<String>) -> bool {
        self.transition_to(DownloadState::Error(message.into()))
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn is_closed(&self) -> bool {
        self.state.is_closed()
    }

    pub fn is_checking(&self) -> bool {
        self.state.is_checking()
    }

    /// True while a queued or running verification owns the on-disk data.
    ///
    /// Peer requests and incoming blocks must be discarded until the pass is
    /// over, otherwise a peer could overwrite bytes that are about to be
    /// hashed (or are being hashed).
    fn verification_owns_data(&self) -> bool {
        self.state.is_checking() || !self.pending_hash_pieces.is_empty()
    }

    /// True once a hash pass has completed without being cancelled and no
    /// piece is still waiting for a resume-triggered recheck.
    pub fn is_hash_checked(&self) -> bool {
        self.hash_checked && self.pending_hash_pieces.is_empty() && !self.state.is_checking()
    }

    /// True while a resume validation left pieces awaiting verification.
    pub fn has_pending_hash_pieces(&self) -> bool {
        !self.pending_hash_pieces.is_empty()
    }

    /// True when the last completed pass found at least one bad piece.
    pub fn is_hash_failed(&self) -> bool {
        self.hash_failed
    }

    /// Pieces that failed in the last hash pass.
    pub fn hash_failed_pieces(&self) -> &[u32] {
        &self.hash_failed_pieces
    }

    /// Pieces that resume validation could not trust and still need rehashing.
    pub fn pending_hash_pieces(&self) -> &[u32] {
        &self.pending_hash_pieces
    }

    /// Takes the pieces queued by resume validation for a partial recheck.
    pub fn take_pending_hash_pieces(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.pending_hash_pieces)
    }

    pub fn is_active(&self) -> bool {
        self.state.is_active()
    }

    pub fn is_paused(&self) -> bool {
        self.state.is_paused()
    }

    pub fn state_changed(&self) -> u64 {
        self.state_changed
    }

    pub fn state_counter(&self) -> u64 {
        self.state_counter
    }

    pub fn set_state_changed(&mut self, value: u64) {
        self.state_changed = value;
    }

    pub fn set_state_counter(&mut self, value: u64) {
        self.state_counter = value;
    }

    pub fn timestamp_started(&self) -> u64 {
        self.timestamp_started
    }

    pub fn set_timestamp_started(&mut self, value: u64) {
        self.timestamp_started = value;
    }

    pub fn timestamp_finished(&self) -> u64 {
        self.timestamp_finished
    }

    pub fn set_timestamp_finished(&mut self, value: u64) {
        self.timestamp_finished = value;
    }

    pub fn load_date(&self) -> u64 {
        self.load_date
    }

    pub fn set_load_date(&mut self, value: u64) {
        self.load_date = value;
    }

    pub fn is_seeder(&self) -> bool {
        self.completed_chunks() == self.total_chunks() && self.total_chunks() > 0
    }

    pub fn is_done(&self) -> bool {
        self.is_seeder()
    }

    /// Enables BEP 16 super-seeding for a complete download.
    ///
    /// Returns `false` when the download is not a seeder yet. Re-enabling
    /// resets the distribution state, matching rTorrent's restart behavior.
    pub fn start_initial_seeding(&mut self) -> bool {
        if !self.is_seeder() {
            return false;
        }
        self.initial_seeding = Some(InitialSeeding::new(self.total_chunks()));
        true
    }

    pub fn stop_initial_seeding(&mut self) {
        self.initial_seeding = None;
    }

    /// Whether new seeders may be added as peers.
    pub fn accepting_seeders(&self) -> bool {
        self.accepting_seeders
    }

    pub fn set_accepting_seeders(&mut self, value: bool) {
        self.accepting_seeders = value;
    }

    pub fn is_initial_seeding(&self) -> bool {
        self.initial_seeding.is_some()
    }

    pub fn initial_seed_chunks_left(&self) -> Option<usize> {
        self.initial_seeding
            .as_ref()
            .map(InitialSeeding::chunks_left)
    }

    /// Offers the next piece to an initial-seed peer.
    ///
    /// Pieces the peer already has are marked done instead of being offered.
    /// When the last piece is distributed the mode is switched off and `None`
    /// is returned.
    pub fn initial_seed_offer(
        &mut self,
        peer_id: &HashString,
        index_done: Option<u32>,
    ) -> Option<u32> {
        let peer_bitfield = self
            .connection_list
            .find_by_id(peer_id)
            .map(|peer| peer.bitfield.clone());

        let mut completed = false;
        let offer = {
            let seeding = match self.initial_seeding.as_mut() {
                Some(seeding) => seeding,
                None => return None,
            };

            let mut offer = None;
            loop {
                let index = seeding.chunk_offer(peer_id, index_done);
                if index == InitialSeeding::NO_OFFER {
                    break;
                }

                let peer_has = peer_bitfield
                    .as_ref()
                    .map(|bitfield| bitfield.get(index as usize))
                    .unwrap_or(false);
                if peer_has {
                    seeding.chunk_seen(index, peer_id);
                    if seeding.is_complete() {
                        completed = true;
                        break;
                    }
                    continue;
                }

                offer = Some(index);
                break;
            }
            offer
        };

        if completed {
            self.initial_seeding = None;
        }
        offer
    }

    /// Records that a peer has a piece while initial seeding.
    ///
    /// Returns `true` when every piece has been distributed and the download
    /// has transitioned back to normal seeding.
    pub fn initial_seed_seen(&mut self, index: u32, peer_id: &HashString) -> bool {
        let completed = match self.initial_seeding.as_mut() {
            Some(seeding) => {
                seeding.chunk_seen(index, peer_id);
                seeding.is_complete()
            }
            None => false,
        };

        if completed {
            self.initial_seeding = None;
        }
        completed
    }

    /// Whether an initial-seed peer is allowed to request `index`.
    pub fn initial_seed_allows(&self, peer_id: &HashString, index: u32) -> bool {
        self.initial_seeding
            .as_ref()
            .map(|seeding| seeding.is_offered(peer_id, index))
            .unwrap_or(true)
    }

    /// Whether the download has the piece that it would upload.
    pub fn has_piece(&self, index: u32) -> bool {
        index < self.total_chunks() as u32 && self.chunk_selector.completed().get(index as usize)
    }

    /// Wire-format bitfield of pieces this download has completed.
    pub fn completed_bitfield_bytes(&self) -> Vec<u8> {
        self.chunk_selector.completed().as_bytes().to_vec()
    }

    /// Records an uploaded block for rate and session counters.
    pub fn record_upload(&mut self, bytes: usize) {
        self.uploaded_bytes = self.uploaded_bytes.saturating_add(bytes as u64);
        self.rate_up.insert(bytes as u64);
    }

    /// Reads one block from a completed piece for serving to a peer.
    ///
    /// Pending blocks in `piece_data_cache` are preferred so a just-written
    /// piece can be uploaded before the disk worker flushes it.
    pub fn read_piece_block(&self, index: u32, begin: u32, length: u32) -> Result<Vec<u8>, String> {
        if index >= self.total_chunks() as u32 {
            return Err("piece index out of range".to_string());
        }
        let piece_len = self.chunk_size(index);
        let end = begin
            .checked_add(length)
            .ok_or_else(|| "block range overflow".to_string())?;
        if end > piece_len {
            return Err("block range exceeds piece length".to_string());
        }
        if self.save_directory.as_os_str().is_empty() && self.piece_data_cache.is_empty() {
            return Err("download has no save directory".to_string());
        }

        let mut out = vec![0u8; length as usize];
        if let Some(cached) = self.piece_data_cache.get(&index) {
            if cached.len() >= end as usize {
                out.copy_from_slice(&cached[begin as usize..end as usize]);
                return Ok(out);
            }
        }

        // Reads are serialised with writes on the disk worker. The caller may
        // block waiting for the reply, but no filesystem syscall is issued on
        // the reactor thread.
        let thread = self.disk_thread.clone().or_else(global_disk_thread);
        let Some(thread) = thread else {
            return Err("no disk worker available for read".to_string());
        };

        thread
            .read_block(
                self.save_directory.clone(),
                Arc::clone(&self.target_files),
                self.metadata.piece_length,
                index,
                begin,
                length,
            )
            .map_err(|err| format!("disk read failed: {err}"))
    }

    pub fn completed_chunks(&self) -> usize {
        self.chunk_selector.completed().size_set()
    }

    pub fn total_chunks(&self) -> usize {
        self.metadata.num_pieces
    }

    pub fn uploaded_bytes(&self) -> u64 {
        self.uploaded_bytes
    }

    pub fn downloaded_bytes(&self) -> u64 {
        self.downloaded_bytes
    }

    /// Restores the session transfer counter from persisted state.
    pub fn set_downloaded_bytes(&mut self, value: u64) {
        self.downloaded_bytes = value;
    }

    /// Restores the session transfer counter from persisted state.
    pub fn set_uploaded_bytes(&mut self, value: u64) {
        self.uploaded_bytes = value;
    }

    /// Payload bytes covered by verified pieces.
    ///
    /// The last piece is usually shorter than `piece_length`, so a full
    /// bitfield is clamped against `total_size` instead of counting it in
    /// full. Computed from the set-bit count to stay O(1) per render.
    pub fn completed_bytes(&self) -> u64 {
        let pieces = self.metadata.num_pieces as u64;
        if pieces == 0 || self.metadata.total_size == 0 {
            return 0;
        }

        let completed = self.chunk_selector.completed().size_set() as u64;
        if completed == 0 {
            return 0;
        }

        // `piece_length * completed` over-counts a short final piece, so cap
        // against the payload size instead of special-casing the last piece.
        let piece_length = self.metadata.piece_length as u64;
        completed
            .saturating_mul(piece_length)
            .min(self.metadata.total_size)
    }

    /// Pieces completed from a disk write that has not been fsynced yet.
    ///
    /// These are persisted as resume extras and re-hashed on restore instead of
    /// being trusted, which keeps a crash from resurrecting page-cache-only
    /// data.
    pub fn uncertain_pieces(&self) -> Vec<u32> {
        let completed = self.chunk_selector.completed();
        self.unsynced_pieces
            .iter()
            .copied()
            .filter(|index| completed.get(*index as usize))
            .collect()
    }

    pub fn rate_up(&mut self) -> u64 {
        self.rate_up.rate()
    }

    pub fn rate_down(&mut self) -> u64 {
        self.rate_down.rate()
    }

    pub fn chunk_selector(&self) -> &ChunkSelector {
        &self.chunk_selector
    }

    pub fn chunk_selector_mut(&mut self) -> &mut ChunkSelector {
        &mut self.chunk_selector
    }

    pub fn chunk_statistics(&self) -> &ChunkStatistics {
        &self.chunk_statistics
    }

    pub fn chunk_statistics_mut(&mut self) -> &mut ChunkStatistics {
        &mut self.chunk_statistics
    }

    pub fn delegator(&self) -> &Delegator {
        &self.delegator
    }

    pub fn delegator_mut(&mut self) -> &mut Delegator {
        &mut self.delegator
    }

    pub fn connection_list(&self) -> &ConnectionList {
        &self.connection_list
    }

    pub fn connection_list_mut(&mut self) -> &mut ConnectionList {
        &mut self.connection_list
    }

    pub fn tracker_controller(&self) -> &TrackerController {
        &self.tracker_controller
    }

    pub fn tracker_controller_mut(&mut self) -> &mut TrackerController {
        &mut self.tracker_controller
    }

    pub fn file_list(&self) -> &FileList {
        &self.file_list
    }

    pub fn file_list_mut(&mut self) -> &mut FileList {
        &mut self.file_list
    }

    pub fn chunk_list(&self) -> &ChunkList {
        &self.chunk_list
    }

    pub fn chunk_list_mut(&mut self) -> &mut ChunkList {
        &mut self.chunk_list
    }

    /// Opens the download, allocating no resources until start.
    ///
    /// Opening is idempotent. A closed download transitions back to `Stopped`.
    pub fn open(&mut self) -> Result<(), String> {
        if self.is_open && !self.state.is_closed() {
            return Ok(());
        }

        if self.state.is_closed() || self.state.is_error() {
            self.transition_to(DownloadState::Stopped);
        }
        self.is_open = true;
        Ok(())
    }

    /// Starts the download.
    ///
    /// This is the compatibility entry point used by older engine callers. A
    /// never-opened download is opened automatically; an explicitly closed or
    /// checking download is left untouched. Prefer [`Download::try_start`]
    /// when the caller needs a diagnostic.
    pub fn start(&mut self) {
        if !self.is_open && self.state.is_stopped() {
            let _ = self.open();
        }
        let _ = self.try_start();
    }

    /// Starts the download, refusing illegal states instead of silently
    /// ignoring them.
    pub fn try_start(&mut self) -> Result<(), String> {
        if !self.is_open || self.state.is_closed() {
            return Err("download is closed".into());
        }
        if self.state.is_checking() {
            return Err("download is currently hash checking".into());
        }
        if self.state.is_error() {
            return Err("download is in an error state".into());
        }

        // Resume validation cleared any file range whose metadata changed.
        // Verify those pieces now, before the transfer starts, while keeping
        // the trusted part of the bitfield intact.
        if !self.pending_hash_pieces.is_empty() {
            let was_active = self.state.is_active();
            let pending = std::mem::take(&mut self.pending_hash_pieces);
            self.recheck_pieces(&pending)?;

            // The recheck may have promoted the download straight to
            // Seeding; make sure the tracker start events are still sent once.
            if !was_active && self.state.is_active() {
                self.tracker_controller.enable(0);
                self.tracker_controller.send_start_event();
                return Ok(());
            }
        } else if !self.hash_checked {
            self.hash_checked = true;
        }

        let next = if self.is_seeder() {
            DownloadState::Seeding
        } else {
            DownloadState::Downloading
        };

        if self.state == next {
            // Idempotent start: do not re-send tracker events or bump the
            // state counter.
            return Ok(());
        }

        if !self.transition_to(next) {
            return Err(format!("illegal transition from {:?} to start", self.state));
        }

        self.tracker_controller.enable(0);
        self.tracker_controller.send_start_event();
        Ok(())
    }

    /// Stops the download. Idempotent and never fails.
    pub fn stop(&mut self) {
        let _ = self.try_stop();
    }

    /// Stops the download if it is running.
    pub fn try_stop(&mut self) -> Result<(), String> {
        if self.state.is_closed() {
            if self.hash_check_queue.pending_size() > 0 || !self.pending_hashes.is_empty() {
                self.clear_hash_check_queue();
            }
            return Ok(());
        }

        self.initial_seeding = None;
        let was_active = self.state.is_active();
        let was_paused = self.state.is_paused();
        if was_active {
            self.tracker_controller.send_stop_event();
        }
        self.tracker_controller.disable();

        // A stopped download must not keep verification work or results
        // around; bumping the generation also invalidates worker replies.
        self.clear_hash_check_queue();

        if matches!(
            self.state,
            DownloadState::Downloading
                | DownloadState::Seeding
                | DownloadState::Paused
                | DownloadState::Checking
                | DownloadState::Error(_)
        ) {
            self.transition_to(DownloadState::Stopped);
        }

        if was_active || was_paused {
            self.clear_peers();
        }

        Ok(())
    }

    /// Pauses an active download. Idempotent.
    pub fn pause(&mut self) {
        if !self.state.is_active() {
            return;
        }
        self.initial_seeding = None;

        self.tracker_controller.send_stop_event();
        self.tracker_controller.disable();
        self.clear_peers();
        self.transition_to(DownloadState::Paused);
    }

    /// Resumes a paused download. Idempotent.
    pub fn resume(&mut self) {
        if self.state.is_paused() {
            let _ = self.try_start();
        }
    }

    /// Stops and closes the download, releasing caches and peers.
    pub fn close(&mut self) -> Result<(), String> {
        if self.state.is_closed() && !self.is_open {
            // Stay idempotent, but still discard work submitted after the
            // download was closed (for example by a late peer event).
            if self.hash_check_queue.pending_size() > 0 || !self.pending_hashes.is_empty() {
                self.clear_hash_check_queue();
            }
            return Ok(());
        }

        if self.state.is_active() {
            self.tracker_controller.send_stop_event();
        }
        self.tracker_controller.disable();

        if !self.pending_writes.is_empty() || !self.queued_writes.is_empty() {
            let _ = self.flush_disk();
        }
        self.clear_hash_check_queue();
        self.piece_data_cache.clear();
        self.initial_seeding = None;
        self.clear_peers();

        if !self.state.is_closed() {
            self.transition_to(DownloadState::Closed);
        }
        self.is_open = false;
        Ok(())
    }

    /// Closes the download without announcing a stop to trackers.
    ///
    /// Mirrors rTorrent's `d.close.directly`: the files are closed and the
    /// state is forced to `Closed`, but no `stopped` announce is queued.
    pub fn close_directly(&mut self) -> Result<(), String> {
        self.tracker_controller.disable();

        if !self.pending_writes.is_empty() || !self.queued_writes.is_empty() {
            let _ = self.flush_disk();
        }
        self.clear_hash_check_queue();
        self.piece_data_cache.clear();
        self.clear_peers();

        if !self.state.is_closed() {
            let _ = self.transition_to(DownloadState::Closed);
        }
        self.is_open = false;
        Ok(())
    }

    /// Removes runtime state for a download that is being erased.
    ///
    /// User data on disk is intentionally never touched here; session/resume
    /// cleanup belongs to the caller.
    pub fn erase(&mut self) {
        let _ = self.close();

        self.clear_peers();
        self.piece_data_cache.clear();
        self.initial_seeding = None;
        self.discovered_peers.clear();
        self.pending_writes.clear();
        self.queued_writes.clear();
        self.clear_hash_check_queue();
        self.pending_hash_pieces.clear();
        self.hash_failed_pieces.clear();
        self.unsynced_pieces.clear();
        self.hash_checked = false;
        self.hash_failed = false;
    }

    fn clear_peers(&mut self) {
        let peers: Vec<HashString> = self
            .connection_list
            .iter()
            .map(|peer| peer.peer_id)
            .collect();
        for peer_id in peers {
            self.remove_peer(&peer_id);
        }

        let min_size = self.connection_list.min_size();
        let max_size = self.connection_list.max_size();
        let max_size_explicit = self.connection_list.max_size_explicit();
        self.connection_list = ConnectionList::new();
        let _ = self.connection_list.set_min_size(min_size);
        if max_size_explicit {
            let _ = self.connection_list.set_max_size(max_size);
        } else {
            let _ = self.connection_list.set_max_size_auto(max_size);
        }
        self.peer_caches.clear();
        self.delegator = Delegator::new();
    }

    /// Effective peer limit for this download, including the optional hard
    /// connection cap from [`ClientConfig`].
    pub fn peer_limit(&self) -> usize {
        self.client_config.effective_max_peers()
    }

    /// Alias for [`Download::peer_limit`].
    pub fn max_peer_count(&self) -> usize {
        self.peer_limit()
    }

    /// Configured minimum peer count used by the discovery scheduler.
    pub fn min_peer_limit(&self) -> usize {
        self.client_config.min_peers() as usize
    }

    /// Alias for [`Download::min_peer_limit`].
    pub fn min_peer_count(&self) -> usize {
        self.min_peer_limit()
    }

    pub fn peer_count(&self) -> usize {
        self.connection_list.size()
    }

    /// True while the swarm is smaller than the configured minimum.
    pub fn below_min_peers(&self) -> bool {
        self.peer_count() < self.min_peer_limit()
    }

    /// Alias for [`Download::below_min_peers`].
    pub fn needs_more_peers(&self) -> bool {
        self.below_min_peers()
    }

    /// Number of connected seeders, used by the seeder peer limits.
    pub fn seeder_peer_count(&self) -> usize {
        self.connection_list
            .iter()
            .filter(|peer| peer.is_seeder())
            .count()
    }

    /// Timeout used when dialing a new peer.
    pub fn connect_timeout(&self) -> std::time::Duration {
        self.client_config.connect_timeout()
    }

    /// Timeout allowed for the peer-wire handshake.
    pub fn handshake_timeout(&self) -> std::time::Duration {
        self.client_config.handshake_timeout()
    }

    /// Whether the download can admit another peer right now.
    pub fn can_accept_incoming(&self) -> bool {
        self.client_config
            .allows_peer_count(self.connection_list.size())
    }

    /// Whether the download can start another outbound dial.
    ///
    /// A dial is cheap to refuse; the caller should keep the address available
    /// for a later attempt instead of treating this as a permanent failure.
    pub fn can_dial_outgoing(&self) -> bool {
        self.can_accept_incoming()
    }

    /// Whether `peer` would be accepted by [`Download::try_add_peer`].
    pub fn can_add_peer(&self, peer: &PeerConnection) -> bool {
        if !self
            .client_config
            .allows_peer_count(self.connection_list.size())
        {
            return false;
        }

        if self.connection_list.contains_id(&peer.peer_id) {
            return false;
        }

        if !self.accepting_seeders && peer.is_seeder() {
            return false;
        }

        let max_seed = self.client_config.max_peers_seed();
        if max_seed >= 0 && peer.is_seeder() && self.seeder_peer_count() >= max_seed as usize {
            return false;
        }

        true
    }

    /// Adds a peer if connection limits allow it, returning the reason when
    /// admission is refused.
    pub fn try_add_peer(&mut self, peer: PeerConnection) -> Result<(), PeerAddError> {
        if self.connection_list.size() >= self.client_config.max_peers() as usize {
            return Err(PeerAddError::MaxPeers);
        }
        if self.connection_list.size() >= self.peer_limit() {
            return Err(PeerAddError::ConnectionLimit);
        }

        if self.connection_list.contains_id(&peer.peer_id) {
            return Err(PeerAddError::DuplicatePeer);
        }

        if !self.accepting_seeders && peer.is_seeder() {
            return Err(PeerAddError::SeedersDisabled);
        }

        let max_seed = self.client_config.max_peers_seed();
        if max_seed >= 0 && peer.is_seeder() && self.seeder_peer_count() >= max_seed as usize {
            return Err(PeerAddError::MaxSeedPeers);
        }

        // Keep the list's own guard in sync with the configured limit unless
        // the caller explicitly configured the connection list.
        if !self.connection_list.max_size_explicit()
            && self.connection_list.max_size() != self.peer_limit()
        {
            let _ = self.connection_list.set_max_size_auto(self.peer_limit());
        }

        let peer_id = peer.peer_id;
        let bf = peer.bitfield.clone();
        if !self.connection_list.push(peer) {
            return Err(PeerAddError::MaxPeers);
        }
        self.chunk_statistics.add_peer(peer_id, &bf);
        Ok(())
    }

    /// Adds a peer if connection limits allow it.
    pub fn add_peer(&mut self, peer: PeerConnection) -> bool {
        self.try_add_peer(peer).is_ok()
    }

    /// Builds and admits a peer with an explicit direction.
    pub fn add_peer_with_direction(
        &mut self,
        peer_id: HashString,
        address: lt_net::addr::SocketAddress,
        bitfield_size: usize,
        direction: PeerDirection,
        encrypted: bool,
    ) -> bool {
        let peer = PeerConnection::new_with_direction(
            peer_id,
            address,
            bitfield_size,
            direction,
            encrypted,
        );
        self.add_peer(peer)
    }

    /// Admits an inbound peer. The `Incoming` direction is preserved by
    /// [`Download::add_peer`] and remains visible through `p.is_incoming`.
    pub fn add_incoming_peer(
        &mut self,
        peer_id: HashString,
        address: lt_net::addr::SocketAddress,
        bitfield_size: usize,
        encrypted: bool,
    ) -> bool {
        self.add_peer_with_direction(
            peer_id,
            address,
            bitfield_size,
            PeerDirection::Incoming,
            encrypted,
        )
    }

    /// Admits an outbound peer.
    pub fn add_outgoing_peer(
        &mut self,
        peer_id: HashString,
        address: lt_net::addr::SocketAddress,
        bitfield_size: usize,
        encrypted: bool,
    ) -> bool {
        self.add_peer_with_direction(
            peer_id,
            address,
            bitfield_size,
            PeerDirection::Outgoing,
            encrypted,
        )
    }

    /// Forces a pre-built connection to be treated as inbound.
    pub fn add_peer_incoming(&mut self, mut peer: PeerConnection) -> bool {
        peer.set_direction(PeerDirection::Incoming);
        self.add_peer(peer)
    }

    /// Forces a pre-built connection to be treated as outbound.
    pub fn add_peer_outgoing(&mut self, mut peer: PeerConnection) -> bool {
        peer.set_direction(PeerDirection::Outgoing);
        self.add_peer(peer)
    }

    /// Removes a peer and releases every resource owned by its session.
    ///
    /// Outstanding block requests are cancelled so they cannot remain
    /// attributed to a peer that no longer exists. Returns whether a peer was
    /// actually removed.
    pub fn remove_peer(&mut self, peer_id: &HashString) -> bool {
        if let Some(seeding) = self.initial_seeding.as_mut() {
            seeding.release_peer(peer_id);
        }

        // Return outstanding blocks before dropping the connection entry so
        // the delegator never holds a transfer for a removed peer.
        self.cancel_peer(peer_id);
        self.peer_caches.remove(&Self::peer_number(peer_id));
        self.chunk_statistics.remove_peer(peer_id);

        self.connection_list.erase(Some(peer_id), 0).is_ok()
    }

    /// Removes a peer by socket address, returning its id if it was connected.
    pub fn remove_peer_by_address(
        &mut self,
        address: &lt_net::addr::SocketAddress,
    ) -> Option<HashString> {
        let peer_id = self
            .connection_list
            .find_by_address(address)
            .map(|peer| peer.peer_id)?;
        if self.remove_peer(&peer_id) {
            Some(peer_id)
        } else {
            None
        }
    }

    /// Snapshot of a connected peer for the command layer.
    pub fn peer_info(&self, peer_id: &HashString) -> Option<PeerInfo> {
        let mut info = self.connection_list.find_by_id(peer_id)?.info();
        info.queue_depth = self.peer_queue_depth(peer_id);
        Some(info)
    }

    /// Snapshot of the peer at `address`.
    pub fn peer_info_by_address(&self, address: &lt_net::addr::SocketAddress) -> Option<PeerInfo> {
        let peer_id = self.connection_list.find_by_address(address)?.peer_id;
        self.peer_info(&peer_id)
    }

    /// Owned snapshots of all connected peers.
    pub fn peer_infos(&self) -> Vec<PeerInfo> {
        self.peer_info_iter().collect()
    }

    /// Lazily snapshots every connected peer.
    pub fn peer_info_iter(&self) -> impl Iterator<Item = PeerInfo> + '_ {
        self.connection_list.iter().map(|peer| {
            let mut info = peer.info();
            info.queue_depth = self.peer_queue_depth(&peer.peer_id);
            info
        })
    }

    /// Iterates over the live connection objects.
    pub fn peers(&self) -> impl Iterator<Item = &PeerConnection> + '_ {
        self.connection_list.iter()
    }

    /// Number of outstanding block requests attributed to `peer_id`.
    pub fn peer_queue_depth(&self, peer_id: &HashString) -> usize {
        let number = Self::peer_number(peer_id);
        self.delegator
            .transfers()
            .iter()
            .map(|block_list| {
                block_list
                    .blocks()
                    .iter()
                    .flat_map(|block| block.transfers())
                    .filter(|transfer| transfer.peer_id() == number && !transfer.is_erased())
                    .count()
            })
            .sum()
    }

    /// Number of peers currently unchoked for upload.
    pub fn unchoked_peer_count(&self) -> usize {
        self.connection_list
            .iter()
            .filter(|peer| !peer.is_choked)
            .count()
    }

    /// Whether an upload slot is available for `peer_id`.
    pub fn can_unchoke_peer(&self, peer_id: &HashString) -> bool {
        let Some(peer) = self.connection_list.find_by_id(peer_id) else {
            return false;
        };
        if !peer.is_choked {
            return true;
        }
        self.client_config
            .allows_upload_count(self.unchoked_peer_count())
    }

    /// Applies an upload choke decision while enforcing `max_uploads`.
    ///
    /// Returns `false` if the peer does not exist or unchoking would exceed the
    /// configured limit.
    pub fn set_peer_choked(&mut self, peer_id: &HashString, choked: bool) -> bool {
        if !choked && !self.can_unchoke_peer(peer_id) {
            return false;
        }

        match self.connection_list.find_by_id_mut(peer_id) {
            Some(peer) => {
                peer.is_choked = choked;
                true
            }
            None => false,
        }
    }

    pub fn unchoke_peer(&mut self, peer_id: &HashString) -> bool {
        self.set_peer_choked(peer_id, false)
    }

    pub fn choke_peer(&mut self, peer_id: &HashString) -> bool {
        self.set_peer_choked(peer_id, true)
    }

    /// Attributes uploaded bytes to both the download and the peer session.
    pub fn record_upload_for_peer(&mut self, peer_id: &HashString, bytes: usize) {
        self.record_upload(bytes);
        if let Some(peer) = self.connection_list.find_by_id_mut(peer_id) {
            peer.add_uploaded(bytes as u64);
        }
    }

    /// Attributes downloaded bytes to both the download and the peer session.
    pub fn record_download_for_peer(&mut self, peer_id: &HashString, bytes: usize) {
        self.downloaded_bytes = self.downloaded_bytes.saturating_add(bytes as u64);
        self.rate_down.insert(bytes as u64);
        if let Some(peer) = self.connection_list.find_by_id_mut(peer_id) {
            peer.add_downloaded(bytes as u64);
        }
    }

    /// Marks a peer's handshake as complete, clearing its handshake timeout.
    pub fn mark_peer_handshake_complete(&mut self, peer_id: &HashString) -> bool {
        match self.connection_list.find_by_id_mut(peer_id) {
            Some(peer) => {
                peer.mark_handshake_complete();
                true
            }
            None => false,
        }
    }

    /// Removes peers whose connect or handshake phase exceeded the configured
    /// timeout. `now` is a Unix timestamp in seconds.
    ///
    /// Returns the number of peers that were disconnected.
    pub fn sweep_peer_timeouts(&mut self, now: u64) -> usize {
        let handshake_timeout = self.client_config.handshake_timeout().as_secs().max(1);
        let connect_timeout = self.client_config.connect_timeout().as_secs().max(1);

        let expired: Vec<HashString> = self
            .connection_list
            .iter()
            .filter(|peer| {
                let elapsed = now.saturating_sub(peer.connected_at);
                if peer.is_handshake {
                    elapsed >= handshake_timeout
                } else if !peer.is_connected {
                    elapsed >= connect_timeout
                } else {
                    false
                }
            })
            .map(|peer| peer.peer_id)
            .collect();

        let removed = expired.len();
        for peer_id in expired {
            self.remove_peer(&peer_id);
        }
        removed
    }

    pub fn receive_have(&mut self, peer_id: &HashString, index: u32) {
        if index as usize >= self.total_chunks() {
            return;
        }

        let changed = if let Some(peer) = self.connection_list.find_by_id_mut(peer_id) {
            if peer.bitfield.get(index as usize) {
                false
            } else {
                peer.bitfield.set(index as usize);
                true
            }
        } else {
            false
        };

        if changed {
            self.chunk_statistics
                .received_have_chunk(peer_id, index as usize);

            let peer_num = Self::peer_number(peer_id);
            let cache = self.peer_caches.entry(peer_num).or_default();
            self.chunk_selector
                .received_have_chunk(cache, index, &self.chunk_statistics);
        }
    }

    pub fn receive_bitfield(&mut self, peer_id: &HashString, bitfield: Bitfield) {
        if let Some(peer) = self.connection_list.find_by_id_mut(peer_id) {
            peer.bitfield = bitfield.clone();
            self.chunk_statistics.received_bitfield(*peer_id, &bitfield);
        }
    }

    fn peer_number(peer_id: &HashString) -> usize {
        ((peer_id.as_bytes()[0] as usize) << 8) | peer_id.as_bytes()[1] as usize
    }

    /// Returns all outstanding blocks for a peer to the delegator.
    pub fn cancel_peer(&mut self, peer_id: &HashString) -> usize {
        self.delegator.cancel_peer(Self::peer_number(peer_id))
    }

    /// Cancels one outstanding block request.
    pub fn cancel_block(
        &mut self,
        peer_id: &HashString,
        piece_index: u32,
        block_offset: u32,
    ) -> bool {
        self.delegator
            .cancel_block(Self::peer_number(peer_id), piece_index, block_offset)
    }

    pub fn delegate_for_peer(&mut self, peer_id: &HashString, max_pieces: usize) -> Vec<Piece> {
        // A queued or running verification owns the on-disk data. Handing out
        // new requests now could overwrite bytes that are about to be hashed.
        if self.verification_owns_data() {
            return Vec::new();
        }

        let (peer_bf, is_seeder) = match self.connection_list.find_by_id(peer_id) {
            Some(p) => (p.bitfield.clone(), p.is_seeder()),
            None => return Vec::new(),
        };

        let peer_num = (peer_id.as_bytes()[0] as usize) << 8 | (peer_id.as_bytes()[1] as usize);
        let mut peer_cache = self.peer_caches.remove(&peer_num).unwrap_or_default();

        let piece_len = self.metadata.piece_length;
        let total_size = self.metadata.total_size;
        let num_pieces = self.metadata.num_pieces;

        let chunk_size_fn = move |idx: u32| -> u32 {
            if (idx as usize) + 1 == num_pieces && total_size > 0 {
                let rem = total_size % (piece_len as u64);
                if rem != 0 {
                    return rem as u32;
                }
            }
            piece_len
        };

        let pieces = self.delegator.delegate(
            peer_num,
            &peer_bf,
            is_seeder,
            None,
            max_pieces,
            &mut self.chunk_selector,
            &self.chunk_statistics,
            &mut peer_cache,
            chunk_size_fn,
        );

        self.peer_caches.insert(peer_num, peer_cache);
        pieces
    }

    /// Rebuilds the completed bitfield by hashing files already on disk.
    ///
    /// This blocks for the duration of the pass; callers that must not block a
    /// reactor should run it on a dedicated thread/`spawn_blocking`.
    pub fn recheck(&mut self) -> Result<HashResult, String> {
        self.recheck_internal(None, None)
    }

    /// Full pass using a caller-supplied cancellation flag.
    ///
    /// A pre-set flag cancels before the first piece is read, which is useful
    /// for tests and for supervisors that queue a cancelled job.
    pub fn recheck_with_cancel(&mut self, cancel: Arc<AtomicBool>) -> Result<HashResult, String> {
        self.recheck_internal(None, Some(cancel))
    }

    /// Rechecks only `pieces`, preserving every other completion bit.
    ///
    /// Used by resume validation to rehash just the files whose size or mtime
    /// changed instead of throwing away a good partial bitfield.
    pub fn recheck_pieces(&mut self, pieces: &[u32]) -> Result<HashResult, String> {
        self.recheck_internal(Some(pieces), None)
    }

    /// Partial pass using a caller-supplied cancellation flag.
    pub fn recheck_pieces_with_cancel(
        &mut self,
        pieces: &[u32],
        cancel: Arc<AtomicBool>,
    ) -> Result<HashResult, String> {
        self.recheck_internal(Some(pieces), Some(cancel))
    }

    pub fn prepare_recheck(
        &mut self,
        selected: Option<&[u32]>,
        cancel: Arc<AtomicBool>,
    ) -> Result<PreparedRecheck, String> {
        let num_pieces = self.metadata.num_pieces;

        if self.state.is_closed() {
            return Err("download is closed".into());
        }
        if self.state.is_checking() {
            return Err("download is already hash checking".into());
        }

        // Hashing reads the files, so make sure the download is logically
        // open even when called directly on a freshly constructed engine.
        self.is_open = true;
        self.pending_hash_pieces.clear();
        // Any in-flight asynchronous hash refers to the pre-recheck state.
        self.clear_hash_check_queue();

        let selected_pieces: Vec<u32> = match selected {
            Some(pieces) => pieces.to_vec(),
            None => (0..num_pieces as u32).collect(),
        };

        if self.save_directory.as_os_str().is_empty() || self.target_files.is_empty() {
            self.hash_checked = false;
            self.hash_failed = false;
            self.hash_failed_pieces.clear();
            // Keep the request pending so the gate stays closed until a
            // later pass can actually read the files.
            self.pending_hash_pieces = selected_pieces.clone();
            self.pending_hash_pieces.sort_unstable();
            self.pending_hash_pieces.dedup();
            return Ok(PreparedRecheck::Immediate(HashResult {
                bitfield: Bitfield::new(num_pieces),
                checked: 0,
                failed: 0,
                failed_pieces: Vec::new(),
                unchecked_pieces: selected_pieces,
                cancelled: false,
            }));
        }

        // Never hash a piece while a queued write may still be in flight.
        if let Err(err) = self.flush_disk() {
            // The pass never started, so nothing may be reported as checked.
            self.hash_checked = false;
            self.pending_hash_pieces = selected_pieces;
            self.pending_hash_pieces.sort_unstable();
            self.pending_hash_pieces.dedup();
            return Err(format!("could not flush disk before hash check: {err}"));
        }

        let previous_state = self.state.clone();
        if !self.transition_to(DownloadState::Checking) {
            self.hash_checked = false;
            self.pending_hash_pieces = selected_pieces;
            self.pending_hash_pieces.sort_unstable();
            self.pending_hash_pieces.dedup();
            return Err(format!(
                "illegal transition from {:?} to hash checking",
                self.state
            ));
        }

        // Start from the existing bitfield so a lost/cancelled pass keeps
        // every chunk that was not part of the selected range. Selected pieces
        // that were never examined are cleared by `run_hash_pass`.
        let initial = self.chunk_selector.completed().clone();

        // Feed the same priority queue used by live verification, then hash
        // in priority order.
        let jobs = self.queue_hash_jobs(&selected_pieces);
        let ordered: Vec<u32> = jobs.iter().map(|job| job.index).collect();

        Ok(PreparedRecheck::Pass(RecheckContext {
            metadata: self.metadata.clone(),
            save_directory: self.save_directory.clone(),
            target_files: Arc::clone(&self.target_files),
            disk_thread: self.disk_thread.clone(),
            disk_generation: self.disk_generation,
            chunks_hashed: Arc::clone(&self.chunks_hashed),
            initial,
            jobs,
            ordered,
            previous_state,
            cancel,
        }))
    }

    pub fn finish_recheck(
        &mut self,
        prepared: PreparedRecheck,
        result: Result<HashResult, String>,
    ) -> Result<HashResult, String> {
        let ctx = match prepared {
            PreparedRecheck::Immediate(res) => return Ok(res),
            PreparedRecheck::Pass(ctx) => ctx,
        };

        let result = match result {
            Ok(result) => result,
            Err(err) => {
                for job in &ctx.jobs {
                    self.hash_check_queue.fail_token(job.index, job.token);
                }
                self.hash_checked = false;
                self.hash_failed = true;
                self.pending_hash_pieces = ctx.ordered;
                self.pending_hash_pieces.sort_unstable();
                self.pending_hash_pieces.dedup();
                let _ = self.transition_to(ctx.previous_state);
                return Err(err);
            }
        };

        // Mark the examined prefix complete; pieces skipped by cancellation
        // are cancelled in flight so their (absent) results can never apply.
        let checked = result.checked as usize;
        for (position, job) in ctx.jobs.iter().enumerate() {
            if position < checked {
                self.hash_check_queue.complete_token(job.index, job.token);
            } else {
                self.hash_check_queue.remove(job.index);
            }
        }

        if result.cancelled {
            // Unchecked pieces must not be trusted and are retried on the
            // next start/recheck.
            self.pending_hash_pieces
                .extend(result.unchecked_pieces.iter().copied());
            self.pending_hash_pieces.sort_unstable();
            self.pending_hash_pieces.dedup();
            self.hash_checked = false;
        } else {
            self.pending_hash_pieces.clear();
            self.hash_checked = true;
        }

        self.hash_failed = result.failed > 0;
        self.hash_failed_pieces = result.failed_pieces.clone();
        self.apply_completed_bitfield(&result.bitfield);

        if self.is_seeder() {
            let _ = self.transition_to(DownloadState::Seeding);
        } else {
            let next = match ctx.previous_state {
                DownloadState::Downloading | DownloadState::Paused => ctx.previous_state,
                // A torrent that used to be complete but now has bad pieces
                // must leech again rather than claim to seed.
                DownloadState::Seeding => DownloadState::Downloading,
                _ => DownloadState::Stopped,
            };
            let _ = self.transition_to(next);
        }

        Ok(result)
    }

    fn recheck_internal(
        &mut self,
        selected: Option<&[u32]>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<HashResult, String> {
        let cancel = cancel.unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let prepared = self.prepare_recheck(selected, cancel)?;
        let result = prepared.execute();
        self.finish_recheck(prepared, result)
    }

    pub fn receive_block(
        &mut self,
        piece_index: u32,
        block_offset: u32,
        data: &[u8],
    ) -> Result<bool, String> {
        // Blocks that were already in flight can arrive after metadata queued
        // a recheck or after the pass switched the download to `Checking`.
        // Never write them while the files are being verified; drop the
        // partial transfer so it is requested again once the pass is over.
        if self.verification_owns_data() {
            self.piece_data_cache.remove(&piece_index);
            let _ = self.delegator.transfers_mut().erase(piece_index);
            return Ok(false);
        }

        self.downloaded_bytes = self.downloaded_bytes.saturating_add(data.len() as u64);
        self.rate_down.insert(data.len() as u64);

        let piece_len = self.chunk_size(piece_index) as usize;
        let piece_buf = self
            .piece_data_cache
            .entry(piece_index)
            .or_insert_with(|| vec![0u8; piece_len]);

        let start = block_offset as usize;
        let end = (start + data.len()).min(piece_buf.len());
        if start < piece_buf.len() {
            piece_buf[start..end].copy_from_slice(&data[0..end - start]);
        }

        // Check if piece is completed in delegator's block list
        let is_complete = if let Some(bl) = self.delegator.transfers_mut().find_mut(piece_index) {
            for block in bl.blocks_mut() {
                if block.piece().offset() == block_offset {
                    block.completed(0);
                    bl.inc_finished();
                    break;
                }
            }
            bl.is_all_finished()
        } else {
            // If no block list was tracked, assume piece is complete when end reached piece length
            end >= piece_len
        };

        if is_complete {
            let completed_piece = self.piece_data_cache.remove(&piece_index).unwrap();
            let computed_hash = sha1_hash(&completed_piece);

            if let Some(expected_hash) = self.metadata.piece_hash(piece_index as usize) {
                if computed_hash != expected_hash {
                    self.chunk_selector.not_using_index(piece_index);
                    let _ = self.delegator.transfers_mut().erase(piece_index);
                    return Err(format!("Hash check failed for piece {}", piece_index));
                }
            }

            self.persist_or_complete_piece(piece_index, completed_piece)?;
            return Ok(true);
        }

        Ok(false)
    }

    /// Persists a verified piece and marks it complete.
    ///
    /// When a disk thread is configured the write is queued and completion is
    /// reported later by [`Download::poll_disk_completions`]. If the bounded
    /// queue is full the verified bytes are buffered and retried from the
    /// periodic poll. Pieces that do not need a save directory complete
    /// immediately. With no configured worker the piece is written through
    /// the process-wide worker and awaited by the caller; file I/O is never
    /// performed inline on the reactor thread.
    fn persist_or_complete_piece(&mut self, piece_index: u32, data: Vec<u8>) -> Result<(), String> {
        let needs_disk_write =
            !self.save_directory.as_os_str().is_empty() && !self.target_files.is_empty();

        if !needs_disk_write {
            self.mark_piece_complete(piece_index);
            return Ok(());
        }

        let Some(thread) = self.disk_thread.clone() else {
            // Compatibility path: no queue was installed for this download.
            // The write still runs on a disk worker (never inline on the
            // reactor), but the caller waits for it so completion is visible
            // immediately.
            let fallback = global_disk_thread().ok_or_else(|| {
                "no disk worker available; refusing synchronous file I/O".to_string()
            })?;
            fallback
                .write_blocking(
                    self.save_directory.clone(),
                    Arc::clone(&self.target_files),
                    self.metadata.piece_length,
                    piece_index,
                    data,
                )
                .map_err(|err| format!("disk write failed: {err}"))?;
            // `write_blocking` waits for the write syscall, not for fsync.
            self.unsynced_pieces.insert(piece_index);
            self.mark_piece_complete(piece_index);
            return Ok(());
        };

        match thread.try_submit_write_block(
            self.save_directory.clone(),
            Arc::clone(&self.target_files),
            self.metadata.piece_length,
            piece_index,
            0,
            data,
        ) {
            Ok(handle) => {
                self.pending_writes.push(PendingDiskWrite {
                    piece_index,
                    handle,
                });
            }
            Err((StorageError::DiskQueueFull, data)) => {
                // Backpressure: keep the verified bytes in memory and retry
                // from the periodic poll. The reactor never writes inline.
                tracing::debug!(
                    piece = piece_index,
                    "Disk queue full; buffering piece until queue drains"
                );
                self.queued_writes
                    .push_back(QueuedDiskWrite { piece_index, data });
            }
            Err((err, _data)) => {
                tracing::error!(piece = piece_index, error = %err, "Could not queue disk write");
                return Err(format!("disk write queue failed: {err}"));
            }
        }

        Ok(())
    }

    fn mark_piece_complete(&mut self, piece_index: u32) {
        self.chunk_selector.mark_completed(piece_index);
        self.file_list.mark_completed(piece_index);
        let _ = self.delegator.transfers_mut().erase(piece_index);

        if self.is_seeder() && self.state != DownloadState::Seeding {
            if self.transition_to(DownloadState::Seeding) {
                self.tracker_controller.send_completed_event();
            }
        }
    }

    pub fn chunk_size(&self, piece_index: u32) -> u32 {
        let num_pieces = self.metadata.num_pieces;
        let piece_len = self.metadata.piece_length;
        let total_size = self.metadata.total_size;

        if (piece_index as usize) + 1 == num_pieces && total_size > 0 {
            let rem = total_size % (piece_len as u64);
            if rem != 0 {
                return rem as u32;
            }
        }
        piece_len
    }

    pub fn tick(&mut self, now_secs: u64) {
        self.tracker_controller.set_cached_time(now_secs);
        self.rate_up.rate();
        self.rate_down.rate();
        // Disconnect half-open peers that exceeded the configured connect or
        // handshake timeout.
        self.sweep_peer_timeouts(now_secs);
    }
}

#[derive(Clone)]
pub struct RecheckContext {
    pub metadata: TorrentMetadata,
    pub save_directory: std::path::PathBuf,
    pub target_files: Arc<Vec<lt_storage::TargetFileInfo>>,
    pub disk_thread: Option<Arc<DiskThread>>,
    pub disk_generation: u64,
    pub chunks_hashed: Arc<AtomicU32>,
    pub initial: Bitfield,
    pub jobs: Vec<HashCheckJob>,
    pub ordered: Vec<u32>,
    pub previous_state: DownloadState,
    pub cancel: Arc<AtomicBool>,
}

impl RecheckContext {
    pub fn execute(&self) -> Result<HashResult, String> {
        let hashes: Vec<[u8; 20]> = self
            .metadata
            .pieces
            .chunks_exact(20)
            .map(|chunk| {
                let mut hash = [0u8; 20];
                hash.copy_from_slice(chunk);
                hash
            })
            .collect();

        if let Some(thread) = self.disk_thread.clone() {
            let mut seen = HashSet::new();
            let indices: Vec<u32> = self
                .ordered
                .iter()
                .copied()
                .filter(|index| (*index as usize) < self.metadata.num_pieces)
                .filter(|index| seen.insert(*index))
                .collect();

            let mut bitfield = self.initial.clone();
            let mut checked: u32 = 0;
            let mut failed: u32 = 0;
            let mut failed_pieces: Vec<u32> = Vec::new();
            let mut unchecked_pieces: Vec<u32> = Vec::new();
            let mut cancelled = false;
            self.chunks_hashed.store(0, Ordering::Relaxed);

            for (position, &piece_index) in indices.iter().enumerate() {
                if self.cancel.load(Ordering::SeqCst) {
                    cancelled = true;
                    unchecked_pieces = indices[position..].to_vec();
                    for &unchecked in &unchecked_pieces {
                        bitfield.unset(unchecked as usize);
                    }
                    break;
                }

                let piece_size = {
                    let index = piece_index as usize;
                    if index >= self.metadata.num_pieces {
                        0
                    } else if index == self.metadata.num_pieces.saturating_sub(1) {
                        let remainder =
                            (self.metadata.total_size % (self.metadata.piece_length as u64)) as u32;
                        if remainder == 0 {
                            self.metadata.piece_length
                        } else {
                            remainder
                        }
                    } else {
                        self.metadata.piece_length
                    }
                };

                match thread.hash_blocking(
                    self.save_directory.clone(),
                    Arc::clone(&self.target_files),
                    self.metadata.piece_length,
                    piece_index,
                    piece_size,
                    self.disk_generation,
                ) {
                    Ok(result) if result.generation == self.disk_generation => {
                        checked += 1;
                        self.chunks_hashed.store(checked, Ordering::Relaxed);
                        let matches = hashes
                            .get(piece_index as usize)
                            .map(|expected| *expected == result.hash)
                            .unwrap_or(false);

                        if matches {
                            bitfield.set(piece_index as usize);
                        } else {
                            bitfield.unset(piece_index as usize);
                            failed += 1;
                            failed_pieces.push(piece_index);
                        }
                    }
                    Ok(_) | Err(_) => {
                        checked += 1;
                        self.chunks_hashed.store(checked, Ordering::Relaxed);
                        bitfield.unset(piece_index as usize);
                        failed += 1;
                        failed_pieces.push(piece_index);
                    }
                }
            }

            return Ok(HashResult {
                bitfield,
                checked,
                failed,
                failed_pieces,
                unchecked_pieces,
                cancelled,
            });
        }

        let mut hasher = HashTorrent::new(
            self.save_directory.clone(),
            Arc::clone(&self.target_files),
            self.metadata.piece_length,
            self.metadata.num_pieces,
            self.metadata.total_size,
            hashes,
        );
        hasher.set_disk_thread(self.disk_thread.clone());
        hasher.set_cancel_handle(Arc::clone(&self.cancel));

        let chunks_hashed = Arc::clone(&self.chunks_hashed);
        chunks_hashed.store(0, Ordering::Relaxed);
        hasher
            .hash_selected_with_progress(&self.initial, &self.ordered, move |chk, _tot| {
                chunks_hashed.store(chk, Ordering::Relaxed);
            })
            .map_err(|err| err.to_string())
    }
}

#[derive(Clone)]
pub enum PreparedRecheck {
    Immediate(HashResult),
    Pass(RecheckContext),
}

impl PreparedRecheck {
    pub fn execute(&self) -> Result<HashResult, String> {
        match self {
            Self::Immediate(res) => Ok(res.clone()),
            Self::Pass(ctx) => ctx.execute(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multi_piece_metadata(piece_length: u32, pieces_count: usize) -> TorrentMetadata {
        let mut pieces = Vec::new();
        for index in 0..pieces_count {
            let payload = vec![index as u8; piece_length as usize];
            pieces.extend_from_slice(&sha1_hash(&payload));
        }
        TorrentMetadata {
            info_hash: HashString::new([0x5Au8; 20]),
            name: "initial-seed".into(),
            piece_length,
            pieces,
            num_pieces: pieces_count,
            total_size: piece_length as u64 * pieces_count as u64,
            is_multi_file: false,
            files: Vec::new(),
            announce: None,
            announce_list: Vec::new(),
            is_private: false,
            is_meta_download: false,
        }
    }

    fn complete_download(piece_length: u32, pieces_count: usize) -> Download {
        let mut download = Download::new(multi_piece_metadata(piece_length, pieces_count));
        download.set_disk_thread(None);
        download.open().unwrap();
        download.try_start().unwrap();
        for index in 0..pieces_count {
            let payload = vec![index as u8; piece_length as usize];
            assert!(download.receive_block(index as u32, 0, &payload).unwrap());
        }
        assert!(download.is_seeder());
        download
    }

    #[test]
    fn initial_seeding_offers_each_piece_once_across_peers() {
        let mut download = complete_download(4, 4);
        assert!(download.start_initial_seeding());
        assert!(download.is_initial_seeding());

        let peer1 = HashString::new([0x11u8; 20]);
        let peer2 = HashString::new([0x22u8; 20]);

        let mut offers = Vec::new();
        offers.push(download.initial_seed_offer(&peer1, None).unwrap());
        // A peer only receives one outstanding offer at a time.
        assert_eq!(download.initial_seed_offer(&peer1, None), None);
        offers.push(download.initial_seed_offer(&peer2, None).unwrap());
        assert_eq!(offers, vec![0, 1]);

        // peer1 has piece 0 -> next offer for peer1 is piece 2.
        assert!(!download.initial_seed_seen(0, &peer1));
        offers.push(download.initial_seed_offer(&peer1, Some(0)).unwrap());
        assert_eq!(offers, vec![0, 1, 2]);

        // peer2 has piece 1 -> offer piece 3.
        assert!(!download.initial_seed_seen(1, &peer2));
        offers.push(download.initial_seed_offer(&peer2, Some(1)).unwrap());
        assert_eq!(offers, vec![0, 1, 2, 3]);

        // The last outstanding piece completes initial seeding.
        assert!(!download.initial_seed_seen(3, &peer2));
        assert!(download.initial_seed_seen(2, &peer1));
        assert!(!download.is_initial_seeding());
        assert_eq!(download.initial_seed_chunks_left(), None);

        let mut unique = offers.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 4, "each piece must be offered exactly once");
    }

    #[test]
    fn disconnecting_initial_seed_peer_releases_its_piece() {
        let mut download = complete_download(4, 3);
        download.start_initial_seeding();

        let peer = HashString::new([0x33u8; 20]);
        assert_eq!(download.initial_seed_offer(&peer, None), Some(0));
        download.remove_peer(&peer);
        assert_eq!(download.initial_seed_offer(&peer, None), Some(0));
    }

    #[test]
    fn read_piece_block_serves_disk_data_across_offsets() {
        let piece_length = 4u32;
        let payload = b"AAAABBBB";
        let hashes = [sha1_hash(&payload[0..4]), sha1_hash(&payload[4..8])];
        let mut pieces = Vec::new();
        for hash in hashes {
            pieces.extend_from_slice(&hash);
        }

        let mut file_path = lt_common::path::Path::new();
        file_path.push("blob.bin");
        let metadata = TorrentMetadata {
            info_hash: HashString::new([0x44u8; 20]),
            name: "read-piece".into(),
            piece_length,
            pieces,
            num_pieces: 2,
            total_size: payload.len() as u64,
            is_multi_file: false,
            files: vec![crate::constructor::TorrentFileEntry {
                path: file_path,
                length: payload.len() as u64,
                offset: 0,
            }],
            announce: None,
            announce_list: Vec::new(),
            is_private: false,
            is_meta_download: false,
        };

        let dir = std::env::temp_dir().join(format!(
            "rtorrent-initial-seed-read-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let thread = Arc::new(DiskThread::spawn(4).unwrap());
        let mut download = Download::new(metadata);
        download.set_disk_thread(Some(Arc::clone(&thread)));
        download.set_save_directory(&dir);
        download.open().unwrap();
        download.try_start().unwrap();
        assert!(download.receive_block(0, 0, &payload[0..4]).unwrap());
        assert!(download.receive_block(1, 0, &payload[4..8]).unwrap());
        download.flush_disk().unwrap();

        assert_eq!(download.read_piece_block(1, 0, 4).unwrap(), b"BBBB");
        assert_eq!(download.read_piece_block(0, 1, 2).unwrap(), b"AA");
        assert!(download.read_piece_block(1, 2, 4).is_err());

        thread.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn accepting_seeders_can_reject_seeder_peers() {
        let mut download = Download::new(multi_piece_metadata(4, 2));
        download.set_accepting_seeders(false);
        assert!(!download.accepting_seeders());

        let peer_id = HashString::new([0x66u8; 20]);
        let mut seeder = PeerConnection::new(peer_id, lt_net::addr::SocketAddress::default(), 2);
        seeder.bitfield.set(0);
        seeder.bitfield.set(1);
        assert!(!download.add_peer(seeder));

        download.set_accepting_seeders(true);
        let mut seeder = PeerConnection::new(peer_id, lt_net::addr::SocketAddress::default(), 2);
        seeder.bitfield.set(0);
        seeder.bitfield.set(1);
        assert!(download.add_peer(seeder));
    }

    #[test]
    fn test_download_lifecycle() {
        let mut pieces = Vec::new();
        let payload1 = b"Hello, piece 0!";
        let h1 = sha1_hash(payload1);
        pieces.extend_from_slice(&h1);

        let meta = TorrentMetadata {
            info_hash: HashString::new(h1),
            name: "test_download".into(),
            piece_length: payload1.len() as u32,
            pieces,
            num_pieces: 1,
            total_size: payload1.len() as u64,
            is_multi_file: false,
            files: Vec::new(),
            announce: Some("http://tracker/announce".into()),
            announce_list: vec![vec!["http://tracker/announce".into()]],
            is_private: false,
            is_meta_download: false,
        };

        let mut download = Download::new(meta);
        assert_eq!(download.state(), &DownloadState::Stopped);
        assert!(!download.is_seeder());

        download.start();
        assert_eq!(download.state(), &DownloadState::Downloading);

        // Receive block
        let completed = download.receive_block(0, 0, payload1).unwrap();
        assert!(completed);
        assert!(download.is_seeder());
        assert_eq!(download.state(), &DownloadState::Seeding);
    }

    fn lifecycle_metadata() -> TorrentMetadata {
        let payload = b"lifecycle";
        let hash = sha1_hash(payload);
        TorrentMetadata {
            info_hash: HashString::new(hash),
            name: "lifecycle".into(),
            piece_length: payload.len() as u32,
            pieces: hash.to_vec(),
            num_pieces: 1,
            total_size: payload.len() as u64,
            is_multi_file: false,
            files: Vec::new(),
            announce: None,
            announce_list: Vec::new(),
            is_private: false,
            is_meta_download: false,
        }
    }

    #[test]
    fn closed_download_refuses_start() {
        let mut download = Download::new(lifecycle_metadata());
        download.close().unwrap();

        assert!(download.is_closed());
        assert!(!download.is_open());
        assert!(download.try_start().is_err());
        assert!(download.is_closed());

        download.open().unwrap();
        assert!(download.is_open());
        assert_eq!(download.state(), &DownloadState::Stopped);
        download.try_start().unwrap();
        assert_eq!(download.state(), &DownloadState::Downloading);
    }

    #[test]
    fn lifecycle_operations_are_idempotent() {
        let mut download = Download::new(lifecycle_metadata());
        download.open().unwrap();

        download.try_start().unwrap();
        assert!(download.is_active());
        let started_at = download.timestamp_started();
        assert!(started_at > 0);

        let counter = download.state_counter();
        download.try_start().unwrap();
        assert_eq!(counter, download.state_counter());
        assert_eq!(started_at, download.timestamp_started());

        download.try_stop().unwrap();
        assert_eq!(download.state(), &DownloadState::Stopped);
        let stopped_counter = download.state_counter();
        download.try_stop().unwrap();
        assert_eq!(stopped_counter, download.state_counter());

        download.try_start().unwrap();
        download.close().unwrap();
        assert!(download.is_closed());
        let closed_counter = download.state_counter();
        download.close().unwrap();
        assert_eq!(closed_counter, download.state_counter());

        download.open().unwrap();
        let opened_counter = download.state_counter();
        download.open().unwrap();
        assert_eq!(opened_counter, download.state_counter());
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        let mut download = Download::new(lifecycle_metadata());
        download.close().unwrap();

        assert!(!download.transition_to(DownloadState::Downloading));
        assert!(download.is_closed());
        assert_eq!(download.state_counter(), 1);
    }

    #[test]
    fn pause_and_resume_preserve_lifecycle_metadata() {
        let mut download = Download::new(lifecycle_metadata());
        download.open().unwrap();
        download.try_start().unwrap();
        let counter = download.state_counter();

        download.pause();
        assert_eq!(download.state(), &DownloadState::Paused);
        assert_eq!(counter + 1, download.state_counter());

        download.pause();
        assert_eq!(counter + 1, download.state_counter());

        download.resume();
        assert_eq!(download.state(), &DownloadState::Downloading);
        assert_eq!(counter + 2, download.state_counter());
    }
}
