//! A restart with trustworthy resume data must not rehash the payload.
//!
//! The old restore path called `queue_recheck_if_files_exist()` for every
//! torrent unconditionally, which discarded a validated bitfield and forced a
//! full recheck of the whole session on every start.

use lt_common::Object;
use rt_core::manager::Manager;
use rt_core::session::restore_entries;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(1);

const NAME: &str = "resume-test";
const SIZE: u64 = 16384;

fn torrent_object() -> Object {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from(NAME)).unwrap();
    info.insert_key("piece length", Object::from(SIZE as i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(vec![0u8; 20]))
        .unwrap();
    info.insert_key("length", Object::from(SIZE as i64))
        .unwrap();

    let mut root = Object::create_map();
    root.insert_key("info", info).unwrap();
    root.insert_key("announce", Object::from("http://tracker/announce"))
        .unwrap();
    root
}

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "rtorrent_test_recheck_{tag}_{}_{}",
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn session_with_completed_piece(fixture: &Fixture) -> (Manager, PathBuf) {
    // The payload has to exist with the recorded size and mtime, otherwise
    // resume validation legitimately clears the saved pieces.
    let payload = fixture.dir.join(NAME);
    std::fs::write(&payload, vec![0u8; SIZE as usize]).unwrap();

    let manager = Manager::new();
    {
        let mut session = manager.session_manager().write().unwrap();
        session.set_path(&fixture.dir.to_string_lossy()).unwrap();
        session.start().unwrap();
    }

    let download = manager
        .load_torrent_object(torrent_object(), false)
        .expect("torrent should load");
    {
        let mut guard = download.write().unwrap();
        guard.set_directory(fixture.dir.to_string_lossy().to_string());
        guard.download_mut().chunk_selector_mut().mark_completed(0);
    }

    manager.session_save().expect("session save should succeed");
    (manager, payload)
}

#[test]
fn a_complete_resume_is_loaded_with_its_pieces() {
    let fixture = Fixture::new("trusted");
    let (manager, _payload) = session_with_completed_piece(&fixture);

    let session_path = {
        let session = manager.session_manager().read().unwrap();
        session.path().to_string()
    };
    let entries = {
        let session = manager.session_manager().read().unwrap();
        session.get_restorable_entries().expect("entries")
    };
    assert_eq!(entries.len(), 1);

    let restored = restore_entries(&session_path, &entries, 1)
        .into_iter()
        .next()
        .unwrap()
        .1
        .expect("restore should succeed");

    assert!(restored.resume_loaded, "resume file must be loaded");
    assert!(
        restored.outcome.completed_chunks > 0,
        "a validated bitfield must be kept"
    );

    manager.shutdown().expect("shutdown");
}

#[test]
fn a_missing_resume_is_reported_as_not_loaded() {
    let fixture = Fixture::new("missing");
    let (manager, payload) = session_with_completed_piece(&fixture);

    let session_path = {
        let session = manager.session_manager().read().unwrap();
        session.path().to_string()
    };
    let entries = {
        let session = manager.session_manager().read().unwrap();
        session.get_restorable_entries().expect("entries")
    };

    // Remove only the resume record: the payload is still there, so the
    // restore path has to verify it instead of trusting an empty bitfield.
    for name in std::fs::read_dir(&session_path).unwrap().flatten() {
        if name
            .file_name()
            .to_string_lossy()
            .ends_with(".libtorrent_resume")
        {
            std::fs::remove_file(name.path()).unwrap();
        }
    }
    assert!(payload.is_file());

    let restored = restore_entries(&session_path, &entries, 1)
        .into_iter()
        .next()
        .unwrap()
        .1
        .expect("restore should succeed");

    assert!(!restored.resume_loaded);
    assert_eq!(restored.outcome.completed_chunks, 0);

    manager.shutdown().expect("shutdown");
}
