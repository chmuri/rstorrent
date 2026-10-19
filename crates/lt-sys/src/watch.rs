use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::fs::FileStat;

pub mod flags {
    pub const FLAG_ON_ADDED: i32 = 0x1;
    pub const FLAG_ON_REMOVED: i32 = 0x2;
    pub const FLAG_ON_UPDATED: i32 = 0x3;
    pub const FLAG_ON_READY: i32 = 0x4;
}

pub type WatchCallback = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone)]
pub struct WatchDescriptor {
    pub flags: i32,
    pub canonical_path: PathBuf,
    pub path_prefix: String,
    pub callback: WatchCallback,
}

pub fn check_flags_conflict(lhs: i32, rhs: i32) -> bool {
    ((lhs & flags::FLAG_ON_READY) != 0 && (rhs & flags::FLAG_ON_ADDED) != 0)
        || ((rhs & flags::FLAG_ON_READY) != 0 && (lhs & flags::FLAG_ON_ADDED) != 0)
}

pub struct DirectoryEvents {
    is_open: bool,
    watch_descriptors: Vec<WatchDescriptor>,
}

impl Default for DirectoryEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl DirectoryEvents {
    pub fn new() -> Self {
        Self {
            is_open: false,
            watch_descriptors: Vec::new(),
        }
    }

    pub fn open(&mut self) -> bool {
        self.is_open = true;
        true
    }

    pub fn close(&mut self) {
        self.is_open = false;
        self.watch_descriptors.clear();
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn notify_on(
        &mut self,
        path: impl AsRef<Path>,
        watch_flags: i32,
        callback: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<(), String> {
        let p = path.as_ref();
        if p.as_os_str().is_empty() {
            return Err("Cannot add notification event for empty paths.".into());
        }

        let canonical = p
            .canonicalize()
            .map_err(|e| format!("Could not resolve watch directory '{:?}': {e}", p))?;

        for wd in &self.watch_descriptors {
            if check_flags_conflict(wd.flags, watch_flags) && wd.canonical_path == canonical {
                return Err(format!(
                    "Conflicting directory watch modes for watch directories: {:?} : {:?}",
                    wd.canonical_path, canonical
                ));
            }
        }

        let mut path_str = p.to_string_lossy().into_owned();
        if !path_str.ends_with('/') {
            path_str.push('/');
        }

        self.watch_descriptors.push(WatchDescriptor {
            flags: watch_flags,
            canonical_path: canonical,
            path_prefix: path_str,
            callback: Arc::new(callback),
        });

        Ok(())
    }

    pub fn trigger_file_event(&self, full_file_path: impl AsRef<Path>) {
        let path = full_file_path.as_ref();
        let filename = match path.file_name().and_then(|n| n.to_str()) {
            Some(f) => f,
            None => return,
        };

        if !filename.ends_with(".torrent") {
            return;
        }

        let parent = match path.parent().and_then(|p| p.canonicalize().ok()) {
            Some(p) => p,
            None => return,
        };

        for wd in &self.watch_descriptors {
            if wd.canonical_path == parent {
                let report_path = format!("{}{filename}", wd.path_prefix);
                (wd.callback)(report_path);
            }
        }
    }
}

//
// WatchReadyQueue
//

#[derive(Debug, Clone)]
struct ReadyEntry {
    command: String,
    path: String,
    regular: bool,
    size: i64,
    mtime: i64,
    first_seen: Duration,
    last_changed: Duration,
    next_time: Duration,
}

pub struct WatchReadyQueue {
    active: bool,
    entries: BTreeMap<String, ReadyEntry>,
    quiet_time: Duration,
    retry_time: Duration,
    stale_time: Duration,
}

impl Default for WatchReadyQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl WatchReadyQueue {
    pub fn new() -> Self {
        Self {
            active: true,
            entries: BTreeMap::new(),
            quiet_time: Duration::from_millis(500),
            retry_time: Duration::from_millis(250),
            stale_time: Duration::from_secs(10),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn shutdown(&mut self) {
        self.active = false;
        self.entries.clear();
    }

    pub fn push(&mut self, command: impl Into<String>, path: impl Into<String>, current_time: Duration) {
        if !self.active {
            return;
        }

        let p = path.into();
        let cmd = command.into();

        let mut fs = FileStat::new();
        let regular = if fs.update(&p) { fs.is_regular() } else { false };
        let size = if regular { fs.size() } else { -1 };
        let mtime = if regular { fs.modified_time() } else { 0 };

        if let Some(entry) = self.entries.get_mut(&p) {
            entry.command = cmd;
            if entry.regular != regular || entry.size != size || entry.mtime != mtime {
                entry.regular = regular;
                entry.size = size;
                entry.mtime = mtime;
                entry.last_changed = current_time;
            } else {
                entry.last_changed = current_time;
            }
            Self::update_next_time(entry, current_time, self.quiet_time, self.retry_time, self.stale_time);
        } else {
            let mut entry = ReadyEntry {
                command: cmd,
                path: p.clone(),
                regular,
                size,
                mtime,
                first_seen: current_time,
                last_changed: current_time,
                next_time: current_time,
            };
            Self::update_next_time(&mut entry, current_time, self.quiet_time, self.retry_time, self.stale_time);
            self.entries.insert(p, entry);
        }
    }

    fn update_next_time(
        entry: &mut ReadyEntry,
        current_time: Duration,
        quiet_time: Duration,
        retry_time: Duration,
        stale_time: Duration,
    ) {
        if entry.regular && entry.size > 0 {
            entry.next_time = entry.last_changed + quiet_time;
        } else {
            entry.next_time = (current_time + retry_time).min(entry.first_seen + stale_time);
        }
    }

    pub fn process(&mut self, current_time: Duration) -> Vec<(String, String)> {
        let mut ready = Vec::new();
        let mut to_remove = Vec::new();

        for (path, entry) in &mut self.entries {
            if entry.next_time > current_time {
                continue;
            }

            let mut fs = FileStat::new();
            let regular = if fs.update(&entry.path) { fs.is_regular() } else { false };
            let size = if regular { fs.size() } else { -1 };
            let mtime = if regular { fs.modified_time() } else { 0 };

            if entry.regular != regular || entry.size != size || entry.mtime != mtime {
                entry.regular = regular;
                entry.size = size;
                entry.mtime = mtime;
                entry.last_changed = current_time;
            }

            let unchanged = current_time.saturating_sub(entry.last_changed) >= self.quiet_time;
            let stale = current_time.saturating_sub(entry.first_seen) >= self.stale_time;
            let nonempty_regular = entry.regular && entry.size > 0;

            if nonempty_regular && unchanged {
                ready.push((entry.command.clone(), entry.path.clone()));
                to_remove.push(path.clone());
            } else if !nonempty_regular && stale {
                to_remove.push(path.clone());
            } else {
                Self::update_next_time(entry, current_time, self.quiet_time, self.retry_time, self.stale_time);
            }
        }

        for p in to_remove {
            self.entries.remove(&p);
        }

        ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_directory_events_conflict() {
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join("lt_watch_test_conflict");
        let _ = std::fs::create_dir(&test_dir);

        let mut events = DirectoryEvents::new();
        assert!(events.open());

        events
            .notify_on(&test_dir, flags::FLAG_ON_READY, |_| {})
            .unwrap();

        // Adding FLAG_ON_ADDED on same directory must fail with conflict
        let res = events.notify_on(&test_dir, flags::FLAG_ON_ADDED, |_| {});
        assert!(res.is_err());

        events.close();
        let _ = std::fs::remove_dir(&test_dir);
    }

    #[test]
    fn test_directory_events_symlink_callback_path() {
        let temp_dir = std::env::temp_dir();
        let root = temp_dir.join("lt_watch_symlink_root");
        let link = temp_dir.join("lt_watch_symlink_link");

        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&root);

        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, &link).unwrap();

        let mut events = DirectoryEvents::new();
        assert!(events.open());

        let called_path = Arc::new(std::sync::Mutex::new(String::new()));
        let cp = Arc::clone(&called_path);

        events
            .notify_on(&link, flags::FLAG_ON_ADDED, move |p| {
                *cp.lock().unwrap() = p;
            })
            .unwrap();

        let torrent_file = root.join("test.torrent");
        std::fs::write(&torrent_file, "torrent data").unwrap();

        events.trigger_file_event(&torrent_file);

        let result = called_path.lock().unwrap().clone();
        assert!(result.starts_with(link.to_str().unwrap()));
        assert!(result.ends_with("test.torrent"));

        events.close();
        let _ = std::fs::remove_file(&torrent_file);
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir(&root);
    }

    #[test]
    fn test_watch_ready_queue_quiet_time() {
        let temp_dir = std::env::temp_dir();
        let file = temp_dir.join("watch_ready_test.torrent");
        std::fs::write(&file, "torrent").unwrap();
        let file_path = file.to_str().unwrap();

        let mut queue = WatchReadyQueue::new();
        let mut time = Duration::from_millis(0);

        queue.push("load", file_path, time);

        // Repeated event at 300ms resets quiet time
        time += Duration::from_millis(300);
        queue.push("load", file_path, time);

        // At 600ms (300ms after last event), should not be ready yet
        time += Duration::from_millis(300);
        let ready = queue.process(time);
        assert!(ready.is_empty());

        // At 801ms (501ms after last event), should be ready!
        time += Duration::from_millis(201);
        let ready2 = queue.process(time);
        assert_eq!(ready2.len(), 1);
        assert_eq!(ready2[0].0, "load");
        assert_eq!(ready2[0].1, file_path);

        let _ = std::fs::remove_file(&file);
    }
}
