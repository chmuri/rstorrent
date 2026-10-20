use std::fs;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicU64, Ordering};

use lt_common::Object;
use rt_core::manager::Manager;
use rt_core::session::{DownloadStorer, SessionCommitEntry, SessionManager};

static COUNTER: AtomicU64 = AtomicU64::new(1);

const HASH_A: &str = "0123456789ABCDEF0123456789ABCDEF01234567";
const HASH_B: &str = "FEDCBA9876543210FEDCBA9876543210FEDCBA98";

struct TempSession {
    dir: std::path::PathBuf,
}

impl TempSession {
    fn new() -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "rtorrent_session_manager_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self) -> &str {
        self.dir.to_str().unwrap()
    }
}

impl Drop for TempSession {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn torrent_object() -> Object {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from("session-manager-test"))
        .unwrap();
    info.insert_key("piece length", Object::from(16384i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(vec![0u8; 20]))
        .unwrap();
    info.insert_key("length", Object::from(16384i64)).unwrap();

    let mut root = Object::create_map();
    root.insert_key("info", info).unwrap();
    root.insert_key("announce", Object::from("http://tracker/announce"))
        .unwrap();
    root
}

#[test]
fn lock_contention_reports_holder_and_drop_releases() {
    let temp = TempSession::new();

    let mut first = SessionManager::new();
    first.set_path(temp.path()).unwrap();
    first.start().unwrap();
    assert!(first.is_locked());

    let lock_path = first.lock_path().expect("lock path");
    assert!(std::path::Path::new(&lock_path).exists());

    let mut second = SessionManager::new();
    second.set_path(temp.path()).unwrap();
    let err = second.start().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::AddrInUse);
    assert!(
        err.to_string().contains("held by"),
        "unexpected lock error: {}",
        err
    );
    assert!(!second.is_active());

    // Dropping the owner releases the lock even without an explicit stop.
    drop(first);
    assert!(!std::path::Path::new(&lock_path).exists());

    second.start().unwrap();
    second.stop().unwrap();
    assert!(!std::path::Path::new(&lock_path).exists());
}

#[test]
fn commit_publishes_all_stream_files() {
    let temp = TempSession::new();
    let mut manager = SessionManager::new();
    manager.set_path(temp.path()).unwrap();
    manager.start().unwrap();

    let entries = vec![SessionCommitEntry::new(
        HASH_A,
        Some(b"torrent-data".to_vec()),
        b"rtorrent-data".to_vec(),
        b"libtorrent-data".to_vec(),
    )];

    assert_eq!(manager.commit(&entries).unwrap(), 1);

    assert_eq!(
        fs::read_to_string(temp.dir.join(format!("{}.torrent", HASH_A))).unwrap(),
        "torrent-data"
    );
    assert_eq!(
        fs::read_to_string(temp.dir.join(format!("{}.torrent.rtorrent", HASH_A))).unwrap(),
        "rtorrent-data"
    );
    assert_eq!(
        fs::read_to_string(
            temp.dir
                .join(format!("{}.torrent.libtorrent_resume", HASH_A))
        )
        .unwrap(),
        "libtorrent-data"
    );

    assert!(!temp.dir.join(format!("{}.torrent.new", HASH_A)).exists());
}

#[test]
fn commit_continues_past_invalid_entries() {
    let temp = TempSession::new();
    let mut manager = SessionManager::new();
    manager.set_path(temp.path()).unwrap();
    manager.start().unwrap();

    let entries = vec![
        SessionCommitEntry::new(HASH_A, Some(b"a".to_vec()), b"a".to_vec(), b"a".to_vec()),
        // Invalid hash must not be turned into a path traversal.
        SessionCommitEntry::new("../escape", None, b"b".to_vec(), b"b".to_vec()),
        SessionCommitEntry::new(HASH_B, Some(b"c".to_vec()), b"c".to_vec(), b"c".to_vec()),
    ];

    let err = manager.commit(&entries).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);

    // The valid entries on both sides of the invalid one were still committed.
    assert!(temp.dir.join(format!("{}.torrent", HASH_A)).is_file());
    assert!(temp.dir.join(format!("{}.torrent", HASH_B)).is_file());
    assert!(!temp.dir.join("..").join("escape.torrent").exists());
}

#[test]
fn valid_entries_skip_malformed_and_symlinked_files() {
    let temp = TempSession::new();
    let mut manager = SessionManager::new();
    manager.set_path(temp.path()).unwrap();
    manager.start().unwrap();

    let entries = vec![SessionCommitEntry::new(
        HASH_A,
        Some(b"a".to_vec()),
        b"a".to_vec(),
        b"a".to_vec(),
    )];
    manager.commit(&entries).unwrap();

    fs::write(temp.dir.join("not-a-session.torrent"), b"x").unwrap();

    let outside = temp.dir.join("outside");
    fs::write(&outside, b"x").unwrap();
    std::os::unix::fs::symlink(&outside, temp.dir.join(format!("{}.torrent", HASH_B))).unwrap();

    let valid = manager.get_valid_entries().unwrap();
    assert_eq!(valid.len(), 1);
    assert_eq!(valid[0].s_name, format!("{}.torrent", HASH_A));
}

#[test]
fn manager_commit_and_restore_roundtrip() {
    let temp = TempSession::new();

    let manager = Manager::new();
    {
        let mut session = manager.session_manager().write().unwrap();
        session.set_path(temp.path()).unwrap();
        session.start().unwrap();
    }

    let download = manager
        .load_torrent_object(torrent_object(), false)
        .expect("torrent should load");
    {
        let mut download = download.write().unwrap();
        download.set_directory(temp.path().to_string());
    }

    manager.commit().expect("commit should succeed");
    manager.shutdown().expect("shutdown should succeed");

    let restored_manager = Manager::new();
    {
        let mut session = restored_manager.session_manager().write().unwrap();
        session.set_path(temp.path()).unwrap();
        session.start().unwrap();
    }

    assert_eq!(restored_manager.restore_session().unwrap(), 1);
    assert_eq!(restored_manager.download_list().read().unwrap().len(), 1);

    let restored = restored_manager.download_list().read().unwrap().all()[0].clone();
    assert_eq!(restored.read().unwrap().directory(), temp.path());

    restored_manager
        .shutdown()
        .expect("shutdown should succeed");
}

#[test]
fn failed_stage_keeps_previous_session_files_intact() {
    let temp = TempSession::new();
    let path = temp.dir.join(format!("{}.torrent", HASH_A));
    let path = path.to_str().unwrap();

    DownloadStorer::save_and_move_streams(
        path,
        false,
        Some(b"old-torrent"),
        b"old-rtorrent",
        b"old-libtorrent",
    )
    .unwrap();

    // Make one staging destination impossible to replace. The commit must fail
    // before publishing anything, leaving the previous generation untouched.
    fs::create_dir_all(format!("{}.rtorrent.new", path)).unwrap();

    DownloadStorer::save_and_move_streams(
        path,
        false,
        Some(b"new-torrent"),
        b"new-rtorrent",
        b"new-libtorrent",
    )
    .unwrap_err();

    assert_eq!(fs::read_to_string(path).unwrap(), "old-torrent");
    assert_eq!(
        fs::read_to_string(format!("{}.rtorrent", path)).unwrap(),
        "old-rtorrent"
    );
    assert_eq!(
        fs::read_to_string(format!("{}.libtorrent_resume", path)).unwrap(),
        "old-libtorrent"
    );
}

#[test]
fn download_storer_rejects_invalid_hash() {
    let temp = TempSession::new();
    let path = format!("{}/", temp.path());

    let err = DownloadStorer::build_path(&path, "../escape").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
}
