//! The session writer is what shutdown blocks on, so its contract is pinned
//! here: every entry is published, a pre-set abort writes nothing, and the
//! torrent-only path never rewrites an existing file.

use rt_core::session::{stats, SessionCommitEntry, SessionManager};
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// The session writer counters are process-global, so tests that assert on
/// them run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    dir: std::path::PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "rtorrent_test_writer_{tag}_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn manager(&self) -> SessionManager {
        let mut manager = SessionManager::new();
        manager.set_use_fsyncdisk(true).unwrap();
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

fn hash(index: usize) -> String {
    format!("{:040X}", index)
}

fn entry(index: usize, torrent: Option<&[u8]>) -> SessionCommitEntry {
    SessionCommitEntry::new(
        hash(index),
        torrent.map(|stream| stream.to_vec()),
        b"rtorrent".to_vec(),
        b"resume".to_vec(),
    )
}

#[test]
fn parallel_commit_publishes_every_entry() {
    let _serial = SERIAL.lock().unwrap();
    let fixture = Fixture::new("parallel");
    let writer = fixture.manager().writer();
    let entries: Vec<_> = (0..100).map(|index| entry(index, None)).collect();

    let saved = writer.commit_parallel(&entries, None, 8).unwrap();

    assert_eq!(saved, 100);
    for index in 0..100 {
        // `torrent_stream: None` means resume-only, so the static anchor is
        // deliberately not rewritten (mirrors `save_resume`).
        assert!(fixture
            .dir
            .join(format!("{}.torrent.libtorrent_resume", hash(index)))
            .is_file());
        assert!(fixture
            .dir
            .join(format!("{}.torrent.rtorrent", hash(index)))
            .is_file());
    }
}

#[test]
fn abort_before_the_first_entry_writes_nothing() {
    let _serial = SERIAL.lock().unwrap();
    let fixture = Fixture::new("abort");
    let manager = fixture.manager();
    let writer = manager.writer();
    let entries: Vec<_> = (0..10).map(|index| entry(index, None)).collect();
    let abort = AtomicBool::new(true);

    let saved = writer.commit_parallel(&entries, Some(&abort), 4).unwrap();

    assert_eq!(saved, 0);
    assert_eq!(
        fs::read_dir(&fixture.dir).unwrap().count(),
        1,
        "only the session lock file should exist"
    );
}

#[test]
fn torrent_only_write_never_rewrites_an_existing_file() {
    let _serial = SERIAL.lock().unwrap();
    let fixture = Fixture::new("torrent-only");
    let writer = fixture.manager().writer();
    let entry = entry(7, Some(b"td-original"));

    writer.save_torrent_if_missing(&entry).unwrap();
    let path = fixture.dir.join(format!("{}.torrent", hash(7)));
    let first = fs::metadata(&path).unwrap();

    // A different stream must not replace the file that is already there.
    let mut changed = entry.clone();
    changed.torrent_stream = Some(b"td-different".to_vec());
    writer.save_torrent_if_missing(&changed).unwrap();

    assert_eq!(fs::read(&path).unwrap(), b"td-original");
    assert_eq!(
        fs::metadata(&path).unwrap().modified().unwrap(),
        first.modified().unwrap()
    );
}

#[test]
fn torrent_only_write_counts_as_a_single_torrent_stream() {
    let _serial = SERIAL.lock().unwrap();
    let fixture = Fixture::new("torrent-only-stats");
    let writer = fixture.manager().writer();
    let before = stats::snapshot();

    writer
        .save_torrent_if_missing(&entry(9, Some(b"td")))
        .unwrap();

    let delta = stats::snapshot().since(before);
    assert_eq!(delta.entries, 1);
    assert_eq!(delta.torrent_streams, 1);
    assert_eq!(delta.resume_streams, 0);
    assert_eq!(delta.staged_files, 1);
}
