use std::path::PathBuf;
use std::sync::Arc;

use lt_common::bitfield::Bitfield;
use lt_crypto::sha1::sha1_hash;
use lt_storage::hash_torrent::HashTorrent;
use lt_storage::{write_piece_to_disk, TargetFileInfo};

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_hash_torrent_it_{}_{}",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn single_file(length: u64) -> Arc<Vec<TargetFileInfo>> {
    Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("data.bin"),
        length,
        offset: 0,
    }])
}

fn payload_and_hashes(piece_length: usize, length: usize) -> (Vec<u8>, Vec<[u8; 20]>) {
    let payload: Vec<u8> = (0..length).map(|value| value as u8).collect();
    let hashes = payload.chunks(piece_length).map(sha1_hash).collect();
    (payload, hashes)
}

#[test]
fn partial_recheck_keeps_trusted_bits_and_clears_bad_piece() {
    let root = temp_root("partial_bad");
    let files = single_file(24);
    let (payload, hashes) = payload_and_hashes(8, 24);
    for (index, chunk) in payload.chunks(8).enumerate() {
        write_piece_to_disk(&root, &files, 8, index as u32, chunk).unwrap();
    }

    // Corrupt the middle piece only.
    let path = root.join("data.bin");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[12] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();

    let mut initial = Bitfield::new(3);
    initial.set_all();

    let ht = HashTorrent::new(&root, Arc::clone(&files), 8, 3, 24, hashes);
    let result = ht.hash_selected(&initial, &[1]).unwrap();

    assert_eq!(result.checked, 1);
    assert_eq!(result.failed, 1);
    assert_eq!(result.failed_pieces, vec![1]);
    assert!(result.unchecked_pieces.is_empty());
    assert!(!result.cancelled);

    // Pieces outside the recheck range keep their trusted state.
    assert!(result.bitfield.get(0));
    assert!(!result.bitfield.get(1));
    assert!(result.bitfield.get(2));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelled_full_pass_clears_selected_bits_and_reports_unchecked() {
    let root = temp_root("cancel_full");
    let files = single_file(16);
    let (payload, hashes) = payload_and_hashes(8, 16);
    for (index, chunk) in payload.chunks(8).enumerate() {
        write_piece_to_disk(&root, &files, 8, index as u32, chunk).unwrap();
    }

    let mut initial = Bitfield::new(2);
    initial.set_all();

    let ht = HashTorrent::new(&root, files, 8, 2, 16, hashes);
    ht.cancel();
    let result = ht.hash_selected(&initial, &[0, 1]).unwrap();

    assert!(result.cancelled);
    assert_eq!(result.checked, 0);
    assert_eq!(result.failed, 0);
    assert_eq!(result.unchecked_pieces, vec![0, 1]);
    // A cancelled pass must not leave the examined range marked complete.
    assert!(!result.bitfield.get(0));
    assert!(!result.bitfield.get(1));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelled_partial_pass_preserves_unselected_bits() {
    let root = temp_root("cancel_partial");
    let files = single_file(24);
    let (payload, hashes) = payload_and_hashes(8, 24);
    for (index, chunk) in payload.chunks(8).enumerate() {
        write_piece_to_disk(&root, &files, 8, index as u32, chunk).unwrap();
    }

    let mut initial = Bitfield::new(3);
    initial.set_all();

    let ht = HashTorrent::new(&root, files, 8, 3, 24, hashes);
    ht.cancel();
    let result = ht.hash_selected(&initial, &[1, 2]).unwrap();

    assert!(result.cancelled);
    assert_eq!(result.unchecked_pieces, vec![1, 2]);
    assert!(result.bitfield.get(0));
    assert!(!result.bitfield.get(1));
    assert!(!result.bitfield.get(2));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_file_records_every_piece_as_failed() {
    let root = temp_root("missing");
    let files = single_file(16);
    let (_, hashes) = payload_and_hashes(8, 16);
    // Deliberately do not create data.bin.

    let ht = HashTorrent::new(&root, files, 8, 2, 16, hashes);
    let result = ht.hash_all().unwrap();

    assert_eq!(result.checked, 2);
    assert_eq!(result.failed, 2);
    assert_eq!(result.failed_pieces, vec![0, 1]);
    assert!(!result.bitfield.get(0));
    assert!(!result.bitfield.get(1));

    let _ = std::fs::remove_dir_all(&root);
}
