//! Integration tests exercising the public `lt-sys` API from an external
//! consumer's perspective: the filesystem lockfile lifecycle and the in-memory
//! log buffer. Each test uses unique temp paths / its own buffer so the suite
//! stays parallel-safe and free of process-global state.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use lt_sys::{Lockfile, LogBuffer, LogEntry};

fn unique_lock_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("rt_it_{}_{}.lock", std::process::id(), tag))
}

#[test]
fn lockfile_acquire_contend_release() {
    let path = unique_lock_path("contend");
    let _ = std::fs::remove_file(&path);
    let path_str = path.to_str().unwrap().to_string();

    let mut lock = Lockfile::with_path(path_str.clone());
    assert!(lock.try_lock());
    assert!(lock.is_locked());
    assert!(path.exists());

    let mut other = Lockfile::with_path(path_str);
    assert!(!other.try_lock());
    assert!(!other.is_locked());

    let (host, pid) = lock.locked_by();
    assert!(!host.is_empty());
    assert_eq!(pid, std::process::id() as i32);
    assert_eq!(lock.locked_by_as_string(), format!("{host}:+{pid}"));

    assert!(lock.unlock());
    assert!(!lock.is_locked());
    assert!(!path.exists());
}

#[test]
fn lockfile_stale_reclaim() {
    let path = unique_lock_path("stale");
    let _ = std::fs::remove_file(&path);
    let path_str = path.to_str().unwrap().to_string();

    let mut probe = Lockfile::with_path(path_str.clone());
    assert!(probe.try_lock());
    let (host, _) = probe.locked_by();
    assert!(probe.unlock());

    std::fs::write(&path, format!("{host}:+99999999\n")).unwrap();

    let mut lock = Lockfile::with_path(path_str);
    assert!(lock.is_stale());
    assert!(lock.try_lock());
    assert_eq!(lock.locked_by().1, std::process::id() as i32);
    assert!(lock.unlock());
    assert!(!path.exists());
}

#[test]
fn lockfile_drop_removes_file() {
    let path = unique_lock_path("drop");
    let _ = std::fs::remove_file(&path);
    {
        let mut lock = Lockfile::with_path(path.to_str().unwrap().to_string());
        assert!(lock.try_lock());
        assert!(path.exists());
    }
    assert!(!path.exists());
}

#[test]
fn log_buffer_eviction_and_group_filter() {
    let mut buf = LogBuffer::new(3);
    assert!(buf.is_empty());
    buf.push_log("a", 1, Some(100));
    buf.push_log("b", 1, Some(101));
    buf.push_log("c", 1, Some(102));
    assert_eq!(buf.len(), 3);

    buf.push_log("d", 1, Some(103));
    assert_eq!(buf.len(), 3);
    assert_eq!(buf.entries().front().unwrap().message, "b");
    assert_eq!(buf.back().unwrap().message, "d");

    buf.push_log("ignored", -1, Some(104));
    assert_eq!(buf.len(), 3);
    assert_eq!(buf.back().unwrap().message, "d");

    buf.push_log("y", 7, Some(105));
    assert_eq!(buf.back().unwrap().group, 1);
    assert_eq!(buf.back().unwrap().message, "y");
    assert_eq!(buf.entries().front().unwrap().message, "c");
}

#[test]
fn log_buffer_find_older() {
    let mut buf = LogBuffer::new(10);
    assert_eq!(buf.find_older(0), None);

    buf.push_log("first", 0, Some(1000));
    assert_eq!(buf.find_older(999), Some(0));
    assert_eq!(buf.find_older(1000), None);
    assert_eq!(buf.find_older(1001), None);

    buf.push_log("second", 0, Some(1010));
    assert_eq!(buf.find_older(1000), Some(0));
    assert_eq!(buf.find_older(1009), Some(1));
    assert_eq!(buf.find_older(1010), None);
}

#[test]
fn log_buffer_update_slot_fires() {
    let mut buf = LogBuffer::new(5);
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    buf.set_update_slot(move || {
        h.fetch_add(1, Ordering::SeqCst);
    });

    buf.push_log("one", 0, Some(1));
    buf.push_log("two", 0, Some(2));
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[test]
fn log_entry_timestamp_comparisons() {
    let e = LogEntry::new(1000, 2, "msg");
    assert_eq!(e.timestamp, 1000);
    assert_eq!(e.group, 2);
    assert_eq!(e.message, "msg");

    assert!(e.is_older_than(1001));
    assert!(!e.is_older_than(1000));
    assert!(e.is_younger_than(999));
    assert!(!e.is_younger_than(1000));
    assert!(e.is_younger_or_same(1000));
    assert!(e.is_younger_or_same(999));
    assert!(!e.is_younger_or_same(1001));
}
