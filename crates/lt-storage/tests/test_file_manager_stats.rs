//! Public-surface coverage for the disk worker's live FileManager counters.
//!
//! These are the values that `system.files.opened_counter`,
//! `system.files.closed_counter`, `system.files.failed_counter` and the
//! fadvise statistics should report.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

// `posix_fadvise` is Linux-only; on other platforms the hint is skipped and
// counted as a failure instead of a success.
const FADVISE_SUCCESS: u64 = if cfg!(target_os = "linux") { 1 } else { 0 };
const FADVISE_FAILURES: u64 = if cfg!(target_os = "linux") { 0 } else { 1 };

use lt_storage::disk_thread::{
    global_disk_thread, global_file_manager_stats, shutdown_global_disk_thread, DiskThread,
};
use lt_storage::file_manager::FileManagerStats;
use lt_storage::writer::TargetFileInfo;

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_file_manager_stats_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn disk_thread_exposes_real_file_manager_counters() {
    let root = temp_root("counters");
    let files = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("payload.bin"),
        length: 8,
        offset: 0,
    }]);

    let thread = DiskThread::spawn(8).unwrap();

    thread
        .submit_write(&root, Arc::clone(&files), 8, 0, b"abcdefgh".to_vec())
        .unwrap()
        .wait()
        .unwrap();

    let stats: FileManagerStats = thread.file_stats();
    assert_eq!(stats.open_files, 1);
    assert_eq!(stats.in_flight, 0);
    assert_eq!(stats.files_opened, 1);
    assert_eq!(stats.files_closed, 0);
    assert_eq!(stats.files_failed, 0);
    assert_eq!(stats.fadvise, FADVISE_SUCCESS);
    assert_eq!(stats.fadvise_failures, FADVISE_FAILURES);
    assert_eq!(thread.file_manager_stats(), stats);
    assert_eq!(thread.files_opened_counter(), 1);
    assert_eq!(thread.files_closed_counter(), 0);
    assert_eq!(thread.files_failed_counter(), 0);
    assert_eq!(thread.files_advised_counter(), FADVISE_SUCCESS);

    // A read of a path that does not exist fails inside `FileManager::open`
    // and must increment the failure counter without changing the open gauge.
    let missing = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("missing-dir/missing.bin"),
        length: 4,
        offset: 0,
    }]);
    assert!(thread.read_block(&root, missing, 4, 0, 0, 4).is_err());

    let stats = thread.file_stats();
    assert_eq!(stats.files_failed, 1);
    assert_eq!(stats.open_files, 1);
    assert_eq!(thread.files_failed_counter(), 1);

    // Closing cached handles updates the close counter and drains the gauge.
    thread.close().unwrap();
    let stats = thread.file_stats();
    assert_eq!(stats.open_files, 0);
    assert_eq!(stats.files_closed, 1);
    assert_eq!(thread.files_closed_counter(), 1);

    // Shutdown is idempotent with respect to the counters: nothing was open.
    thread.shutdown();
    let stats = thread.file_stats();
    assert_eq!(stats.open_files, 0);
    assert_eq!(stats.files_closed, 1);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn global_worker_exposes_the_same_counter_snapshot() {
    let thread = global_disk_thread().expect("global disk worker should spawn");
    let from_thread = thread.file_stats();
    let from_global = global_file_manager_stats().expect("global stats available after spawn");

    assert_eq!(from_global, from_thread);
    assert_eq!(from_global.open_files, thread.file_stats().open_files);
    assert_eq!(from_global.files_opened, thread.files_opened_counter());

    shutdown_global_disk_thread();
}
