//! Generic custom variables (`d.custom`/`d.custom.set`) accept arbitrary keys,
//! unlike the five numbered label slots. They are written to the `.rtorrent`
//! session sidecar as a nested `custom` dict so values set by an external
//! client survive a daemon restart.

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use rt_core::download::Download;
use rt_core::manager::Manager;
use rt_core::session::restore_rtorrent_state;

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn metadata() -> TorrentMetadata {
    let mut file_path = TorrentPath::new();
    file_path.push("payload.bin");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(b"generic-custom-persistence")),
        name: "generic-custom-persistence".to_string(),
        piece_length: 16384,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 16384,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: file_path,
            length: 16384,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
        v2: None,
    }
}

fn temp_root() -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "rstorrent_generic_custom_{}_{}",
        std::process::id(),
        id
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn generic_custom_variables_survive_the_session_sidecar() {
    let root = temp_root();
    let base = root.join("entry");

    let mut download = Download::from_metadata(metadata());
    download.custom_set("label", "movies");
    download.custom_set("tracker_tag", "abc");
    // Numbered slots use a separate mechanism and must keep working alongside.
    download.set_custom_field(1, "tv-sonarr");
    assert_eq!(download.custom_get("label"), Some("movies"));
    assert_eq!(download.custom_get("missing"), None);

    let entry = Manager::build_commit_entry(&download).expect("entry should serialize");
    fs::write(
        format!("{}.rtorrent", base.display()),
        entry.rtorrent_stream,
    )
    .unwrap();

    let mut restored = Download::from_metadata(metadata());
    restore_rtorrent_state(base.to_str().unwrap(), &mut restored);

    assert_eq!(restored.custom_get("label"), Some("movies"));
    assert_eq!(restored.custom_get("tracker_tag"), Some("abc"));
    assert_eq!(restored.custom_get("missing"), None);
    assert_eq!(restored.custom_map().len(), 2);
    assert_eq!(restored.custom_field(1), "tv-sonarr");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn overwriting_a_generic_custom_key_keeps_the_latest_value() {
    let root = temp_root();
    let base = root.join("entry");

    let mut download = Download::from_metadata(metadata());
    download.custom_set("label", "old");
    download.custom_set("label", "new");
    assert_eq!(download.custom_map().len(), 1);

    let entry = Manager::build_commit_entry(&download).expect("entry should serialize");
    fs::write(
        format!("{}.rtorrent", base.display()),
        entry.rtorrent_stream,
    )
    .unwrap();

    let mut restored = Download::from_metadata(metadata());
    restore_rtorrent_state(base.to_str().unwrap(), &mut restored);
    assert_eq!(restored.custom_get("label"), Some("new"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn empty_generic_custom_map_is_not_written_to_the_sidecar() {
    let root = temp_root();
    let base = root.join("entry");

    let download = Download::from_metadata(metadata());
    let entry = Manager::build_commit_entry(&download).expect("entry should serialize");
    fs::write(
        format!("{}.rtorrent", base.display()),
        entry.rtorrent_stream,
    )
    .unwrap();

    let object = lt_common::object::bencode::decode(
        &fs::read(format!("{}.rtorrent", base.display())).unwrap(),
    )
    .unwrap();

    assert!(
        object.get_key_map("custom").is_err(),
        "an empty generic custom map should not be persisted"
    );

    let _ = fs::remove_dir_all(&root);
}
