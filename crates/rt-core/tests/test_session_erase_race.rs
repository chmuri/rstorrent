//! Regression tests for the erase ↔ session-commit race.
//!
//! A periodic checkpoint serializes the whole download list up front and
//! publishes entries for minutes on large sessions. An erase that lands inside
//! that window unlinks the session files, but the in-flight commit used to
//! publish its pre-erase snapshot afterwards, resurrecting `.rtorrent` /
//! `.libtorrent_resume` (and, for a full save, the `.torrent` anchor itself).
//! Erase tombstones in the writer must make every late write a no-op.

use rt_core::session::{SessionCommitEntry, SessionManager};
use std::fs;

struct Fixture {
    dir: std::path::PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "rtorrent_test_erase_race_{tag}_{}_{}",
            std::process::id(),
            tag
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

    fn files_for(&self, hash: &str) -> Vec<String> {
        let base = format!("{hash}.torrent");
        ["", ".rtorrent", ".libtorrent_resume", ".meta", ".new"]
            .iter()
            .map(|suffix| format!("{base}{suffix}"))
            .filter(|name| self.dir.join(name).exists())
            .collect()
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
        format!("rtorrent-{index}").into_bytes(),
        format!("resume-{index}").into_bytes(),
    )
}

#[test]
fn erase_then_late_full_commit_publishes_nothing() {
    let fixture = Fixture::new("full");
    let manager = fixture.manager();
    let writer = manager.writer();
    let entries = vec![entry(1, Some(b"torrent-payload"))];

    writer.commit_parallel(&entries, None, 1).unwrap();
    assert!(!fixture.files_for(&hash(1)).is_empty());

    manager.erase_entry(&hash(1)).unwrap();
    assert!(fixture.files_for(&hash(1)).is_empty());

    // The stale in-flight pass publishes its pre-erase snapshot.
    writer.commit_parallel(&entries, None, 1).unwrap();
    assert!(
        fixture.files_for(&hash(1)).is_empty(),
        "late full commit resurrected an erased entry: {:?}",
        fixture.files_for(&hash(1))
    );
}

#[test]
fn erase_then_late_resume_commit_publishes_nothing() {
    let fixture = Fixture::new("resume");
    let manager = fixture.manager();
    let writer = manager.writer();
    // Mirror the checkpoint: the anchor exists, the pass is resume-only.
    let anchor = entry(2, Some(b"torrent-payload"));
    writer.save_torrent_if_missing(&anchor).unwrap();
    let entries = vec![entry(2, None)];
    writer.commit_parallel(&entries, None, 1).unwrap();
    assert!(!fixture.files_for(&hash(2)).is_empty());

    manager.erase_entry(&hash(2)).unwrap();
    writer.commit_parallel(&entries, None, 1).unwrap();

    assert!(
        fixture.files_for(&hash(2)).is_empty(),
        "late resume commit resurrected an erased entry: {:?}",
        fixture.files_for(&hash(2))
    );
}

#[test]
fn erase_before_any_write_blocks_publication() {
    let fixture = Fixture::new("pre");
    let manager = fixture.manager();
    let writer = manager.writer();

    manager.erase_entry(&hash(3)).unwrap();
    writer
        .commit_parallel(&[entry(3, Some(b"td"))], None, 1)
        .unwrap();

    assert!(fixture.files_for(&hash(3)).is_empty());
}

#[test]
fn batch_erase_blocks_late_writes_for_erased_hashes_only() {
    let fixture = Fixture::new("batch");
    let manager = fixture.manager();
    let writer = manager.writer();
    let entries: Vec<_> = (0..10).map(|i| entry(i, Some(b"td"))).collect();
    writer.commit_parallel(&entries, None, 4).unwrap();

    let erased: Vec<String> = (0..10).step_by(2).map(hash).collect();
    manager.erase_entries_batch(&erased).unwrap();

    // One stale pass still carries every pre-erase entry.
    writer.commit_parallel(&entries, None, 4).unwrap();

    for i in 0..10 {
        if i % 2 == 0 {
            assert!(
                fixture.files_for(&hash(i)).is_empty(),
                "erased hash {i} resurrected"
            );
        } else {
            assert!(
                !fixture.files_for(&hash(i)).is_empty(),
                "live hash {i} must keep its session files"
            );
        }
    }
}

#[test]
fn late_save_cleans_a_resurrected_torrent_anchor() {
    let fixture = Fixture::new("anchor");
    let manager = fixture.manager();
    let writer = manager.writer();

    manager.erase_entry(&hash(4)).unwrap();
    // Simulate `save_torrent_if_missing` winning the race just before the
    // erase: the anchor is back on disk while the tombstone is already set.
    let anchor = fixture.dir.join(format!("{}.torrent", hash(4)));
    fs::write(&anchor, b"td").unwrap();

    writer
        .save_streams(&hash(4), None, b"rtorrent", b"resume")
        .unwrap();

    assert!(
        fixture.files_for(&hash(4)).is_empty(),
        "stale .torrent anchor survived the late save"
    );
}

#[test]
fn reinsert_clears_the_tombstone() {
    let fixture = Fixture::new("reinsert");
    let manager = fixture.manager();
    let writer = manager.writer();
    let entries = vec![entry(5, Some(b"td"))];

    writer.commit_parallel(&entries, None, 1).unwrap();
    manager.erase_entry(&hash(5)).unwrap();
    // `event.download.inserted` for the re-added torrent.
    manager.clear_erased(&hash(5));
    writer.commit_parallel(&entries, None, 1).unwrap();

    assert!(
        !fixture.files_for(&hash(5)).is_empty(),
        "re-added torrent must persist normally"
    );
}

#[test]
fn concurrent_erase_during_parallel_commit_leaves_no_orphans() {
    let fixture = Fixture::new("concurrent");
    let manager = fixture.manager();
    let writer = manager.writer();
    let entries: Vec<_> = (0..1200).map(|i| entry(i, None)).collect();
    let erased: Vec<String> = (0..1200).step_by(2).map(hash).collect();

    let commit_writer = writer.clone();
    std::thread::scope(|scope| {
        let handle = scope.spawn(move || {
            commit_writer.commit_parallel(&entries, None, 4).unwrap();
        });
        // Land the erase while the pass is publishing. Even if the timing
        // misses the window on a fast machine, the assertions below hold for
        // every interleaving — that is the tombstone contract.
        std::thread::sleep(std::time::Duration::from_millis(30));
        manager.erase_entries_batch(&erased).unwrap();
        handle.join().unwrap();
    });

    for i in 0..1200 {
        if i % 2 == 0 {
            assert!(
                fixture.files_for(&hash(i)).is_empty(),
                "erased hash {i} left orphan files: {:?}",
                fixture.files_for(&hash(i))
            );
        } else {
            assert!(
                !fixture.files_for(&hash(i)).is_empty(),
                "live hash {i} lost its session files"
            );
        }
    }
}
