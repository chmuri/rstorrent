use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download;

fn torrent_path(value: &str) -> TorrentPath {
    let mut path = TorrentPath::new();
    path.push(value);
    path
}

#[test]
fn recheck_rebuilds_bitfield_from_disk() {
    let payload: Vec<u8> = (0..16).collect();
    let piece0 = payload[0..8].to_vec();
    let piece1 = payload[8..16].to_vec();
    let mut pieces = Vec::new();
    pieces.extend_from_slice(&sha1_hash(&piece0));
    pieces.extend_from_slice(&sha1_hash(&piece1));

    let metadata = TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&pieces)),
        name: "recheck_torrent".to_string(),
        piece_length: 8,
        pieces,
        num_pieces: 2,
        total_size: 16,
        is_multi_file: true,
        files: vec![
            TorrentFileEntry {
                path: torrent_path("dir/a.bin"),
                length: 5,
                offset: 0,
            },
            TorrentFileEntry {
                path: torrent_path("dir/b.bin"),
                length: 11,
                offset: 5,
            },
        ],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    };

    let root = std::env::temp_dir().join(format!("rtorrent_engine_recheck_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let target_files: Vec<lt_storage::TargetFileInfo> = metadata
        .files
        .iter()
        .map(|f| lt_storage::TargetFileInfo {
            path: std::path::PathBuf::from(f.path.as_string().trim_start_matches('/')),
            length: f.length,
            offset: f.offset,
        })
        .collect();

    lt_storage::write_piece_to_disk(&root, &target_files, 8, 0, &piece0).unwrap();
    lt_storage::write_piece_to_disk(&root, &target_files, 8, 1, &piece1).unwrap();

    let mut download = Download::new(metadata);
    download.set_disk_thread(None);
    download.set_save_directory(&root);

    let result = download.recheck().expect("recheck should succeed");
    assert_eq!(result.checked, 2);
    assert_eq!(result.failed, 0);
    assert!(result.bitfield.is_all_set());
    assert!(download.is_seeder());

    // Corrupt the second piece: the recheck must drop it.
    let b_path = root.join("dir/b.bin");
    let mut b = std::fs::read(&b_path).unwrap();
    b[0] ^= 0xFF;
    std::fs::write(&b_path, b).unwrap();

    let result = download.recheck().expect("second recheck should succeed");
    assert_eq!(result.checked, 2);
    assert_eq!(result.failed, 1);
    assert_eq!(download.completed_chunks(), 1);
    assert!(!download.is_seeder());

    let _ = std::fs::remove_dir_all(&root);
}

fn three_piece_metadata() -> TorrentMetadata {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"AAAAAAAA");
    payload.extend_from_slice(b"BBBBBBBB");
    payload.extend_from_slice(b"CCCCCCCC");

    let mut pieces = Vec::new();
    for chunk in payload.chunks(8) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&pieces)),
        name: "recheck_partial".to_string(),
        piece_length: 8,
        pieces,
        num_pieces: 3,
        total_size: 24,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: torrent_path("data.bin"),
            length: 24,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

fn three_piece_download(root: &std::path::Path) -> Download {
    let metadata = three_piece_metadata();
    let target_files: Vec<lt_storage::TargetFileInfo> = metadata
        .files
        .iter()
        .map(|file| lt_storage::TargetFileInfo {
            path: std::path::PathBuf::from(file.path.as_string().trim_start_matches('/')),
            length: file.length,
            offset: file.offset,
        })
        .collect();

    let bytes = b"AAAAAAAABBBBBBBBCCCCCCCC";
    for (index, chunk) in bytes.chunks(8).enumerate() {
        lt_storage::write_piece_to_disk(root, &target_files, 8, index as u32, chunk).unwrap();
    }

    let mut download = Download::new(metadata);
    download.set_disk_thread(None);
    download.set_save_directory(root);
    download
}

#[test]
fn partial_recheck_records_failed_pieces_and_keeps_trusted_bits() {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_engine_partial_failure_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut download = three_piece_download(&root);

    download.recheck().unwrap();
    assert_eq!(download.completed_chunks(), 3);

    // Corrupt piece 1 only, then recheck just that piece.
    let path = root.join("data.bin");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[9] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();

    let result = download.recheck_pieces(&[1]).unwrap();

    assert_eq!(result.checked, 1);
    assert_eq!(result.failed, 1);
    assert_eq!(result.failed_pieces, vec![1]);
    assert_eq!(download.completed_chunks(), 2);
    assert!(download.chunk_selector().completed().get(0));
    assert!(!download.chunk_selector().completed().get(1));
    assert!(download.chunk_selector().completed().get(2));
    assert!(download.is_hash_checked());
    assert!(download.is_hash_failed());
    assert_eq!(download.hash_failed_pieces(), &[1u32][..]);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelled_recheck_does_not_mark_bits_and_keeps_pending_work() {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_engine_cancel_full_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut download = three_piece_download(&root);

    let cancel = Arc::new(AtomicBool::new(true));
    let result = download.recheck_with_cancel(cancel).unwrap();

    assert!(result.cancelled);
    assert_eq!(result.checked, 0);
    assert_eq!(result.unchecked_pieces, vec![0, 1, 2]);
    assert_eq!(download.completed_chunks(), 0);
    assert_eq!(download.pending_hash_pieces(), &[0u32, 1, 2][..]);
    assert!(!download.is_hash_checked());
    assert!(!download.is_hash_failed());

    // The deferred pieces can be run to completion, which then clears the
    // pending gate.
    let pending = download.pending_hash_pieces().to_vec();
    download.recheck_pieces(&pending).unwrap();
    assert_eq!(download.completed_chunks(), 3);
    assert!(download.is_hash_checked());
    assert!(download.pending_hash_pieces().is_empty());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelled_partial_recheck_preserves_unselected_bits() {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_engine_cancel_partial_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut download = three_piece_download(&root);
    download.recheck().unwrap();

    let cancel = Arc::new(AtomicBool::new(true));
    let result = download.recheck_pieces_with_cancel(&[1], cancel).unwrap();

    assert!(result.cancelled);
    assert_eq!(result.unchecked_pieces, vec![1]);
    assert!(download.chunk_selector().completed().get(0));
    assert!(!download.chunk_selector().completed().get(1));
    assert!(download.chunk_selector().completed().get(2));
    assert_eq!(download.pending_hash_pieces(), &[1u32][..]);
    assert!(!download.is_hash_checked());

    let _ = std::fs::remove_dir_all(&root);
}
