//! Cached `stat(2)` results, mirroring libtorrent's `FileStatusCache`.
//!
//! Entries are invalidated explicitly after writes and renames, and `prune`
//! both ages out old entries and re-validates their size/mtime so a cache can
//! never keep reporting a stale file forever.
//!
//! # Thread safety
//!
//! `FileStatusCache` is a plain data structure with **no internal locking**.
//! Mutating operations (`stat`, `insert`, `invalidate*`, `prune*`, `clear`,
//! and the setters) take `&mut self`; read-only operations take `&self`.
//! Shared use therefore requires an external `Mutex`/`RwLock`; the RPC command
//! layer in `rt-core` keeps the process-wide instance behind a `Mutex`.
//! Avoid holding that lock while performing blocking I/O so command handlers
//! remain responsive.
//!
//! [`FileStatusCacheStats`] is a cheap snapshot. The crate root does not
//! re-export it; import it from `lt_storage::file_status_cache::FileStatusCacheStats`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStatus {
    pub size: u64,
    pub mtime: i64,
    pub atime: i64,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
struct Entry {
    status: FileStatus,
    inserted: Instant,
}

/// Aggregate counters exposed to `system.file_status_cache.*` commands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileStatusCacheStats {
    /// Number of cached entries.
    pub size: usize,
    /// Successful lookups served from the cache.
    pub hits: u64,
    /// Lookups that had to stat the filesystem.
    pub misses: u64,
    /// New entries inserted by `stat`/`insert`.
    pub inserts: u64,
    /// Existing entries refreshed because size/mtime changed.
    pub updates: u64,
    /// Entries removed by explicit invalidation.
    pub invalidations: u64,
    /// Number of prune operations run (forced or due).
    pub prune_runs: u64,
    /// Entries removed by pruning.
    pub pruned_entries: u64,
}

#[derive(Debug)]
pub struct FileStatusCache {
    entries: HashMap<PathBuf, Entry>,
    prune_interval: Duration,
    last_prune: Instant,
    hits: u64,
    misses: u64,
    inserts: u64,
    updates: u64,
    invalidations: u64,
    prune_runs: u64,
    pruned_entries: u64,
}

impl Default for FileStatusCache {
    fn default() -> Self {
        Self::new()
    }
}

fn unix_seconds(time: io::Result<SystemTime>) -> i64 {
    time.ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn status_from_metadata(metadata: &fs::Metadata) -> FileStatus {
    FileStatus {
        size: metadata.len(),
        mtime: unix_seconds(metadata.modified()),
        atime: unix_seconds(metadata.accessed()),
        is_dir: metadata.is_dir(),
    }
}

fn same_status(left: &FileStatus, right: &FileStatus) -> bool {
    left.size == right.size && left.mtime == right.mtime && left.is_dir == right.is_dir
}

fn current_status(path: &Path) -> Option<FileStatus> {
    fs::metadata(path)
        .ok()
        .map(|metadata| status_from_metadata(&metadata))
}

impl FileStatusCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            prune_interval: Duration::from_secs(300),
            last_prune: Instant::now(),
            hits: 0,
            misses: 0,
            inserts: 0,
            updates: 0,
            invalidations: 0,
            prune_runs: 0,
            pruned_entries: 0,
        }
    }

    pub fn size(&self) -> usize {
        self.entries.len()
    }

    pub fn len(&self) -> usize {
        self.size()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn hits(&self) -> u64 {
        self.hits
    }

    pub fn misses(&self) -> u64 {
        self.misses
    }

    /// New entries inserted by `stat`/`insert`.
    pub fn inserts(&self) -> u64 {
        self.inserts
    }

    /// Existing entries refreshed because size/mtime changed.
    pub fn updates(&self) -> u64 {
        self.updates
    }

    /// Entries removed by explicit invalidation.
    pub fn invalidations(&self) -> u64 {
        self.invalidations
    }

    /// Number of prune operations run (forced or due).
    pub fn prune_runs(&self) -> u64 {
        self.prune_runs
    }

    /// Entries removed by pruning.
    pub fn pruned_entries(&self) -> u64 {
        self.pruned_entries
    }

    pub fn prune_interval(&self) -> Duration {
        self.prune_interval
    }

    pub fn set_prune_interval(&mut self, interval: Duration) {
        self.prune_interval = interval.max(Duration::from_millis(1));
    }

    pub fn stats(&self) -> FileStatusCacheStats {
        FileStatusCacheStats {
            size: self.entries.len(),
            hits: self.hits,
            misses: self.misses,
            inserts: self.inserts,
            updates: self.updates,
            invalidations: self.invalidations,
            prune_runs: self.prune_runs,
            pruned_entries: self.pruned_entries,
        }
    }

    /// Returns cached metadata, refreshing it on a miss or after the prune
    /// interval has elapsed for that entry.
    pub fn stat(&mut self, path: impl AsRef<Path>) -> io::Result<FileStatus> {
        let path = path.as_ref();

        if let Some(entry) = self.entries.get(path) {
            if entry.inserted.elapsed() < self.prune_interval {
                self.hits += 1;
                return Ok(entry.status.clone());
            }
        }

        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(err) => {
                // The lookup could not be served from the cache even though
                // the underlying filesystem call failed.
                self.misses += 1;
                return Err(err);
            }
        };
        let status = status_from_metadata(&metadata);
        let existed = self.entries.contains_key(path);

        self.entries.insert(
            path.to_path_buf(),
            Entry {
                status: status.clone(),
                inserted: Instant::now(),
            },
        );
        self.misses += 1;
        if existed {
            self.updates += 1;
        } else {
            self.inserts += 1;
        }
        Ok(status)
    }

    /// Refreshes an entry and reports whether it was new or changed.
    ///
    /// This mirrors `FileStatusCache::insert` from the C++ daemon: `false`
    /// means the cached size/mtime still match the filesystem.
    pub fn insert(&mut self, path: impl AsRef<Path>) -> io::Result<bool> {
        let path = path.as_ref();
        let metadata = fs::metadata(path)?;
        let status = status_from_metadata(&metadata);

        if let Some(entry) = self.entries.get_mut(path) {
            if same_status(&entry.status, &status) {
                return Ok(false);
            }

            entry.status = status;
            entry.inserted = Instant::now();
            self.updates += 1;
            return Ok(true);
        }

        self.entries.insert(
            path.to_path_buf(),
            Entry {
                status,
                inserted: Instant::now(),
            },
        );
        self.inserts += 1;
        Ok(true)
    }

    pub fn get(&self, path: impl AsRef<Path>) -> Option<&FileStatus> {
        self.entries.get(path.as_ref()).map(|entry| &entry.status)
    }

    /// Iterates over cached path/status pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&Path, &FileStatus)> {
        self.entries
            .iter()
            .map(|(path, entry)| (path.as_path(), &entry.status))
    }

    pub fn invalidate(&mut self, path: impl AsRef<Path>) -> bool {
        let removed = self.entries.remove(path.as_ref()).is_some();
        if removed {
            self.invalidations += 1;
        }
        removed
    }

    /// Invalidate a path after data has been written to it.
    pub fn invalidate_after_write(&mut self, path: impl AsRef<Path>) -> bool {
        self.invalidate(path)
    }

    /// Convenience aliases for callers that name the operation after the event.
    pub fn invalidate_on_write(&mut self, path: impl AsRef<Path>) -> bool {
        self.invalidate(path)
    }

    pub fn invalidate_write(&mut self, path: impl AsRef<Path>) -> bool {
        self.invalidate(path)
    }

    /// Invalidate both sides of a rename. Returns the number of removed entries.
    pub fn invalidate_after_rename(
        &mut self,
        from: impl AsRef<Path>,
        to: impl AsRef<Path>,
    ) -> usize {
        let mut removed = 0;
        if self.invalidate(from) {
            removed += 1;
        }
        if self.invalidate(to) {
            removed += 1;
        }
        removed
    }

    /// Convenience aliases for callers that name the operation after the event.
    pub fn invalidate_on_rename(&mut self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> usize {
        self.invalidate_after_rename(from, to)
    }

    pub fn invalidate_rename(&mut self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> usize {
        self.invalidate_after_rename(from, to)
    }

    /// Removes entries older than the prune interval, and entries whose
    /// size/mtime no longer match the filesystem. Returns the count removed.
    pub fn prune(&mut self) -> usize {
        self.prune_runs += 1;
        let now = Instant::now();
        let interval = self.prune_interval;
        let before = self.entries.len();

        self.entries.retain(|path, entry| {
            if now.duration_since(entry.inserted) >= interval {
                return false;
            }

            match current_status(path) {
                Some(status) => same_status(&entry.status, &status),
                None => false,
            }
        });

        let removed = before - self.entries.len();
        self.pruned_entries += removed as u64;
        self.last_prune = now;
        removed
    }

    /// Prunes only when at least `prune_interval` has elapsed since the last
    /// prune. Returns zero when the cache is not due yet.
    pub fn prune_if_due(&mut self) -> usize {
        if self.last_prune.elapsed() < self.prune_interval {
            return 0;
        }
        self.prune()
    }

    /// Re-validates every entry against the filesystem without applying the
    /// age limit. Returns the count removed.
    pub fn prune_stale(&mut self) -> usize {
        self.prune_runs += 1;
        let before = self.entries.len();

        self.entries
            .retain(|path, entry| match current_status(path) {
                Some(status) => same_status(&entry.status, &status),
                None => false,
            });

        let removed = before - self.entries.len();
        self.pruned_entries += removed as u64;
        removed
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rtorrent_file_status_{}_{}.tmp",
            tag,
            std::process::id()
        ))
    }

    #[test]
    fn caches_until_invalidated() {
        let path = temp_path("cache");
        let _ = fs::remove_file(&path);

        {
            let mut file = fs::File::create(&path).unwrap();
            file.write_all(b"1234").unwrap();
        }

        let mut cache = FileStatusCache::new();
        let first = cache.stat(&path).unwrap();
        assert_eq!(first.size, 4);
        assert_eq!(cache.misses(), 1);

        let second = cache.stat(&path).unwrap();
        assert_eq!(second.size, 4);
        assert_eq!(cache.hits(), 1);

        {
            let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
            file.write_all(b"56").unwrap();
        }
        assert_eq!(cache.stat(&path).unwrap().size, 4);

        assert!(cache.invalidate(&path));
        assert_eq!(cache.stat(&path).unwrap().size, 6);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn insert_reports_new_and_changed_entries() {
        let path = temp_path("insert");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"abcd").unwrap();

        let mut cache = FileStatusCache::new();
        assert!(cache.insert(&path).unwrap());
        assert!(!cache.insert(&path).unwrap());
        assert_eq!(cache.get(&path).unwrap().size, 4);

        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"ef")
            .unwrap();

        assert!(cache.insert(&path).unwrap());
        assert_eq!(cache.get(&path).unwrap().size, 6);

        let stats = cache.stats();
        assert_eq!(stats.inserts, 1);
        assert_eq!(stats.updates, 1);
        assert_eq!(stats.size, 1);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn write_and_rename_invalidate_entries() {
        let old_path = temp_path("rename_old");
        let new_path = temp_path("rename_new");
        let write_path = temp_path("write");
        let _ = fs::remove_file(&old_path);
        let _ = fs::remove_file(&new_path);
        let _ = fs::remove_file(&write_path);

        fs::write(&old_path, b"old").unwrap();
        fs::write(&write_path, b"write").unwrap();

        let mut cache = FileStatusCache::new();
        cache.stat(&old_path).unwrap();
        cache.stat(&write_path).unwrap();

        assert!(cache.invalidate_after_write(&write_path));
        assert!(cache.get(&write_path).is_none());

        fs::rename(&old_path, &new_path).unwrap();
        assert_eq!(cache.invalidate_after_rename(&old_path, &new_path), 1);
        assert!(cache.get(&old_path).is_none());

        // The renamed path can now be cached under its new name.
        assert_eq!(cache.stat(&new_path).unwrap().size, 3);
        assert_eq!(cache.stats().invalidations, 2);

        let _ = fs::remove_file(&new_path);
        let _ = fs::remove_file(&write_path);
    }

    #[test]
    fn stat_refreshes_after_prune_interval() {
        let path = temp_path("ttl");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"a").unwrap();

        let mut cache = FileStatusCache::new();
        cache.set_prune_interval(Duration::from_millis(1));
        assert_eq!(cache.stat(&path).unwrap().size, 1);
        std::thread::sleep(Duration::from_millis(5));

        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"bb")
            .unwrap();

        assert_eq!(cache.stat(&path).unwrap().size, 3);
        assert_eq!(cache.stats().misses, 2);
        assert_eq!(cache.stats().updates, 1);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn prune_removes_old_entries() {
        let path = temp_path("prune");
        let _ = fs::write(&path, b"x");

        let mut cache = FileStatusCache::new();
        cache.set_prune_interval(Duration::from_millis(1));
        cache.stat(&path).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(cache.prune(), 1);
        assert_eq!(cache.size(), 0);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn prune_removes_changed_and_missing_entries() {
        let changed_path = temp_path("prune_changed");
        let missing_path = temp_path("prune_missing");
        let _ = fs::remove_file(&changed_path);
        let _ = fs::remove_file(&missing_path);

        fs::write(&changed_path, b"a").unwrap();
        fs::write(&missing_path, b"b").unwrap();

        let mut cache = FileStatusCache::new();
        // Keep a long age limit so only status validation can evict.
        cache.set_prune_interval(Duration::from_secs(60));
        cache.stat(&changed_path).unwrap();
        cache.stat(&missing_path).unwrap();

        fs::OpenOptions::new()
            .append(true)
            .open(&changed_path)
            .unwrap()
            .write_all(b"more")
            .unwrap();
        fs::remove_file(&missing_path).unwrap();

        assert_eq!(cache.prune(), 2);
        assert_eq!(cache.size(), 0);

        let _ = fs::remove_file(&changed_path);
    }

    #[test]
    fn prune_if_due_respects_interval_and_counts_stats() {
        let path = temp_path("due");
        let _ = fs::write(&path, b"x");

        let mut cache = FileStatusCache::new();
        cache.set_prune_interval(Duration::from_secs(60));
        cache.stat(&path).unwrap();

        // Immediately after inserting the age limit has not elapsed.
        assert_eq!(cache.prune_if_due(), 0);

        // Age both the entry and the last-prune marker past the interval.
        let old = Instant::now()
            .checked_sub(Duration::from_secs(61))
            .unwrap_or_else(Instant::now);
        cache.last_prune = old;
        cache.entries.get_mut(&path).unwrap().inserted = old;

        assert_eq!(cache.prune_if_due(), 1);

        let stats = cache.stats();
        assert_eq!(stats.size, 0);
        assert_eq!(stats.inserts, 1);
        assert_eq!(stats.prune_runs, 1);
        assert_eq!(stats.pruned_entries, 1);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn stats_accessors_track_invalidation_and_stale_prune() {
        let kept = temp_path("stats_kept");
        let missing = temp_path("stats_missing");
        let _ = fs::remove_file(&kept);
        let _ = fs::remove_file(&missing);
        fs::write(&kept, b"keep").unwrap();
        fs::write(&missing, b"gone").unwrap();

        let mut cache = FileStatusCache::new();
        cache.set_prune_interval(Duration::from_secs(60));
        cache.stat(&kept).unwrap();
        cache.stat(&missing).unwrap();
        assert_eq!(cache.len(), 2);
        assert!(!cache.is_empty());
        assert_eq!(cache.inserts(), 2);
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.hits(), 0);

        // A failed filesystem lookup is still a cache miss.
        assert!(cache.stat(temp_path("stats_absent")).is_err());
        assert_eq!(cache.misses(), 3);
        assert_eq!(cache.len(), 2);

        // A cache hit and a no-op invalidation must not change counters.
        cache.stat(&kept).unwrap();
        assert_eq!(cache.hits(), 1);
        assert!(!cache.invalidate(temp_path("not_cached")));
        assert_eq!(cache.invalidations(), 0);

        fs::remove_file(&missing).unwrap();
        assert_eq!(cache.prune_stale(), 1);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.prune_runs(), 1);
        assert_eq!(cache.pruned_entries(), 1);

        // Explicit invalidation is counted and removes the last entry.
        assert!(cache.invalidate_after_write(&kept));
        assert_eq!(cache.invalidations(), 1);
        assert!(cache.is_empty());

        let stats = cache.stats();
        assert_eq!(stats.invalidations, 1);
        assert_eq!(stats.inserts, 2);
        assert_eq!(stats.size, 0);

        let _ = fs::remove_file(&kept);
    }

    #[test]
    fn clear_removes_entries_but_keeps_cumulative_stats() {
        let path = temp_path("clear");
        let _ = fs::write(&path, b"x");

        let mut cache = FileStatusCache::new();
        cache.stat(&path).unwrap();
        cache.clear();

        assert!(cache.is_empty());
        assert_eq!(cache.size(), 0);
        assert_eq!(cache.inserts(), 1);
        assert_eq!(cache.misses(), 1);

        let _ = fs::remove_file(&path);
    }
}
