use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use lt_common::bitfield::Bitfield;
use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download;
use lt_storage::resume::{ResumeData, MTIME_DOWNLOADING};

fn torrent_path(value: &str) -> TorrentPath {
    let mut path = TorrentPath::new();
    path.push(value);
    path
}

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_resume_validation_{}_{}",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn mtime_seconds(path: &std::path::Path) -> i64 {
    std::fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn payload_and_hashes() -> (Vec<u8>, Vec<u8>) {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"AAAAAAAA");
    payload.extend_from_slice(b"BBBBBBBB");
    payload.extend_from_slice(b"CCCCCCCC");

    let mut pieces = Vec::new();
    for chunk in payload.chunks(8) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }
    (payload, pieces)
}

/// Two files: a.bin owns piece 0, b.bin owns pieces 1 and 2.
fn metadata() -> TorrentMetadata {
    let (_, pieces) = payload_and_hashes();
    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&pieces)),
        name: "resume_validation".to_string(),
        piece_length: 8,
        pieces,
        num_pieces: 3,
        total_size: 24,
        is_multi_file: true,
        files: vec![
            TorrentFileEntry {
                path: torrent_path("dir/a.bin"),
                length: 8,
                offset: 0,
            },
            TorrentFileEntry {
                path: torrent_path("dir/b.bin"),
                length: 16,
                offset: 8,
            },
        ],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

fn target_files(metadata: &TorrentMetadata) -> Vec<lt_storage::TargetFileInfo> {
    metadata
        .files
        .iter()
        .map(|file| lt_storage::TargetFileInfo {
            path: PathBuf::from(file.path.as_string().trim_start_matches('/')),
            length: file.length,
            offset: file.offset,
        })
        .collect()
}

fn write_all(root: &std::path::Path, metadata: &TorrentMetadata) {
    let (payload, _) = payload_and_hashes();
    let files = target_files(metadata);
    for (index, chunk) in payload.chunks(8).enumerate() {
        lt_storage::write_piece_to_disk(
            root,
            &files,
            8,
            index as u32,
            chunk,
        )
        .unwrap();
    }
}

fn completed_all() -> Bitfield {
    let mut bitfield = Bitfield::new(3);
    bitfield.set_all();
    bitfield
}

#[test]
fn mtime_mismatch_rechecks_only_the_affected_file() {
    let root = temp_root("partial");
    let metadata = metadata();
    write_all(&root, &metadata);

    // Corrupt piece 0, but record a.bin's post-corruption mtime so the resume
    // entry for it is trusted. b.bin is marked for recheck.
    let a_path = root.join("dir/a.bin");
    let mut a = std::fs::read(&a_path).unwrap();
    a[0] ^= 0xFF;
    std::fs::write(&a_path, &a).unwrap();

    let b_path = root.join("dir/b.bin");
    let resume = ResumeData {
        bitfield: Some(completed_all()),
        file_priorities: Vec::new(),
        file_mtimes: vec![
            mtime_seconds(&a_path),
            mtime_seconds(&b_path) - 100,
        ],
        completed_bytes: 24,
    };

    let mut download = Download::new(metadata);
    download.set_disk_thread(None);
    download.set_save_directory(&root);

    let completed = download.apply_resume(&resume).unwrap();

    // Only b.bin's pieces are queued; a.bin was trusted, so its stale bit is
    // preserved even though the bytes on disk are corrupt.
    assert_eq!(completed, 1);
    assert_eq!(download.pending_hash_pieces(), &[1u32, 2u32][..]);
    assert!(!download.is_hash_checked());

    let pending = download.take_pending_hash_pieces();
    let result = download.recheck_pieces(&pending).unwrap();
    assert_eq!(result.checked, 2);
    assert_eq!(result.failed, 0);
    assert_eq!(download.completed_chunks(), 3);
    assert!(download.is_seeder());
    assert!(download.is_hash_checked());
    assert!(!download.is_hash_failed());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn changed_file_drops_bad_chunks_and_sets_hash_failed() {
    let root = temp_root("mismatch");
    let metadata = metadata();
    write_all(&root, &metadata);

    // Corrupt the first byte of piece 2 (inside b.bin).
    let b_path = root.join("dir/b.bin");
    let mut b = std::fs::read(&b_path).unwrap();
    b[8] ^= 0xFF;
    std::fs::write(&b_path, &b).unwrap();

    let a_path = root.join("dir/a.bin");
    let resume = ResumeData {
        bitfield: Some(completed_all()),
        file_priorities: Vec::new(),
        file_mtimes: vec![
            mtime_seconds(&a_path),
            mtime_seconds(&b_path) - 100,
        ],
        completed_bytes: 24,
    };

    let mut download = Download::new(metadata);
    download.set_disk_thread(None);
    download.set_save_directory(&root);

    let completed = download.apply_resume(&resume).unwrap();
    assert_eq!(completed, 1);
    assert_eq!(download.pending_hash_pieces(), &[1u32, 2u32][..]);

    let pending = download.take_pending_hash_pieces();
    download.recheck_pieces(&pending).unwrap();

    // Piece 0 was trusted, piece 1 rehashed successfully, piece 2 failed.
    assert_eq!(download.completed_chunks(), 2);
    assert!(!download.is_seeder());
    assert!(download.is_hash_checked());
    assert!(download.is_hash_failed());
    assert_eq!(download.hash_failed_pieces(), &[2u32][..]);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_file_invalidates_stale_resume_bits() {
    let root = temp_root("stale");
    let metadata = metadata();
    write_all(&root, &metadata);

    // Remove the file that owns pieces 1 and 2. The stale bitfield must not
    // keep those chunks.
    std::fs::remove_file(root.join("dir/b.bin")).unwrap();

    let a_path = root.join("dir/a.bin");
    let resume = ResumeData {
        bitfield: Some(completed_all()),
        file_priorities: Vec::new(),
        file_mtimes: vec![mtime_seconds(&a_path), 12345],
        completed_bytes: 24,
    };

    let mut download = Download::new(metadata);
    download.set_disk_thread(None);
    download.set_save_directory(&root);

    let completed = download.apply_resume(&resume).unwrap();
    assert_eq!(completed, 1);
    assert_eq!(download.pending_hash_pieces(), &[1u32, 2u32][..]);

    let pending = download.take_pending_hash_pieces();
    download.recheck_pieces(&pending).unwrap();

    assert_eq!(download.completed_chunks(), 1);
    assert!(!download.is_seeder());
    assert!(download.is_hash_checked());
    assert!(download.is_hash_failed());
    assert_eq!(download.hash_failed_pieces(), &[1u32, 2u32][..]);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn active_download_persists_recheck_sentinels() {
    let root = temp_root("sentinel");
    let metadata = metadata();
    write_all(&root, &metadata);

    let mut download = Download::new(metadata);
    download.set_disk_thread(None);
    download.set_save_directory(&root);
    download.open().unwrap();
    download.start();
    assert!(download.state().is_active());

    let resume = download.to_resume_data();
    assert_eq!(
        resume.file_mtimes,
        vec![MTIME_DOWNLOADING, MTIME_DOWNLOADING]
    );

    // Once stopped the real mtimes are safe to persist again.
    download.stop();
    let resume = download.to_resume_data();
    assert!(resume.file_mtimes.iter().all(|mtime| *mtime >= 0));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn short_last_piece_round_trips_through_hash_torrent() {
    let root = temp_root("short");
    let mut metadata = metadata();
    // 20 bytes -> pieces of 8, 8 and a short 4 byte tail.
    metadata.files[1].length = 12;
    metadata.total_size = 20;

    let payload = b"AAAAAAAA".to_vec();
    let mut tail = b"BBBBBBBB".to_vec();
    tail.extend_from_slice(b"CCCC");
    let mut pieces = Vec::new();
    pieces.extend_from_slice(&sha1_hash(&payload));
    pieces.extend_from_slice(&sha1_hash(&tail[0..8]));
    pieces.extend_from_slice(&sha1_hash(&tail[8..12]));
    metadata.pieces = pieces;
    metadata.num_pieces = 3;

    write_all(&root, &metadata);

    let mut download = Download::new(metadata);
    download.set_disk_thread(None);
    download.set_save_directory(&root);

    let result = download.recheck().unwrap();
    assert_eq!(result.checked, 3);
    assert_eq!(result.failed, 0);
    assert!(result.bitfield.is_all_set());
    assert!(download.is_seeder());

    let _ = std::fs::remove_dir_all(&root);
}
