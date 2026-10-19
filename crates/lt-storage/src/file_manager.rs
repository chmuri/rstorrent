use std::collections::HashSet;
use std::os::unix::io::RawFd;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::error::StorageError;
use crate::file::{File, FileFlags};
use crate::mmap_safe::Prot;
use crate::socket_file::{OpenFlags, SocketFile};

/// Set once the first time the kernel/filesystem reports that `fallocate` is
/// unavailable. The fallback path is still attempted; this only suppresses
/// repeated log spam when it fails on every allocation.
static FALLOCATE_UNSUPPORTED_LOGGED: AtomicBool = AtomicBool::new(false);

/// Set once if the `posix_fallocate` fallback also fails, so an unsupported
/// filesystem does not emit a warning for every allocation.
static FALLOCATE_FALLBACK_FAILED_LOGGED: AtomicBool = AtomicBool::new(false);

/// Snapshot of the counters maintained by [`FileManager`].
///
/// Public path: `lt_storage::file_manager::FileManagerStats`. The crate root
/// intentionally does not re-export this type, so command/config layers should
/// import it from the module (or consume it through
/// [`crate::disk_thread::DiskThread::file_stats`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileManagerStats {
    /// Descriptors currently tracked as open (excluding padding files).
    pub open_files: usize,
    /// Files marked busy by `begin_io` and not yet released by `end_io`.
    pub in_flight: usize,
    /// Successful `open` calls, including re-opens of an already-open file.
    pub files_opened: u64,
    /// Descriptors actually handed to `close(2)` by this manager.
    pub files_closed: u64,
    /// `open` attempts that failed (after any FD-exhaustion retries).
    pub files_failed: u64,
    /// Successful `posix_fadvise(POSIX_FADV_RANDOM)` calls.
    pub fadvise: u64,
    /// `posix_fadvise` calls that failed while the hint was enabled.
    pub fadvise_failures: u64,
}

#[derive(Debug, Default)]
struct FileManagerCounters {
    open_files: AtomicUsize,
    in_flight: AtomicUsize,
    files_opened: AtomicU64,
    files_closed: AtomicU64,
    files_failed: AtomicU64,
    fadvise: AtomicU64,
    fadvise_failures: AtomicU64,
}

fn snapshot(counters: &FileManagerCounters) -> FileManagerStats {
    // Every field is independently atomic. A snapshot is not required to be a
    // single atomic transaction: command callers only need monotonic counters
    // and a current approximation of the open/in-flight gauges.
    FileManagerStats {
        open_files: counters.open_files.load(Ordering::Relaxed),
        in_flight: counters.in_flight.load(Ordering::Relaxed),
        files_opened: counters.files_opened.load(Ordering::Relaxed),
        files_closed: counters.files_closed.load(Ordering::Relaxed),
        files_failed: counters.files_failed.load(Ordering::Relaxed),
        fadvise: counters.fadvise.load(Ordering::Relaxed),
        fadvise_failures: counters.fadvise_failures.load(Ordering::Relaxed),
    }
}

/// Cheap, shareable reader for a [`FileManager`]'s counters.
///
/// The disk worker creates one of these before moving its manager onto the
/// worker thread, allowing [`crate::disk_thread::DiskThread`] to expose live
/// counters without locking the manager or racing with the worker's replies.
///
/// Public path: `lt_storage::file_manager::FileManagerStatsHandle`.
#[derive(Clone, Debug)]
pub struct FileManagerStatsHandle {
    counters: Arc<FileManagerCounters>,
}

impl FileManagerStatsHandle {
    pub fn stats(&self) -> FileManagerStats {
        snapshot(&self.counters)
    }

    pub fn files_opened_counter(&self) -> u64 {
        self.counters.files_opened.load(Ordering::Relaxed)
    }

    pub fn files_closed_counter(&self) -> u64 {
        self.counters.files_closed.load(Ordering::Relaxed)
    }

    pub fn files_failed_counter(&self) -> u64 {
        self.counters.files_failed.load(Ordering::Relaxed)
    }

    pub fn fadvise_counter(&self) -> u64 {
        self.counters.fadvise.load(Ordering::Relaxed)
    }
}

/// Tracks open file descriptors and applies the legacy libtorrent policies:
/// bounded descriptor count, least-active eviction, optional `fadvise` hints
/// and idle-close.
///
/// # Thread safety
///
/// The manager is designed to be owned by a single thread (normally the disk
/// worker). Mutating methods take `&mut self` and are **not** internally
/// synchronized; callers that need to share one must put it behind a `Mutex`.
/// Eviction and idle close cooperate with the owner through
/// [`FileManager::begin_io`]/[`FileManager::end_io`] so a descriptor with
/// in-flight I/O is never closed. Counter snapshots are lock-free and may be
/// read from other threads through [`FileManager::stats_handle`].
pub struct FileManager {
    open_file_ptrs: Vec<*mut File>,
    max_open_files: usize,
    counters: Arc<FileManagerCounters>,
    least_active_cache: Vec<(*mut File, u64)>,
    advise_random: bool,
    advise_random_hashing: bool,
    close_idle_secs: u64,
    last_close_idle_check: u64,
    /// Files with an operation in progress. Eviction and idle-close must not
    /// close these descriptors; the owner calls `begin_io`/`end_io` around
    /// the blocking operation (normally the disk worker).
    in_flight: HashSet<*mut File>,
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn is_fd_exhaustion(err: &std::io::Error) -> bool {
    matches!(err.raw_os_error(), Some(libc::EMFILE) | Some(libc::ENFILE))
}

/// Returns true only for the first unsupported-`fallocate` report.
fn log_fallocate_unsupported_once(errno: i32) -> bool {
    log_fallocate_unsupported_once_with(&FALLOCATE_UNSUPPORTED_LOGGED, errno)
}

fn log_fallocate_unsupported_once_with(flag: &AtomicBool, errno: i32) -> bool {
    if errno != libc::EOPNOTSUPP && errno != libc::ENOSYS {
        return false;
    }

    let first = !flag.swap(true, Ordering::Relaxed);
    if first {
        tracing::warn!(
            errno = errno,
            "fallocate is unsupported on this filesystem; using posix_fallocate/skipping preallocation"
        );
    }
    first
}

unsafe impl Send for FileManager {}
unsafe impl Sync for FileManager {}

impl Default for FileManager {
    fn default() -> Self {
        Self {
            open_file_ptrs: Vec::new(),
            max_open_files: 500,
            counters: Arc::new(FileManagerCounters::default()),
            least_active_cache: Vec::new(),
            advise_random: true,
            advise_random_hashing: false,
            close_idle_secs: 60,
            last_close_idle_check: 0,
            in_flight: HashSet::new(),
        }
    }
}

impl FileManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_files(&self) -> usize {
        self.open_file_ptrs.len()
    }

    pub fn max_open_files(&self) -> usize {
        self.max_open_files
    }

    pub fn set_max_open_files(&mut self, max: usize) {
        if max < 4 || max > (1 << 16) {
            panic!("Max open files must be between 4 and 65536");
        }
        self.max_open_files = max;
        self.verify_max_open_or_evict(0);
    }

    pub fn files_opened_counter(&self) -> u64 {
        self.counters.files_opened.load(Ordering::Relaxed)
    }

    pub fn files_closed_counter(&self) -> u64 {
        self.counters.files_closed.load(Ordering::Relaxed)
    }

    pub fn files_failed_counter(&self) -> u64 {
        self.counters.files_failed.load(Ordering::Relaxed)
    }

    /// Number of successful `posix_fadvise(POSIX_FADV_RANDOM)` calls.
    pub fn fadvise_counter(&self) -> u64 {
        self.counters.fadvise.load(Ordering::Relaxed)
    }

    /// Alias used by command layers that report file advice statistics.
    pub fn files_advised_counter(&self) -> u64 {
        self.fadvise_counter()
    }

    pub fn fadvise_failures(&self) -> u64 {
        self.counters.fadvise_failures.load(Ordering::Relaxed)
    }

    /// Snapshot of all file-manager counters.
    pub fn stats(&self) -> FileManagerStats {
        snapshot(&self.counters)
    }

    /// Creates a counter reader that remains valid after this manager is moved
    /// onto the disk-worker thread.
    pub fn stats_handle(&self) -> FileManagerStatsHandle {
        FileManagerStatsHandle {
            counters: Arc::clone(&self.counters),
        }
    }

    pub fn advise_random(&self) -> bool {
        self.advise_random
    }

    pub fn set_advise_random(&mut self, enabled: bool) {
        self.advise_random = enabled;
    }

    pub fn advise_random_hashing(&self) -> bool {
        self.advise_random_hashing
    }

    pub fn set_advise_random_hashing(&mut self, enabled: bool) {
        self.advise_random_hashing = enabled;
    }

    pub fn close_idle_secs(&self) -> u64 {
        self.close_idle_secs
    }

    pub fn set_close_idle_secs(&mut self, seconds: u64) {
        self.close_idle_secs = seconds;
    }

    /// Alias matching the `system.files.close_idle` option name.
    pub fn set_close_idle(&mut self, seconds: u64) {
        self.set_close_idle_secs(seconds);
    }

    pub fn in_flight_files(&self) -> usize {
        self.in_flight.len()
    }

    pub fn is_in_flight(&self, file: &File) -> bool {
        self.in_flight.contains(&(file as *const File as *mut File))
    }

    /// Marks a file as having an operation in progress. Eviction and idle
    /// close skip in-flight files until [`FileManager::end_io`] is called.
    pub fn begin_io(&mut self, file: &File) {
        let ptr = file as *const File as *mut File;
        if self.in_flight.insert(ptr) {
            self.counters.in_flight.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn end_io(&mut self, file: &File) {
        let ptr = file as *const File as *mut File;
        if self.in_flight.remove(&ptr) {
            self.counters.in_flight.fetch_sub(1, Ordering::Relaxed);
        }
    }

    pub fn touch(&mut self, file: &mut File) {
        file.set_last_touched(now_seconds());
    }

    /// Closes files that have not been touched for `close_idle_secs`.
    ///
    /// Files that are currently in flight (see [`FileManager::begin_io`]) are
    /// left open. A timeout of zero disables idle closing.
    pub fn close_idle(&mut self, now: u64) -> usize {
        let idle = self.close_idle_secs;
        if idle == 0 {
            return 0;
        }

        let mut to_close = Vec::new();
        for &ptr in &self.open_file_ptrs {
            let file = unsafe { &*ptr };
            if !file.is_open() || file.is_padding() {
                continue;
            }
            if self.in_flight.contains(&ptr) {
                continue;
            }
            if now.saturating_sub(file.last_touched()) >= idle {
                to_close.push(ptr);
            }
        }

        let count = to_close.len();
        for ptr in to_close {
            self.close(unsafe { &mut *ptr });
        }
        count
    }

    pub fn close_idle_now(&mut self) -> usize {
        self.close_idle(now_seconds())
    }

    /// Scheduler-friendly variant that does not scan more often than a quarter
    /// of the configured timeout, mirroring `periodic_close_idle` in libtorrent.
    pub fn periodic_close_idle(&mut self, now: u64) -> usize {
        if self.close_idle_secs == 0 || self.open_file_ptrs.is_empty() {
            return 0;
        }

        let interval = (self.close_idle_secs / 4).max(1);
        if now.saturating_sub(self.last_close_idle_check) < interval {
            return 0;
        }

        self.last_close_idle_check = now;
        self.close_idle(now)
    }

    pub fn open(&mut self, file: &mut File, hashing: bool, prot: Prot, flags: OpenFlags) -> bool {
        if file.is_padding() {
            return true;
        }

        if file.is_open() {
            self.close(file);
        }

        self.verify_max_open_or_evict(1);

        let mut socket_file = SocketFile::new();
        let mut retries = 0;

        loop {
            match socket_file.open(file.frozen_path(), prot, flags, None) {
                Ok(()) => break,
                Err(StorageError::Io(ref err))
                    if is_fd_exhaustion(err) && retries < 2 && !self.open_file_ptrs.is_empty() =>
                {
                    // Another part of the process (or this manager's own cache)
                    // filled the descriptor table. Evict a batch and retry
                    // rather than failing the piece outright.
                    retries += 1;
                    let evict = (self.open_file_ptrs.len() / 2).max(1);
                    self.evict_least_active(evict);
                }
                Err(_) => {
                    self.counters
                        .files_failed
                        .fetch_add(1, Ordering::Relaxed);
                    return false;
                }
            }
        }

        let raw_fd = socket_file.fd();
        std::mem::forget(socket_file); // FileManager takes ownership of closing

        let advise = if hashing {
            self.advise_random_hashing
        } else {
            self.advise_random
        };
        if advise && apply_fadvise_random(raw_fd) {
            self.counters.fadvise.fetch_add(1, Ordering::Relaxed);
        } else if advise {
            self.counters
                .fadvise_failures
                .fetch_add(1, Ordering::Relaxed);
        }

        if file.flags().contains(FileFlags::FALLOCATE) {
            allocate_for_file(raw_fd, file.size_bytes());
        }

        file.set_protection(prot);
        file.set_file_descriptor(raw_fd);
        file.set_last_touched(now_seconds());

        let ptr = file as *mut File;
        self.open_file_ptrs.push(ptr);
        self.counters.open_files.fetch_add(1, Ordering::Relaxed);
        self.counters.files_opened.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub fn close(&mut self, file: &mut File) {
        let ptr = file as *mut File;

        if self.in_flight.remove(&ptr) {
            self.counters.in_flight.fetch_sub(1, Ordering::Relaxed);
        }
        self.least_active_cache.retain(|(cached, _)| *cached != ptr);

        if !file.is_open() || file.is_padding() {
            return;
        }

        let fd = file.file_descriptor();
        file.reset_file_descriptor();
        file.set_protection(Prot::NONE);

        if fd != -1 {
            unsafe {
                libc::close(fd);
            }
            self.counters.files_closed.fetch_add(1, Ordering::Relaxed);
        }

        if let Some(pos) = self.open_file_ptrs.iter().position(|&p| p == ptr) {
            self.open_file_ptrs.swap_remove(pos);
            self.counters.open_files.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn verify_max_open_or_evict(&mut self, reserve: usize) {
        if self.open_file_ptrs.len() + reserve > self.max_open_files {
            let to_evict = (self.open_file_ptrs.len() + reserve) - self.max_open_files;
            self.evict_least_active(to_evict);
        }
    }

    fn evict_least_active(&mut self, mut count: usize) {
        if count == 0 || self.open_file_ptrs.is_empty() {
            return;
        }

        count = self.evict_least_active_from_cache(count);
        if count == 0 || self.open_file_ptrs.is_empty() {
            return;
        }

        let cache_size = (self.max_open_files / 16).max(2);
        let target_candidates = count.saturating_add(cache_size);

        let mut sorted_ptrs: Vec<*mut File> = self
            .open_file_ptrs
            .iter()
            .copied()
            .filter(|&ptr| {
                if self.in_flight.contains(&ptr) {
                    return false;
                }
                let file = unsafe { &*ptr };
                file.is_open() && !file.is_padding()
            })
            .collect();
        sorted_ptrs.sort_by_key(|&p| unsafe { (*p).last_touched() });

        let to_close_count = count.min(sorted_ptrs.len());
        let to_close = sorted_ptrs[..to_close_count].to_vec();

        self.least_active_cache.clear();
        if sorted_ptrs.len() > to_close_count {
            let remaining_candidates =
                &sorted_ptrs[to_close_count..target_candidates.min(sorted_ptrs.len())];
            for &p in remaining_candidates {
                let touched = unsafe { (*p).last_touched() };
                self.least_active_cache.push((p, touched));
            }
        }

        for p in to_close {
            self.close(unsafe { &mut *p });
        }
    }

    /// Evicts cached candidates until `count` files have been closed or the
    /// cache is exhausted. Busy candidates stay cached for a later eviction.
    fn evict_least_active_from_cache(&mut self, mut count: usize) -> usize {
        if count == 0 {
            return 0;
        }

        let cached = std::mem::take(&mut self.least_active_cache);
        let mut remaining = Vec::with_capacity(cached.len());

        for (ptr, touched) in cached {
            if count == 0 {
                remaining.push((ptr, touched));
                continue;
            }

            if !self.open_file_ptrs.contains(&ptr) {
                // Stale candidate: drop it and let the scan below find a fresh one.
                continue;
            }

            let file = unsafe { &mut *ptr };
            if !file.is_open() || file.is_padding() || file.last_touched() != touched {
                continue;
            }

            if self.in_flight.contains(&ptr) {
                remaining.push((ptr, touched));
                continue;
            }

            self.close(file);
            count -= 1;
        }

        self.least_active_cache = remaining;
        count
    }
}

fn apply_fadvise_random(fd: RawFd) -> bool {
    #[cfg(target_os = "linux")]
    {
        // `posix_fadvise` returns an error number directly rather than setting errno.
        unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_RANDOM) == 0 }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Not available on macOS/BSD; leave the kernel heuristics alone.
        let _ = fd;
        false
    }
}

fn allocate_for_file(fd: RawFd, size: u64) {
    if size == 0 {
        return;
    }

    #[cfg(target_os = "linux")]
    {
        let ret = unsafe { libc::fallocate(fd, 0, 0, size as libc::off_t) };
        if ret == 0 {
            return;
        }

        let err = std::io::Error::last_os_error();
        let errno = err.raw_os_error().unwrap_or(0);

        if errno == libc::EOPNOTSUPP || errno == libc::ENOSYS {
            log_fallocate_unsupported_once(errno);

            // Explicit fallback for filesystems without Linux fallocate.
            let posix_ret = unsafe { libc::posix_fallocate(fd, 0, size as libc::off_t) };
            if posix_ret != 0 && !FALLOCATE_FALLBACK_FAILED_LOGGED.swap(true, Ordering::Relaxed)
            {
                tracing::warn!(
                    errno = posix_ret,
                    "posix_fallocate fallback also failed; continuing without preallocation"
                );
            }
            return;
        }

        tracing::debug!(
            errno = errno,
            "fallocate failed; continuing without preallocation"
        );
    }

    #[cfg(not(target_os = "linux"))]
    {
        // macOS/BSD have no fallocate; extend the file instead (sparse).
        let ret = unsafe { libc::ftruncate(fd, size as libc::off_t) };
        if ret != 0 {
            tracing::debug!(
                errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
                "ftruncate failed; continuing without preallocation"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rtorrent_file_manager_unit_{}_{}.bin",
            tag,
            std::process::id()
        ))
    }

    fn open_file(manager: &mut FileManager, path: &PathBuf, hashing: bool) -> Box<File> {
        // The manager keeps a raw pointer to the File, so it must live at a
        // stable address (a Box) rather than a stack slot that is moved on
        // return.
        let mut file = Box::new(File::new());
        file.set_path(path);
        file.set_size_bytes(4096);
        assert!(manager.open(
            &mut file,
            hashing,
            Prot::READ | Prot::WRITE,
            OpenFlags::CREATE,
        ));
        file
    }

    #[test]
    fn fadvise_is_gated_by_mode_and_hashing() {
        let path = temp_path("fadvise");
        let _ = fs::remove_file(&path);

        let mut manager = FileManager::new();
        manager.set_advise_random(true);
        manager.set_advise_random_hashing(false);

        let mut normal = open_file(&mut manager, &path, false);
        assert_eq!(manager.fadvise_counter(), 1);
        manager.close(&mut normal);

        manager.set_advise_random(false);
        let mut quiet = open_file(&mut manager, &path, false);
        assert_eq!(manager.fadvise_counter(), 1);
        manager.close(&mut quiet);

        manager.set_advise_random(true);
        let mut hashing_without_hint = open_file(&mut manager, &path, true);
        assert_eq!(manager.fadvise_counter(), 1);
        manager.close(&mut hashing_without_hint);

        manager.set_advise_random_hashing(true);
        let mut hashing = open_file(&mut manager, &path, true);
        assert_eq!(manager.fadvise_counter(), 2);
        manager.close(&mut hashing);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn padding_files_are_not_opened_or_advised() {
        let path = temp_path("padding");
        let _ = fs::remove_file(&path);

        let mut manager = FileManager::new();
        let mut file = File::new();
        file.set_path(&path);
        file.set_flags(FileFlags::ATTR_PADDING);

        assert!(manager.open(
            &mut file,
            false,
            Prot::READ | Prot::WRITE,
            OpenFlags::CREATE,
        ));
        assert!(!file.is_open());
        assert_eq!(manager.open_files(), 0);
        assert_eq!(manager.files_opened_counter(), 0);
        assert_eq!(manager.fadvise_counter(), 0);
    }

    #[test]
    fn eviction_uses_least_active_cache() {
        let dir = std::env::temp_dir().join(format!(
            "rtorrent_file_manager_cache_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut manager = FileManager::new();
        manager.set_max_open_files(64);

        let mut files: Vec<Box<File>> = Vec::new();
        for i in 0..48u64 {
            let path = dir.join(format!("file_{i}"));
            let mut file = Box::new(File::new());
            file.set_path(&path);
            assert!(manager.open(
                &mut file,
                false,
                Prot::READ | Prot::WRITE,
                OpenFlags::CREATE,
            ));
            file.set_last_touched(100 + i * 10);
            files.push(file);
        }

        manager.set_max_open_files(47);
        assert_eq!(manager.files_closed_counter(), 1);
        assert!(!files[0].is_open());
        assert!(files[1].is_open());
        assert!(files[2].is_open());

        // A scan would pick file 3 now, but the cache still holds file 1 as the
        // next least-active candidate.
        files[3].set_last_touched(1);
        manager.set_max_open_files(46);

        assert!(!files[1].is_open());
        assert!(files[2].is_open());
        assert!(files[3].is_open());
        assert_eq!(manager.files_closed_counter(), 2);

        for file in &mut files {
            manager.close(file);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn idle_close_skips_in_flight_files() {
        let first_path = temp_path("idle_first");
        let second_path = temp_path("idle_second");
        let _ = fs::remove_file(&first_path);
        let _ = fs::remove_file(&second_path);

        let mut manager = FileManager::new();
        manager.set_close_idle_secs(10);

        let first = open_file(&mut manager, &first_path, false);
        let second = open_file(&mut manager, &second_path, false);

        manager.begin_io(&first);
        assert_eq!(manager.in_flight_files(), 1);
        assert_eq!(manager.close_idle(u64::MAX), 1);
        assert!(first.is_open());
        assert!(!second.is_open());

        manager.end_io(&first);
        assert_eq!(manager.close_idle(u64::MAX), 1);
        assert!(!first.is_open());
        assert_eq!(manager.open_files(), 0);
        assert_eq!(manager.files_closed_counter(), 2);

        let _ = fs::remove_file(&first_path);
        let _ = fs::remove_file(&second_path);
    }

    #[test]
    fn idle_close_uses_configured_timeout_and_periodic_gate() {
        let path = temp_path("periodic");
        let _ = fs::remove_file(&path);

        let mut manager = FileManager::new();
        manager.set_close_idle_secs(40);
        let file = open_file(&mut manager, &path, false);
        let touched = file.last_touched();

        // Too early for the periodic gate.
        assert_eq!(manager.periodic_close_idle(touched + 5), 0);
        assert!(file.is_open());

        // The direct close still honours the timeout boundary.
        assert_eq!(manager.close_idle(touched + 39), 0);
        assert_eq!(manager.close_idle(touched + 40), 1);
        assert!(!file.is_open());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn failed_open_increments_counter() {
        let dir =
            std::env::temp_dir().join(format!("rtorrent_missing_parent_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("file.bin");

        let mut manager = FileManager::new();
        let mut file = File::new();
        file.set_path(&path);

        assert!(!manager.open(
            &mut file,
            false,
            Prot::READ | Prot::WRITE,
            OpenFlags::CREATE,
        ));
        assert_eq!(manager.files_failed_counter(), 1);
        assert_eq!(manager.files_opened_counter(), 0);
    }

    #[test]
    fn fallocate_flag_is_handled_without_failing_open() {
        let path = temp_path("fallocate");
        let _ = fs::remove_file(&path);

        let mut manager = FileManager::new();
        let mut file = File::new();
        file.set_path(&path);
        file.set_size_bytes(8192);
        file.set_flags(FileFlags::FALLOCATE);

        assert!(manager.open(
            &mut file,
            false,
            Prot::READ | Prot::WRITE,
            OpenFlags::CREATE,
        ));
        assert!(file.is_open());
        manager.close(&mut file);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn fd_exhaustion_errors_are_recognized() {
        let too_many = std::io::Error::from_raw_os_error(libc::EMFILE);
        let system = std::io::Error::from_raw_os_error(libc::ENFILE);
        let missing = std::io::Error::from_raw_os_error(libc::ENOENT);

        assert!(is_fd_exhaustion(&too_many));
        assert!(is_fd_exhaustion(&system));
        assert!(!is_fd_exhaustion(&missing));
    }

    #[test]
    fn unsupported_fallocate_is_reported_once() {
        let flag = AtomicBool::new(false);

        // First report wins even if called repeatedly.
        let first = log_fallocate_unsupported_once_with(&flag, libc::EOPNOTSUPP);
        let second = log_fallocate_unsupported_once_with(&flag, libc::ENOSYS);
        assert!(first);
        assert!(!second);
        assert!(!log_fallocate_unsupported_once_with(&flag, libc::EIO));
    }

    #[test]
    fn touch_refreshes_last_access() {
        let path = temp_path("touch");
        let _ = fs::remove_file(&path);

        let mut manager = FileManager::new();
        let mut file = open_file(&mut manager, &path, false);

        file.set_last_touched(1);
        manager.touch(&mut file);
        assert!(file.last_touched() > 1);

        manager.close(&mut file);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn stats_snapshot_and_handle_track_counters() {
        let path = temp_path("stats");
        let _ = fs::remove_file(&path);

        let mut manager = FileManager::new();
        let handle = manager.stats_handle();

        let file = open_file(&mut manager, &path, false);
        let stats = manager.stats();
        assert_eq!(stats.open_files, 1);
        assert_eq!(stats.in_flight, 0);
        assert_eq!(stats.files_opened, 1);
        assert_eq!(stats.files_closed, 0);
        assert_eq!(stats.files_failed, 0);
        assert_eq!(stats.fadvise, 1);
        assert_eq!(stats.fadvise_failures, 0);

        // The handle observes the same counters after the manager is moved.
        let handle_stats = handle.stats();
        assert_eq!(handle_stats.files_opened, 1);
        assert_eq!(handle_stats.fadvise, 1);
        assert_eq!(handle.files_opened_counter(), 1);
        assert_eq!(handle.fadvise_counter(), 1);

        manager.begin_io(&file);
        assert_eq!(manager.stats().in_flight, 1);
        assert_eq!(handle.stats().in_flight, 1);
        manager.end_io(&file);
        assert_eq!(manager.stats().in_flight, 0);

        let mut file = file;
        manager.close(&mut file);
        let stats = handle.stats();
        assert_eq!(stats.open_files, 0);
        assert_eq!(stats.files_closed, 1);
        assert_eq!(handle.files_closed_counter(), 1);

        // Failed opens are counted but never inflate the open gauge.
        let missing = temp_path("stats_missing").join("no_such_dir").join("f");
        let mut missing_file = File::new();
        missing_file.set_path(&missing);
        assert!(!manager.open(
            &mut missing_file,
            false,
            Prot::READ,
            OpenFlags::empty(),
        ));
        assert_eq!(handle.files_failed_counter(), 1);
        assert_eq!(handle.stats().open_files, 0);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn repeated_begin_and_end_io_do_not_underflow_the_guard() {
        let path = temp_path("guard");
        let _ = fs::remove_file(&path);

        let mut manager = FileManager::new();
        let file = open_file(&mut manager, &path, false);

        manager.begin_io(&file);
        manager.begin_io(&file);
        assert_eq!(manager.in_flight_files(), 1);
        assert_eq!(manager.stats().in_flight, 1);

        manager.end_io(&file);
        assert_eq!(manager.in_flight_files(), 0);
        manager.end_io(&file);
        assert_eq!(manager.stats().in_flight, 0);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn concurrent_close_idle_never_closes_in_flight_files() {
        use std::sync::{Arc, Mutex};

        let busy_path = temp_path("race_busy");
        let idle_path = temp_path("race_idle");
        let _ = fs::remove_file(&busy_path);
        let _ = fs::remove_file(&idle_path);

        let mut manager = FileManager::new();
        manager.set_close_idle_secs(1);

        let busy = open_file(&mut manager, &busy_path, false);
        let idle = open_file(&mut manager, &idle_path, false);
        manager.begin_io(&busy);

        let shared = Arc::new(Mutex::new(manager));
        let worker = {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                for _ in 0..256 {
                    let mut guard = shared.lock().unwrap();
                    guard.close_idle(u64::MAX);
                    drop(guard);
                    std::thread::yield_now();
                }
            })
        };
        worker.join().unwrap();

        let mut guard = shared.lock().unwrap();
        // `close_idle` may close the idle file, but must never close a file
        // whose owner announced in-flight I/O.
        assert!(busy.is_open());
        assert!(!idle.is_open());
        assert_eq!(guard.in_flight_files(), 1);
        assert_eq!(guard.stats().in_flight, 1);

        guard.end_io(&busy);
        assert_eq!(guard.close_idle(u64::MAX), 1);
        assert!(!busy.is_open());
        assert_eq!(guard.stats().open_files, 0);
        drop(guard);
        drop(shared);

        let _ = fs::remove_file(&busy_path);
        let _ = fs::remove_file(&idle_path);
    }
}
