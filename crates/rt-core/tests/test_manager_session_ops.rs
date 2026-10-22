//! `Manager` session lifecycle coverage: no-op, inactive and active paths.

use std::sync::atomic::{AtomicU64, Ordering};

use lt_common::hash_string::HashString;
use lt_common::Object;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use rt_core::download::Download;
use rt_core::manager::Manager;

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "rstorrent_manager_ops_{}_{}_{}",
        tag,
        std::process::id(),
        id
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn metadata(name: &str) -> TorrentMetadata {
    let mut file_path = lt_common::path::Path::new();
    file_path.push("payload.bin");
    let piece = sha1_hash(b"payload");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece)),
        name: name.to_string(),
        piece_length: 7,
        pieces: piece.to_vec(),
        num_pieces: 1,
        total_size: 7,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: file_path,
            length: 7,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

#[test]
fn manager_accessors_and_throttles_round_trip() {
    let mut manager = Manager::default();

    manager.set_magnet_path("/tmp/magnets");
    assert_eq!(manager.magnet_path(), "/tmp/magnets");

    manager.set_throttle("slow", 10, 20);
    assert_eq!(manager.get_throttle("slow"), Some((10, 20)));
    assert_eq!(manager.get_throttle("missing"), None);
}

#[test]
fn unconfigured_session_operations_are_noops() {
    let manager = Manager::new();
    let download = Download::from_metadata(metadata("noop"));

    assert!(manager.session_save().is_ok());
    assert!(manager.session_commit().is_ok());
    assert!(manager.commit().is_ok());
    assert!(manager.save_download_resume(&download).is_ok());
    assert!(manager.save_download_full_session(&download).is_ok());
    assert_eq!(manager.restore_session().unwrap(), 0);

    manager.shutdown().unwrap();
}

#[test]
fn configured_but_inactive_session_reports_errors() {
    let dir = temp_dir("inactive");
    let manager = Manager::new();
    manager
        .session_manager()
        .write()
        .unwrap()
        .set_path(dir.to_str().unwrap())
        .unwrap();

    let download = Download::from_metadata(metadata("inactive"));

    assert!(manager.save_download_resume(&download).is_err());
    assert!(manager.save_download_full_session(&download).is_err());
    assert_eq!(manager.restore_session().unwrap(), 0);
    // Commit silently skips an inactive session.
    assert!(manager.commit().is_ok());

    manager.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn active_session_saves_resume_and_full_session() {
    let dir = temp_dir("active");
    let manager = Manager::new();
    {
        let mut session = manager.session_manager().write().unwrap();
        session.set_path(dir.to_str().unwrap()).unwrap();
        session.start().unwrap();
    }

    let mut download = Download::from_metadata(metadata("active"));
    // A real download carries the static torrent; provide one so the full
    // session path writes the `.torrent` stream too.
    download.set_bencode(Object::create_map());
    let hash = download.info_hash_hex().to_uppercase();

    assert!(manager.save_download_resume(&download).is_ok());
    assert!(dir.join(format!("{hash}.torrent.rtorrent")).is_file());
    assert!(dir
        .join(format!("{hash}.torrent.libtorrent_resume"))
        .is_file());

    assert!(manager.save_download_full_session(&download).is_ok());
    assert!(dir.join(format!("{hash}.torrent")).is_file());

    manager.shutdown().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn restore_session_tolerates_a_vanished_directory() {
    let dir = temp_dir("vanished");
    let manager = Manager::new();
    {
        let mut session = manager.session_manager().write().unwrap();
        session.set_path(dir.to_str().unwrap()).unwrap();
        session.start().unwrap();
    }

    std::fs::remove_dir_all(&dir).unwrap();

    // Enumeration fails; startup continues without restored downloads.
    assert_eq!(manager.restore_session().unwrap(), 0);

    {
        let mut session = manager.session_manager().write().unwrap();
        let _ = session.stop();
    }
}

#[test]
fn load_torrent_file_reads_a_bencoded_torrent() {
    let dir = temp_dir("load_file");
    let path = dir.join("sample.torrent");

    let mut info = lt_common::Object::create_map();
    info.insert_key("name", lt_common::Object::from("from-file"))
        .unwrap();
    info.insert_key("length", lt_common::Object::from(16i64))
        .unwrap();
    info.insert_key("piece length", lt_common::Object::from(262144i64))
        .unwrap();
    info.insert_key("pieces", lt_common::Object::from(vec![0u8; 20]))
        .unwrap();
    let mut root = lt_common::Object::create_map();
    root.insert_key("info", info).unwrap();
    std::fs::write(&path, lt_common::object::bencode::encode(&root).unwrap()).unwrap();

    let manager = Manager::new();
    let download = manager
        .load_torrent_file(path.to_str().unwrap(), false)
        .expect("torrent file should load");
    assert_eq!(download.read().unwrap().name(), "from-file");
    assert_eq!(manager.download_list().read().unwrap().len(), 1);

    let _ = std::fs::remove_dir_all(dir);
}
