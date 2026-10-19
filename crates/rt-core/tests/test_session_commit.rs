use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use rt_core::session::{DownloadStorer, SessionManager};

static COUNTER: AtomicU64 = AtomicU64::new(1);

const ENTRY_NAME: &str = "0123456789ABCDEF0123456789ABCDEF01234567.torrent";

struct TempCommitFixture {
    session_dir: std::path::PathBuf,
}

impl TempCommitFixture {
    fn new() -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let session_dir = std::env::temp_dir().join(format!("rtorrent_test_commit_{}_{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&session_dir);
        fs::create_dir_all(&session_dir).unwrap();

        Self { session_dir }
    }
}

impl Drop for TempCommitFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.session_dir);
    }
}

fn commit_and_verify(use_fsyncdisk: bool) {
    let fixture = TempCommitFixture::new();
    let path = fixture.session_dir.join(ENTRY_NAME);
    let path_str = path.to_str().unwrap();

    DownloadStorer::save_and_move_streams(
        path_str,
        use_fsyncdisk,
        Some(b"torrent-data"),
        b"rtorrent-data",
        b"libtorrent-data",
    )
    .unwrap();

    assert_eq!("torrent-data", fs::read_to_string(&path).unwrap());
    assert_eq!(
        "rtorrent-data",
        fs::read_to_string(fixture.session_dir.join(format!("{}.rtorrent", ENTRY_NAME))).unwrap()
    );
    assert_eq!(
        "libtorrent-data",
        fs::read_to_string(fixture.session_dir.join(format!("{}.libtorrent_resume", ENTRY_NAME))).unwrap()
    );

    // .new temporary file must no longer exist
    assert!(!fixture.session_dir.join(format!("{}.new", ENTRY_NAME)).exists());
}

#[test]
fn test_commit_publishes_all_three_files() {
    commit_and_verify(false);
}

#[test]
fn test_commit_with_fsync_publishes_all_three_files() {
    commit_and_verify(true);
}

#[test]
fn test_session_manager_lifecycle_and_lock() {
    let fixture = TempCommitFixture::new();
    let mut manager = SessionManager::new();

    assert!(!manager.is_used());
    manager.set_path(fixture.session_dir.to_str().unwrap()).unwrap();
    assert!(manager.is_used());
    assert!(manager.path().ends_with('/'));

    manager.start().unwrap();
    assert!(manager.is_active());

    // Lockfile should exist and be locked
    let lock_path = fixture.session_dir.join("rtorrent.lock");
    assert!(lock_path.exists());

    // Trying to start a second manager on same path should fail with lock conflict
    let mut second_manager = SessionManager::new();
    second_manager.set_path(fixture.session_dir.to_str().unwrap()).unwrap();
    assert!(second_manager.start().is_err());

    // Save streams through manager
    manager
        .save_streams(
            "0123456789ABCDEF0123456789ABCDEF01234567",
            Some(b"t-data"),
            b"rt-data",
            b"lt-data",
        )
        .unwrap();

    let torrent_file = fixture.session_dir.join(ENTRY_NAME);
    assert_eq!("t-data", fs::read_to_string(&torrent_file).unwrap());

    // Stop manager
    manager.stop().unwrap();
    assert!(!manager.is_active());

    // Second manager can now start
    second_manager.start().unwrap();
    assert!(second_manager.is_active());
    second_manager.stop().unwrap();
}
