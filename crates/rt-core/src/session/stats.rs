//! Cumulative counters for the session writer.
//!
//! Every session write is a small file plus a rename plus (optionally) two
//! `fsync` calls, so "how many files did we rewrite" is the metric that explains
//! write amplification at rest. The counters are monotonic, cheap (relaxed
//! atomics) and never reset on their own; callers compare two snapshots.

use std::sync::atomic::{AtomicU64, Ordering};

/// Point-in-time view of the session writer's cumulative work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionWriteStats {
    /// Entries passed to [`crate::session::SessionManager::save_streams`].
    pub entries: u64,
    /// Entries that also rewrote the static `.torrent` stream.
    pub torrent_streams: u64,
    /// `libtorrent_resume` streams written (one per entry).
    pub resume_streams: u64,
    /// `.rtorrent` sidecar streams written (one per entry).
    pub rtorrent_streams: u64,
    /// Temporary `.new` files created before rename.
    pub staged_files: u64,
    /// Payload bytes handed to the writer (bencode size, not disk blocks).
    pub bytes: u64,
    /// `fdatasync`/`fsync` calls on staged files.
    pub fdatasync_calls: u64,
    /// Directory `fsync` calls that persist renames.
    pub dir_sync_calls: u64,
    /// Calls to [`crate::session::SessionManager::commit`].
    pub commit_calls: u64,
}

static ENTRIES: AtomicU64 = AtomicU64::new(0);
static TORRENT_STREAMS: AtomicU64 = AtomicU64::new(0);
static RESUME_STREAMS: AtomicU64 = AtomicU64::new(0);
static RTORRENT_STREAMS: AtomicU64 = AtomicU64::new(0);
static STAGED_FILES: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static FDATASYNC_CALLS: AtomicU64 = AtomicU64::new(0);
static DIR_SYNC_CALLS: AtomicU64 = AtomicU64::new(0);
static COMMIT_CALLS: AtomicU64 = AtomicU64::new(0);

/// Records one `save_streams` attempt.
pub(crate) fn record_save_streams(torrent_stream: bool, bytes: u64) {
    ENTRIES.fetch_add(1, Ordering::Relaxed);
    if torrent_stream {
        TORRENT_STREAMS.fetch_add(1, Ordering::Relaxed);
    }
    RESUME_STREAMS.fetch_add(1, Ordering::Relaxed);
    RTORRENT_STREAMS.fetch_add(1, Ordering::Relaxed);
    BYTES.fetch_add(bytes, Ordering::Relaxed);
}

/// Records the `.new` files staged for one entry.
pub(crate) fn record_staged_files(count: u64) {
    STAGED_FILES.fetch_add(count, Ordering::Relaxed);
}

/// Records one staged-file `fsync`/`fdatasync`.
pub(crate) fn record_fdatasync() {
    FDATASYNC_CALLS.fetch_add(1, Ordering::Relaxed);
}

/// Records one directory `fsync`.
pub(crate) fn record_dir_sync() {
    DIR_SYNC_CALLS.fetch_add(1, Ordering::Relaxed);
}

/// Records one `commit` call.
pub(crate) fn record_commit() {
    COMMIT_CALLS.fetch_add(1, Ordering::Relaxed);
}

/// Reads all counters.
pub fn snapshot() -> SessionWriteStats {
    SessionWriteStats {
        entries: ENTRIES.load(Ordering::Relaxed),
        torrent_streams: TORRENT_STREAMS.load(Ordering::Relaxed),
        resume_streams: RESUME_STREAMS.load(Ordering::Relaxed),
        rtorrent_streams: RTORRENT_STREAMS.load(Ordering::Relaxed),
        staged_files: STAGED_FILES.load(Ordering::Relaxed),
        bytes: BYTES.load(Ordering::Relaxed),
        fdatasync_calls: FDATASYNC_CALLS.load(Ordering::Relaxed),
        dir_sync_calls: DIR_SYNC_CALLS.load(Ordering::Relaxed),
        commit_calls: COMMIT_CALLS.load(Ordering::Relaxed),
    }
}

impl SessionWriteStats {
    /// Difference from an earlier snapshot, saturating at zero so a concurrent
    /// reset can never underflow in a benchmark.
    pub fn since(&self, earlier: Self) -> Self {
        Self {
            entries: self.entries.saturating_sub(earlier.entries),
            torrent_streams: self.torrent_streams.saturating_sub(earlier.torrent_streams),
            resume_streams: self.resume_streams.saturating_sub(earlier.resume_streams),
            rtorrent_streams: self
                .rtorrent_streams
                .saturating_sub(earlier.rtorrent_streams),
            staged_files: self.staged_files.saturating_sub(earlier.staged_files),
            bytes: self.bytes.saturating_sub(earlier.bytes),
            fdatasync_calls: self.fdatasync_calls.saturating_sub(earlier.fdatasync_calls),
            dir_sync_calls: self.dir_sync_calls.saturating_sub(earlier.dir_sync_calls),
            commit_calls: self.commit_calls.saturating_sub(earlier.commit_calls),
        }
    }

    /// Flattens the counters into `("name", value)` pairs for metrics output.
    pub fn as_fields(&self) -> [(&'static str, u64); 9] {
        [
            ("session_entries", self.entries),
            ("session_torrent_streams", self.torrent_streams),
            ("session_resume_streams", self.resume_streams),
            ("session_rtorrent_streams", self.rtorrent_streams),
            ("session_staged_files", self.staged_files),
            ("session_bytes", self.bytes),
            ("session_fdatasync_calls", self.fdatasync_calls),
            ("session_dir_sync_calls", self.dir_sync_calls),
            ("session_commit_calls", self.commit_calls),
        ]
    }
}
