use std::path::PathBuf;
use std::sync::Arc;

use lt_common::bitfield::Bitfield;
use lt_crypto::sha1::sha1_hash;
use lt_storage::hash_torrent::HashTorrent;
use lt_storage::writer::{write_piece_to_disk, TargetFileInfo};

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_hash_recheck_{}_{}",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn files(payload_len: u64) -> Arc<Vec<TargetFileInfo>> {
    Arc::new(vec![
        // A zero-length file must never contribute bytes to a piece.
        TargetFileInfo {
            path: PathBuf::from("empty.bin"),
            length: 0,
            offset: 0,
        },
        TargetFileInfo {
            path: PathBuf::from("data.bin"),
            length: payload_len,
            offset: 0,
        },
    ])
}

#[test]
fn full_recheck_handles_short_final_piece_and_zero_length_file() {
    let root = temp_root("short");
    let payload: Vec<u8> = (0..20).collect();
    let files = files(payload.len() as u64);

    for (index, chunk) in payload.chunks(8).enumerate() {
        write_piece_to_disk(&root, &files, 8, index as u32, chunk).unwrap();
    }

    let hashes = payload.chunks(8).map(sha1_hash).collect();
    let hasher = HashTorrent::new(&root, Arc::clone(&files), 8, 3, 20, hashes);
    assert_eq!(hasher.piece_size(2), 4);

    let result = hasher.hash_all().unwrap();
    assert_eq!(result.checked, 3);
    assert_eq!(result.failed, 0);
    assert!(result.failed_pieces.is_empty());
    assert!(result.bitfield.is_all_set());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn partial_recheck_preserves_unselected_bits_and_reports_bad_pieces() {
    let root = temp_root("partial");
    let payload: Vec<u8> = (0..24).collect();
    let files = files(payload.len() as u64);

    for (index, chunk) in payload.chunks(8).enumerate() {
        write_piece_to_disk(&root, &files, 8, index as u32, chunk).unwrap();
    }

    // Corrupt piece 2; pieces 0 and 1 stay valid.
    let path = root.join("data.bin");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[16] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();

    let mut initial = Bitfield::new(3);
    initial.set(0);
    initial.set(2);

    let hashes = payload.chunks(8).map(sha1_hash).collect();
    let hasher = HashTorrent::new(&root, files, 8, 3, 24, hashes);
    let result = hasher.hash_selected(&initial, &[1, 2]).unwrap();

    assert_eq!(result.checked, 2);
    assert_eq!(result.failed, 1);
    assert_eq!(result.failed_pieces, vec![2u32]);
    assert!(result.bitfield.get(0)); // untouched
    assert!(result.bitfield.get(1)); // rechecked and good
    assert!(!result.bitfield.get(2)); // rechecked and bad

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancellation_preserves_existing_target_bits() {
    let root = temp_root("cancel");
    let payload: Vec<u8> = (0..16).collect();
    let files = files(payload.len() as u64);

    for (index, chunk) in payload.chunks(8).enumerate() {
        write_piece_to_disk(&root, &files, 8, index as u32, chunk).unwrap();
    }

    let mut initial = Bitfield::new(2);
    initial.set(0);

    let hashes = payload.chunks(8).map(sha1_hash).collect();
    let hasher = HashTorrent::new(&root, files, 8, 2, 16, hashes);
    hasher.cancel();

    let result = hasher.hash_all().unwrap();
    assert!(result.cancelled);
    assert_eq!(result.checked, 0);

    // hash_all starts from an empty target, but a selected pass on the old
    // bitfield must keep the trusted bit when cancelled.
    hasher
        .cancel_handle()
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let result = hasher.hash_selected(&initial, &[1]).unwrap();
    assert!(result.cancelled);
    assert!(result.bitfield.get(0));
    assert!(!result.bitfield.get(1));

    let _ = std::fs::remove_dir_all(&root);
}
