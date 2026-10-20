//! Public-surface coverage for `FileStatusCache` statistics, pruning and
//! write/rename invalidation. The cache itself is intentionally not
//! internally synchronized; shared callers wrap it in a `Mutex`.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use lt_storage::file_status_cache::{FileStatus, FileStatusCache, FileStatusCacheStats};

fn temp_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rtorrent_file_status_cache_ops_{}_{}_{}.tmp",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn append(path: &PathBuf, bytes: &[u8]) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

#[test]
fn stats_and_invalidation_follow_writes_and_renames() {
    let written = temp_path("written");
    let renamed_from = temp_path("renamed_from");
    let renamed_to = temp_path("renamed_to");
    fs::write(&written, b"abcd").unwrap();
    fs::write(&renamed_from, b"old").unwrap();

    let mut cache = FileStatusCache::new();
    let status: FileStatus = cache.stat(&written).unwrap();
    assert_eq!(status.size, 4);
    assert!(!status.is_dir);

    let before: FileStatusCacheStats = cache.stats();
    assert_eq!(before.size, 1);
    assert_eq!(before.inserts, 1);
    assert_eq!(before.misses, 1);
    assert_eq!(before.hits, 0);

    // A fresh hit is served from the cache.
    assert_eq!(cache.stat(&written).unwrap().size, 4);
    assert_eq!(cache.hits(), 1);
    assert_eq!(cache.misses(), 1);

    // Writes invalidate the entry; the next lookup re-stats the file.
    append(&written, b"ef");
    assert!(cache.invalidate_after_write(&written));
    assert_eq!(cache.invalidations(), 1);
    assert_eq!(cache.stat(&written).unwrap().size, 6);
    assert_eq!(cache.inserts(), 2);
    assert_eq!(cache.misses(), 2);
    assert_eq!(cache.updates(), 0);

    // Both sides of a rename are invalidated when cached.
    cache.stat(&renamed_from).unwrap();
    fs::rename(&renamed_from, &renamed_to).unwrap();
    assert_eq!(cache.invalidate_after_rename(&renamed_from, &renamed_to), 1);
    assert!(cache.get(&renamed_from).is_none());
    assert_eq!(cache.stat(&renamed_to).unwrap().size, 3);
    assert_eq!(cache.invalidations(), 2);

    assert_eq!(cache.invalidate_rename(&renamed_to, &renamed_to), 1);
    assert_eq!(cache.invalidations(), 3);

    let stats = cache.stats();
    assert_eq!(stats.invalidations, 3);
    assert_eq!(stats.size, 1);

    fs::remove_file(&written).unwrap();
    fs::remove_file(&renamed_to).unwrap();
}

#[test]
fn prune_removes_changed_missing_and_aged_entries() {
    let changed = temp_path("changed");
    let missing = temp_path("missing");
    let aged = temp_path("aged");
    fs::write(&changed, b"a").unwrap();
    fs::write(&missing, b"b").unwrap();
    fs::write(&aged, b"c").unwrap();

    let mut cache = FileStatusCache::new();
    // Long age limit: only filesystem validation can remove changed/missing
    // entries in the first prune.
    cache.set_prune_interval(Duration::from_secs(300));
    cache.stat(&changed).unwrap();
    cache.stat(&missing).unwrap();
    cache.stat(&aged).unwrap();
    assert_eq!(cache.len(), 3);

    append(&changed, b"more");
    fs::remove_file(&missing).unwrap();
    assert_eq!(cache.prune(), 2);
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.pruned_entries(), 2);

    // A short interval ages out the remaining entry. Keep the test robust by
    // sleeping well past the millisecond boundary.
    cache.set_prune_interval(Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(cache.prune_if_due(), 1);
    assert!(cache.is_empty());
    assert_eq!(cache.pruned_entries(), 3);

    // A forced prune of an empty cache is a no-op but still counts a run.
    assert_eq!(cache.prune(), 0);

    let stats = cache.stats();
    assert_eq!(stats.size, 0);
    assert_eq!(stats.prune_runs, 3);
    assert_eq!(stats.pruned_entries, 3);
    assert_eq!(stats.inserts, 3);

    let _ = fs::remove_file(&changed);
    let _ = fs::remove_file(&aged);
}

#[test]
fn prune_stale_revalidates_without_age() {
    let kept = temp_path("stale_kept");
    let removed = temp_path("stale_removed");
    fs::write(&kept, b"keep").unwrap();
    fs::write(&removed, b"remove").unwrap();

    let mut cache = FileStatusCache::new();
    cache.set_prune_interval(Duration::from_secs(300));
    cache.stat(&kept).unwrap();
    cache.stat(&removed).unwrap();

    fs::remove_file(&removed).unwrap();
    assert_eq!(cache.prune_stale(), 1);
    assert_eq!(cache.len(), 1);
    assert!(cache.get(&kept).is_some());
    assert_eq!(cache.prune_runs(), 1);
    assert_eq!(cache.pruned_entries(), 1);

    let _ = fs::remove_file(&kept);
}
