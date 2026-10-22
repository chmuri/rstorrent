//! `Control` session-save paths that do not require the full daemon loop.

use std::sync::atomic::{AtomicU64, Ordering};

use lt_common::hash_string::HashString;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use rstorrent::control::Control;
use rt_core::download::Download;
use std::sync::{Arc, RwLock};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "rstorrent_control_save_{}_{}_{}",
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
    let piece = sha1_hash(b"control-save");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece)),
        name: name.to_string(),
        piece_length: 12,
        pieces: piece.to_vec(),
        num_pieces: 1,
        total_size: 12,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: file_path,
            length: 12,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

#[test]
fn save_session_is_a_noop_without_an_active_session() {
    let control = Control::new();
    assert!(control.save_session().is_ok());
}

#[test]
fn save_session_writes_sidecar_and_resume_for_active_session() {
    let session_dir = temp_dir("session");
    let data_dir = temp_dir("data");

    let control = Control::new();
    {
        let mut session = control.session_manager.lock().unwrap();
        session.set_path(session_dir.to_str().unwrap()).unwrap();
        session.start().unwrap();
    }

    let mut download = Download::from_metadata(metadata("saved"));
    download.set_directory(data_dir.to_str().unwrap());
    let hash = download.info_hash_hex().to_uppercase();
    let download = Arc::new(RwLock::new(download));
    assert!(control
        .download_list
        .write()
        .unwrap()
        .insert(Arc::clone(&download)));

    control.save_session().expect("session save should succeed");

    assert!(session_dir
        .join(format!("{hash}.torrent.rtorrent"))
        .is_file());
    assert!(session_dir
        .join(format!("{hash}.torrent.libtorrent_resume"))
        .is_file());

    control.session_manager.lock().unwrap().stop().unwrap();
    let _ = std::fs::remove_dir_all(session_dir);
    let _ = std::fs::remove_dir_all(data_dir);
}
