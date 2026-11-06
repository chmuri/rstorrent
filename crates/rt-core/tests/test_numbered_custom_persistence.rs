//! Numbered custom fields (`d.custom1`..`d.custom5`) are rTorrent's label
//! slots. They are written to the `.rtorrent` session sidecar as
//! `custom1`..`custom5` keys so a label set by an external client survives a
//! daemon restart.

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
        info_hash: HashString::new(sha1_hash(b"numbered-custom-persistence")),
        name: "numbered-custom-persistence".to_string(),
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
    }
}

fn temp_root() -> std::path::PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "rstorrent_numbered_custom_{}_{}",
        std::process::id(),
        id
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn numbered_custom_fields_survive_the_session_sidecar() {
    let root = temp_root();
    let base = root.join("entry");

    let mut download = Download::from_metadata(metadata());
    download.set_custom_field(1, "tv-sonarr");
    download.set_custom_field(5, "fifth");
    assert_eq!(download.custom_field(1), "tv-sonarr");
    assert_eq!(download.custom_field(2), "");

    let entry = Manager::build_commit_entry(&download).expect("entry should serialize");
    fs::write(
        format!("{}.rtorrent", base.display()),
        entry.rtorrent_stream,
    )
    .unwrap();

    let mut restored = Download::from_metadata(metadata());
    restore_rtorrent_state(base.to_str().unwrap(), &mut restored);

    assert_eq!(restored.custom_field(1), "tv-sonarr");
    assert_eq!(restored.custom_field(2), "");
    assert_eq!(restored.custom_field(5), "fifth");

    // Out-of-range indexes are harmless on both sides of the round trip.
    assert_eq!(restored.custom_field(0), "");
    assert_eq!(restored.custom_field(6), "");
    restored.set_custom_field(6, "ignored");
    assert_eq!(restored.custom_field(6), "");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn empty_numbered_custom_fields_are_not_written_to_the_sidecar() {
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

    for index in 1..=5 {
        assert!(
            object.get_key_string(format!("custom{index}")).is_err(),
            "empty custom{index} should not be persisted"
        );
    }

    let _ = fs::remove_dir_all(&root);
}
