use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::{Download, DownloadState};

fn torrent_path(value: &str) -> TorrentPath {
    let mut path = TorrentPath::new();
    path.push(value);
    path
}

fn sample_metadata() -> TorrentMetadata {
    let payload: Vec<u8> = (0..24).collect();
    let mut pieces = Vec::new();
    for chunk in payload.chunks(8) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&pieces)),
        name: "test_seeding".to_string(),
        piece_length: 8,
        pieces,
        num_pieces: 3,
        total_size: 24,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: torrent_path("test_data.bin"),
            length: 24,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

#[test]
fn start_with_existing_complete_file_triggers_hash_check_and_seeds() {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_test_existing_seeding_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let file_path = root.join("test_data.bin");
    let payload: Vec<u8> = (0..24).collect();
    std::fs::write(&file_path, &payload).unwrap();

    let mut download = Download::new(sample_metadata());
    download.set_save_directory(&root);
    download.set_disk_thread(None);
    assert!(download.queue_recheck_if_files_exist());

    assert!(download.has_existing_files());
    assert!(!download.is_hash_checked());

    download.open().unwrap();
    assert!(download.try_start().is_ok());

    assert!(download.is_hash_checked());
    assert!(!download.is_hash_failed());
    assert!(download.is_done());
    assert!(download.is_seeder());
    assert_eq!(*download.state(), DownloadState::Seeding);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn start_with_partial_file_rechecks_and_downloads_remainder() {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_test_existing_partial_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    // Only 16 out of 24 bytes (first 2 pieces match, 3rd missing)
    let file_path = root.join("test_data.bin");
    let mut payload: Vec<u8> = (0..24).collect();
    payload[20] ^= 0xFF; // Corrupt piece 2
    std::fs::write(&file_path, &payload).unwrap();

    let mut download = Download::new(sample_metadata());
    download.set_save_directory(&root);
    download.set_disk_thread(None);
    assert!(download.queue_recheck_if_files_exist());

    assert!(download.has_existing_files());

    download.open().unwrap();
    assert!(download.try_start().is_ok());

    assert!(download.is_hash_checked());
    assert!(!download.is_done());
    assert!(!download.is_seeder());
    assert_eq!(*download.state(), DownloadState::Downloading);
    assert_eq!(download.chunk_selector().completed().size_set(), 2);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn start_without_existing_files_marks_checked_and_downloads() {
    let root =
        std::env::temp_dir().join(format!("rtorrent_test_no_existing_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut download = Download::new(sample_metadata());
    download.set_save_directory(&root);
    download.set_disk_thread(None);

    assert!(!download.has_existing_files());

    download.open().unwrap();
    assert!(download.try_start().is_ok());

    assert!(download.is_hash_checked());
    assert!(!download.is_done());
    assert_eq!(*download.state(), DownloadState::Downloading);
    assert_eq!(download.chunk_selector().completed().size_set(), 0);

    let _ = std::fs::remove_dir_all(&root);
}
