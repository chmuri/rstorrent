//! Pins the session-writer counters and the semantics they describe:
//! one `fsync` per staged file only when `use_fsyncdisk` is on, one directory
//! sync per entry, and no `.torrent` rewrite for `save_resume`.

use rt_core::session::stats::{snapshot, SessionWriteStats};
use rt_core::session::{SessionCommitEntry, SessionManager};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// The counters are process-global, so tests in this binary run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());
static COUNTER: AtomicU64 = AtomicU64::new(1);

const TORRENT: &[u8] = b"td";
const RTORRENT: &[u8] = b"rtorrent-data";
const LIBTORRENT: &[u8] = b"libtorrent-data";

struct Fixture {
    dir: std::path::PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "rtorrent_test_stats_{tag}_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn manager(&self, use_fsyncdisk: bool) -> SessionManager {
        let mut manager = SessionManager::new();
        manager.set_use_fsyncdisk(use_fsyncdisk).unwrap();
        manager.set_path(self.dir.to_str().unwrap()).unwrap();
        manager.start().unwrap();
        manager
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn entry(hash: &str, torrent: Option<&[u8]>) -> SessionCommitEntry {
    SessionCommitEntry::new(
        hash,
        torrent.map(|stream| stream.to_vec()),
        RTORRENT.to_vec(),
        LIBTORRENT.to_vec(),
    )
}

fn delta(before: SessionWriteStats) -> SessionWriteStats {
    snapshot().since(before)
}

#[test]
fn commit_counts_entries_streams_and_syncs() {
    let _serial = SERIAL.lock().unwrap();
    let fixture = Fixture::new("commit");
    let manager = fixture.manager(true);

    let before = snapshot();
    manager
        .commit(&[
            entry("AA00000000000000000000000000000000000000", Some(TORRENT)),
            entry("BB00000000000000000000000000000000000000", None),
            entry("CC00000000000000000000000000000000000000", None),
        ])
        .unwrap();
    let stats = delta(before);

    assert_eq!(stats.commit_calls, 1);
    assert_eq!(stats.entries, 3);
    assert_eq!(
        stats.torrent_streams, 1,
        "only the explicit stream is written"
    );
    assert_eq!(stats.resume_streams, 3);
    assert_eq!(stats.rtorrent_streams, 3);

    // 3 files for the entry with a torrent stream, 2 for the others.
    assert_eq!(stats.staged_files, 7);
    // One fsync per staged file.
    assert_eq!(stats.fdatasync_calls, 7);
    // One directory sync per published entry.
    assert_eq!(stats.dir_sync_calls, 3);

    let per_entry = RTORRENT.len() as u64 + LIBTORRENT.len() as u64;
    assert_eq!(stats.bytes, TORRENT.len() as u64 + per_entry * 3);
}

#[test]
fn save_resume_never_rewrites_the_torrent_stream() {
    let _serial = SERIAL.lock().unwrap();
    let fixture = Fixture::new("resume");
    let manager = fixture.manager(false);

    let before = snapshot();
    manager
        .save_resume(&entry(
            "DD00000000000000000000000000000000000000",
            Some(TORRENT),
        ))
        .unwrap();
    let stats = delta(before);

    assert_eq!(stats.entries, 1);
    assert_eq!(stats.torrent_streams, 0);
    assert_eq!(stats.staged_files, 2);
}

#[test]
fn disabled_fsync_writes_no_syncs() {
    let _serial = SERIAL.lock().unwrap();
    let fixture = Fixture::new("nosync");
    let manager = fixture.manager(false);

    let before = snapshot();
    manager
        .commit(&[entry("EE00000000000000000000000000000000000000", None)])
        .unwrap();
    let stats = delta(before);

    assert_eq!(stats.entries, 1);
    assert_eq!(stats.staged_files, 2);
    assert_eq!(stats.fdatasync_calls, 0);
    assert_eq!(stats.dir_sync_calls, 0);
}
