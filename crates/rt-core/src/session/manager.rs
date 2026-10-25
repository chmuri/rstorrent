use super::stats;
use super::storer::{file_holds, is_correct_format, DirectoryEntry, DownloadStorer};
use crate::commands::system::resolve_path;
use lt_sys::lockfile::Lockfile;
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// One download's already-serialized session streams.
///
/// Keeping serialization outside `SessionManager` means the manager owns only
/// persistence concerns, while the caller (normally `rt_core::Manager::commit`)
/// owns the download/session-data mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCommitEntry {
    pub info_hash_hex: String,
    pub torrent_stream: Option<Vec<u8>>,
    pub rtorrent_stream: Vec<u8>,
    pub libtorrent_stream: Vec<u8>,
}

impl SessionCommitEntry {
    pub fn new(
        info_hash_hex: impl Into<String>,
        torrent_stream: Option<Vec<u8>>,
        rtorrent_stream: Vec<u8>,
        libtorrent_stream: Vec<u8>,
    ) -> Self {
        Self {
            info_hash_hex: info_hash_hex.into(),
            torrent_stream,
            rtorrent_stream,
            libtorrent_stream,
        }
    }
}

pub struct SessionManager {
    path: String,
    use_fsyncdisk: bool,
    use_lock: bool,
    on_completion: bool,
    active: bool,
    freeze_info: bool,
    lockfile: Option<Lockfile>,
    /// Last successfully published bytes per info hash, shared with every
    /// writer snapshot so an unchanged entry is never written twice.
    write_cache: Arc<Mutex<WriteCache>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            path: String::new(),
            use_fsyncdisk: true,
            use_lock: true,
            on_completion: true,
            active: false,
            freeze_info: false,
            lockfile: None,
            write_cache: Arc::new(Mutex::new(WriteCache::default())),
        }
    }

    pub fn is_used(&self) -> bool {
        !self.path.is_empty()
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn set_path(&mut self, path: &str) -> io::Result<()> {
        if self.freeze_info {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Session path cannot be changed after startup.",
            ));
        }

        if path.is_empty() {
            self.path.clear();
            return Ok(());
        }

        let expanded =
            crate::commands::system::expand_tilde(path).unwrap_or_else(|_| path.to_string());
        let _ = fs::create_dir_all(&expanded);

        let resolved = resolve_path(&expanded);
        if resolved.is_empty() {
            return Err(Error::new(
                ErrorKind::NotFound,
                format!(
                    "Could not resolve session path '{}'. The directory must exist and be accessible.",
                    path
                ),
            ));
        }

        if resolved.ends_with('/') {
            self.path = resolved;
        } else {
            self.path = format!("{}/", resolved);
        }

        Ok(())
    }

    fn session_lock_path(&self) -> String {
        format!("{}rtorrent.lock", self.path)
    }

    /// Returns the path of the session lock file, if a path is configured.
    pub fn lock_path(&self) -> Option<String> {
        if self.path.is_empty() {
            None
        } else {
            Some(self.session_lock_path())
        }
    }

    pub fn is_locked(&self) -> bool {
        self.lockfile
            .as_ref()
            .map(Lockfile::is_locked)
            .unwrap_or(false)
    }

    pub fn use_fsyncdisk(&self) -> bool {
        self.use_fsyncdisk
    }

    pub fn set_use_fsyncdisk(&mut self, use_fsyncdisk: bool) -> io::Result<()> {
        if self.freeze_info {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Session fsyncdisk option cannot be changed after startup.",
            ));
        }
        self.use_fsyncdisk = use_fsyncdisk;
        Ok(())
    }

    /// Whether finishing a download triggers an immediate session save.
    pub fn on_completion(&self) -> bool {
        self.on_completion
    }

    pub fn set_on_completion(&mut self, on_completion: bool) {
        self.on_completion = on_completion;
    }

    pub fn use_lock(&self) -> bool {
        self.use_lock
    }

    pub fn set_use_lock(&mut self, use_lock: bool) -> io::Result<()> {
        if self.freeze_info {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                "Session lock option cannot be changed after startup.",
            ));
        }
        self.use_lock = use_lock;
        Ok(())
    }

    pub fn start(&mut self) -> io::Result<()> {
        if self.active || self.freeze_info {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                "SessionManager::start() called while already started.",
            ));
        }

        if self.path.is_empty() {
            self.active = true;
            self.freeze_info = true;
            return Ok(());
        }

        let path = Path::new(&self.path);
        match fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                let err = Error::new(
                    ErrorKind::NotADirectory,
                    format!("Session path is not a directory: {}", self.path),
                );
                return Err(err);
            }
            Err(err) => {
                return Err(Error::new(
                    err.kind(),
                    format!("Could not access session directory {}: {}", self.path, err),
                ));
            }
        }

        if self.use_lock {
            let lock_path = self.session_lock_path();
            let mut lock = Lockfile::with_path(lock_path.clone());
            if !lock.try_lock() {
                let locked_by = lock.locked_by_as_string();
                let lock_exists = Path::new(&lock_path).exists();
                return Err(if lock_exists {
                    Error::new(
                        ErrorKind::AddrInUse,
                        format!(
                            "Could not lock session directory, held by: {} : {}",
                            locked_by, self.path
                        ),
                    )
                } else {
                    Error::new(
                        ErrorKind::PermissionDenied,
                        format!(
                            "Could not lock session directory (create '{}' failed) : {}",
                            lock_path, self.path
                        ),
                    )
                });
            }

            self.lockfile = Some(lock);
        }

        self.active = true;
        self.freeze_info = true;
        Ok(())
    }

    pub fn stop(&mut self) -> io::Result<()> {
        if !self.active {
            return Err(Error::other(
                "SessionManager::stop() called while not active.",
            ));
        }

        if let Some(mut lock) = self.lockfile.take() {
            if !lock.unlock() {
                tracing::warn!(
                    path = %lock.path(),
                    "Could not remove session lock file while stopping."
                );
            }
        }

        self.active = false;
        Ok(())
    }

    pub fn save_streams(
        &self,
        info_hash_hex: &str,
        torrent_stream: Option<&[u8]>,
        rtorrent_stream: &[u8],
        libtorrent_stream: &[u8],
    ) -> io::Result<()> {
        if self.path.is_empty() {
            return Ok(());
        }

        if !self.active {
            return Err(Error::other(
                "SessionManager::save_streams() called while not active.",
            ));
        }

        self.writer().save_streams(
            info_hash_hex,
            torrent_stream,
            rtorrent_stream,
            libtorrent_stream,
        )
    }

    /// Returns a writer that owns a copy of the session settings.
    ///
    /// Session writes are the slow part of shutdown (one `fsync` per staged
    /// file). Holding the manager mutex across them makes `stop()` wait for the
    /// whole write, so callers that need to bound shutdown take a writer
    /// snapshot under the lock and write outside it.
    pub fn writer(&self) -> SessionWriter {
        SessionWriter {
            path: self.path.clone(),
            use_fsyncdisk: self.use_fsyncdisk,
            cache: Arc::clone(&self.write_cache),
        }
    }

    /// Commits every entry, continuing past individual failures so one broken
    /// download cannot prevent the remaining downloads from being persisted.
    ///
    /// Each download is published atomically (staged temp files are fsynced
    /// before the rename). The first error is returned after all entries have
    /// been attempted.
    pub fn commit(&self, entries: &[SessionCommitEntry]) -> io::Result<usize> {
        if self.path.is_empty() {
            return Ok(0);
        }

        if !self.active {
            return Err(Error::other(
                "SessionManager::commit() called while not active.",
            ));
        }

        self.writer().commit_with_abort(entries, None)
    }

    /// Persist only the resume and `.rtorrent` streams for one download.
    ///
    /// This mirrors the legacy `d.save_resume` action: the static torrent file
    /// is left untouched.
    pub fn save_resume(&self, entry: &SessionCommitEntry) -> io::Result<()> {
        self.save_streams(
            &entry.info_hash_hex,
            None,
            &entry.rtorrent_stream,
            &entry.libtorrent_stream,
        )
    }

    /// Persist the torrent, resume and `.rtorrent` streams for one download.
    ///
    /// This mirrors the legacy `d.save_full_session` action.
    pub fn save_full_session(&self, entry: &SessionCommitEntry) -> io::Result<()> {
        self.save_streams(
            &entry.info_hash_hex,
            entry.torrent_stream.as_deref(),
            &entry.rtorrent_stream,
            &entry.libtorrent_stream,
        )
    }

    /// Returns the valid session entries in deterministic restore order.
    ///
    /// Symlinks, malformed names and vanished files are skipped. A single bad
    /// entry must not prevent the rest of the session from being restored.
    pub fn get_valid_entries(&self) -> io::Result<Vec<DirectoryEntry>> {
        let mut entries = self.get_entries()?;
        entries.retain(|entry| {
            entry.is_file
                && is_correct_format(&entry.s_name)
                && Path::new(&format!("{}{}", self.path, entry.s_name)).is_file()
        });
        entries.sort_by(|a, b| a.s_name.cmp(&b.s_name));
        Ok(entries)
    }

    /// Returns all restorable base names, including magnet placeholders whose
    /// static `.torrent` file has not been written yet.
    pub fn get_restorable_entries(&self) -> io::Result<Vec<super::storer::SessionEntry>> {
        if self.path.is_empty() {
            return Ok(Vec::new());
        }
        super::storer::get_restorable_entries(&self.path)
    }

    pub fn remove_session_files(&self, info_hash_hex: &str) -> io::Result<()> {
        if self.path.is_empty() {
            return Ok(());
        }

        if !self.active {
            return Err(Error::other(
                "SessionManager::remove_session_files() called while not active.",
            ));
        }

        let file_path = DownloadStorer::build_path(&self.path, info_hash_hex)?;
        DownloadStorer::unlink_files(&file_path);
        Ok(())
    }

    /// Removes the session files belonging to one download.
    ///
    /// Called when a download is erased so stale session entries cannot be
    /// restored on the next startup. User data files are never touched.
    pub fn erase_entry(&self, info_hash_hex: &str) -> io::Result<bool> {
        if self.path.is_empty() || !self.active {
            return Ok(false);
        }

        let base = DownloadStorer::build_path(&self.path, info_hash_hex)?;
        DownloadStorer::unlink_files(&base);
        for suffix in [
            ".new",
            ".libtorrent_resume.new",
            ".rtorrent.new",
            ".meta.new",
        ] {
            let _ = fs::remove_file(format!("{base}{suffix}"));
        }
        self.write_cache.lock().unwrap().forget(info_hash_hex);
        Ok(true)
    }

    pub fn get_entries(&self) -> io::Result<Vec<DirectoryEntry>> {
        if self.path.is_empty() {
            return Ok(Vec::new());
        }
        let mut entries = DownloadStorer::get_formatted_entries(&self.path)?;
        // Restore in a stable order rather than relying on readdir order.
        entries.sort_by(|a, b| a.s_name.cmp(&b.s_name));
        Ok(entries)
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        // `Lockfile` also removes the file on drop, but doing it explicitly here
        // makes the release ordering obvious and guarantees the lock is gone
        // before the manager is considered dead.
        if let Some(mut lock) = self.lockfile.take() {
            if !lock.unlock() {
                tracing::warn!(
                    path = %lock.path(),
                    "Could not remove session lock file while dropping manager."
                );
            }
        }
        self.active = false;
    }
}

/// Number of threads used to publish session entries.
///
/// Entries are independent (distinct paths, atomic rename) and an `fsync` is
/// latency-bound rather than throughput-bound, so a handful of workers turns a
/// minute-long write into a few seconds. Capped so the write does not starve
/// the disk pool on the same filesystem.
pub fn write_workers() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get().clamp(1, 8))
        .unwrap_or(4)
}

/// Remembers the bytes last published for each info hash.
///
/// A session commit re-serializes every download, so the writer needs a way to
/// tell "same bytes as last time" from "changed". Entries are compared
/// byte-for-byte (no hash collisions to reason about); one average entry is a
/// few hundred bytes, so a 14k-torrent session keeps a few megabytes here.
#[derive(Debug, Default)]
struct WriteCache {
    entries: std::collections::HashMap<String, CachedStreams>,
}

#[derive(Debug, Clone)]
struct CachedStreams {
    torrent: Option<Vec<u8>>,
    rtorrent: Vec<u8>,
    libtorrent: Vec<u8>,
}

impl WriteCache {
    /// `Some(true)` when the entry is known to be unchanged, `Some(false)` when
    /// it is known to have changed, `None` when this process has not seen it.
    ///
    /// `torrent_stream: None` means a resume-only write: the static torrent is
    /// not being published, so it does not take part in the comparison.
    fn matches(
        &self,
        info_hash_hex: &str,
        torrent_stream: Option<&[u8]>,
        rtorrent_stream: &[u8],
        libtorrent_stream: &[u8],
    ) -> Option<bool> {
        let cached = self.entries.get(info_hash_hex)?;
        if cached.rtorrent != rtorrent_stream || cached.libtorrent != libtorrent_stream {
            return Some(false);
        }
        match (torrent_stream, cached.torrent.as_deref()) {
            (Some(expected), Some(cached_torrent)) => Some(expected == cached_torrent),
            (Some(_), None) => Some(false),
            (None, _) => Some(true),
        }
    }

    fn remember(
        &mut self,
        info_hash_hex: &str,
        torrent_stream: Option<&[u8]>,
        rtorrent_stream: &[u8],
        libtorrent_stream: &[u8],
    ) {
        let torrent = match torrent_stream {
            Some(stream) => Some(stream.to_vec()),
            // A resume-only write leaves the static torrent on disk untouched.
            None => self
                .entries
                .get(info_hash_hex)
                .and_then(|cached| cached.torrent.clone()),
        };
        self.entries.insert(
            info_hash_hex.to_string(),
            CachedStreams {
                torrent,
                rtorrent: rtorrent_stream.to_vec(),
                libtorrent: libtorrent_stream.to_vec(),
            },
        );
    }

    fn forget(&mut self, info_hash_hex: &str) {
        self.entries.remove(info_hash_hex);
    }
}

/// Snapshot of the session directory settings, usable without the manager
/// mutex.
///
/// Obtained from [`SessionManager::writer`]. Publishing one entry is atomic and
/// touches only that entry's paths, so distinct entries can be written
/// concurrently.
#[derive(Debug, Clone)]
pub struct SessionWriter {
    path: String,
    use_fsyncdisk: bool,
    cache: Arc<Mutex<WriteCache>>,
}

impl SessionWriter {
    pub fn is_enabled(&self) -> bool {
        !self.path.is_empty()
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn use_fsyncdisk(&self) -> bool {
        self.use_fsyncdisk
    }

    /// Stages and publishes one entry's streams.
    ///
    /// An entry whose bytes already match the last published generation is
    /// skipped: shutdown must not rewrite a session that did not change. The
    /// first sighting after a restart compares against the files on disk, which
    /// are warm in the page cache right after restore.
    pub fn save_streams(
        &self,
        info_hash_hex: &str,
        torrent_stream: Option<&[u8]>,
        rtorrent_stream: &[u8],
        libtorrent_stream: &[u8],
    ) -> io::Result<()> {
        if self.path.is_empty() {
            return Ok(());
        }

        let file_path = DownloadStorer::build_path(&self.path, info_hash_hex)?;
        if self.already_published(
            info_hash_hex,
            torrent_stream,
            rtorrent_stream,
            libtorrent_stream,
            &file_path,
        ) {
            stats::record_skipped_entry();
            return Ok(());
        }

        let bytes = torrent_stream.map_or(0, |stream| stream.len() as u64)
            + rtorrent_stream.len() as u64
            + libtorrent_stream.len() as u64;
        stats::record_save_streams(torrent_stream.is_some(), bytes);
        let result = DownloadStorer::save_and_move_streams(
            &file_path,
            self.use_fsyncdisk,
            torrent_stream,
            rtorrent_stream,
            libtorrent_stream,
        );
        let mut cache = self.cache.lock().unwrap();
        match &result {
            Ok(()) => cache.remember(
                info_hash_hex,
                torrent_stream,
                rtorrent_stream,
                libtorrent_stream,
            ),
            // A failed publish must not leave "this entry is current" behind.
            Err(_) => cache.forget(info_hash_hex),
        }
        result
    }

    /// Returns `true` when the entry does not need to be written.
    fn already_published(
        &self,
        info_hash_hex: &str,
        torrent_stream: Option<&[u8]>,
        rtorrent_stream: &[u8],
        libtorrent_stream: &[u8],
        file_path: &str,
    ) -> bool {
        if let Some(unchanged) = self.cache.lock().unwrap().matches(
            info_hash_hex,
            torrent_stream,
            rtorrent_stream,
            libtorrent_stream,
        ) {
            return unchanged;
        }

        // First sighting in this process: the files on disk are the last
        // committed generation.
        if !file_holds(&format!("{file_path}.rtorrent"), rtorrent_stream) {
            return false;
        }
        if !file_holds(&format!("{file_path}.libtorrent_resume"), libtorrent_stream) {
            return false;
        }
        if let Some(stream) = torrent_stream {
            if !file_holds(file_path, stream) {
                return false;
            }
        }

        self.cache.lock().unwrap().remember(
            info_hash_hex,
            torrent_stream,
            rtorrent_stream,
            libtorrent_stream,
        );
        true
    }

    pub fn save_resume(&self, entry: &SessionCommitEntry) -> io::Result<()> {
        self.save_streams(
            &entry.info_hash_hex,
            None,
            &entry.rtorrent_stream,
            &entry.libtorrent_stream,
        )
    }

    /// Writes the static `.torrent` stream only when the file is missing.
    ///
    /// A verification in flight makes the resume bitfield untrustworthy, but a
    /// newly imported torrent still has to be recorded or it disappears on the
    /// next start.
    pub fn save_torrent_if_missing(&self, entry: &SessionCommitEntry) -> io::Result<()> {
        let Some(stream) = entry.torrent_stream.as_deref() else {
            return Ok(());
        };
        if self.path.is_empty() {
            return Ok(());
        }
        let path = DownloadStorer::build_path(&self.path, &entry.info_hash_hex)?;
        if Path::new(&path).is_file() {
            return Ok(());
        }
        stats::record_torrent_only(stream.len() as u64);
        DownloadStorer::save_single_stream(&path, self.use_fsyncdisk, stream)
    }

    /// Writes entries sequentially, stopping at the first abort request.
    pub fn commit_with_abort(
        &self,
        entries: &[SessionCommitEntry],
        abort: Option<&AtomicBool>,
    ) -> io::Result<usize> {
        if self.path.is_empty() {
            return Ok(0);
        }

        stats::record_commit();
        let mut saved = 0usize;
        let mut first_error = None;

        for entry in entries {
            if is_aborted(abort) {
                break;
            }
            match self.save_entry(entry) {
                Ok(()) => saved += 1,
                Err(err) => {
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
            }
        }

        match first_error {
            Some(err) => Err(err),
            None => Ok(saved),
        }
    }

    /// Writes entries across `workers` threads.
    ///
    /// Each entry still gets its own `fdatasync` and directory `fsync`; only
    /// the number of entries in flight changes. This is what keeps shutdown
    /// proportional to `entries / workers` instead of `entries`.
    pub fn commit_parallel(
        &self,
        entries: &[SessionCommitEntry],
        abort: Option<&AtomicBool>,
        workers: usize,
    ) -> io::Result<usize> {
        if self.path.is_empty() {
            return Ok(0);
        }

        let workers = workers.clamp(1, entries.len().max(1));
        stats::record_commit();

        let cursor = AtomicUsize::new(0);
        let saved = AtomicUsize::new(0);
        let first_error: std::sync::Mutex<Option<io::Error>> = std::sync::Mutex::new(None);

        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    if is_aborted(abort) {
                        break;
                    }
                    let index = cursor.fetch_add(1, Ordering::Relaxed);
                    let Some(entry) = entries.get(index) else {
                        break;
                    };
                    match self.save_entry(entry) {
                        Ok(()) => {
                            saved.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(err) => {
                            let mut slot = first_error.lock().unwrap();
                            if slot.is_none() {
                                *slot = Some(err);
                            }
                        }
                    }
                });
            }
        });

        if let Some(err) = first_error.into_inner().unwrap() {
            // One broken entry must not hide the entries that did succeed.
            tracing::warn!(
                saved = saved.load(Ordering::Relaxed),
                entries = entries.len(),
                error = %err,
                "Some session entries could not be written."
            );
            Err(err)
        } else {
            Ok(saved.load(Ordering::Relaxed))
        }
    }

    fn save_entry(&self, entry: &SessionCommitEntry) -> io::Result<()> {
        let result = self.save_streams(
            &entry.info_hash_hex,
            entry.torrent_stream.as_deref(),
            &entry.rtorrent_stream,
            &entry.libtorrent_stream,
        );
        if let Err(err) = &result {
            tracing::error!(
                hash = %entry.info_hash_hex,
                path = %self.path,
                error = %err,
                "Could not commit download session data."
            );
        }
        result
    }
}

fn is_aborted(abort: Option<&AtomicBool>) -> bool {
    abort.is_some_and(|flag| flag.load(Ordering::Relaxed))
}
