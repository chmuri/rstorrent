use std::path::PathBuf;
use std::sync::Arc;

use lt_common::bitfield::Bitfield;
use lt_crypto::sha1::sha1_hash;
use lt_storage::disk_thread::DiskThread;
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
fn hash_pass_syncs_once_and_sees_writes_queued_before_it() {
    let root = temp_root("sync_once");
    let payload: Vec<u8> = (0..24).collect();
    let files = files(payload.len() as u64);

    let thread = Arc::new(DiskThread::spawn(8).unwrap());
    let mut writes = Vec::new();
    for (index, chunk) in payload.chunks(8).enumerate() {
        writes.push(
            thread
                .submit_write(
                    root.clone(),
                    Arc::clone(&files),
                    8,
                    index as u32,
                    chunk.to_vec(),
                )
                .unwrap(),
        );
    }

    let hashes = payload.chunks(8).map(sha1_hash).collect();
    let mut hasher = HashTorrent::new(&root, Arc::clone(&files), 8, 3, 24, hashes);
    hasher.set_disk_thread(Some(Arc::clone(&thread)));

    let result = hasher.hash_all().unwrap();

    // One sync at the start of the pass makes every write queued before it
    // durable, so the reads cannot observe an in-flight block.
    assert_eq!(
        thread.stats().syncs,
        1,
        "one sync per pass, not one per piece"
    );
    assert_eq!(result.checked, 3);
    assert_eq!(result.failed, 0);
    assert!(result.bitfield.is_all_set());

    for write in writes {
        write.wait().unwrap();
    }
    drop(thread);
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

#[test]
fn read_cache_size_is_configurable_and_clamped() {
    let root = temp_root("read_cache");
    let payload: Vec<u8> = (0..16).collect();
    let files = files(payload.len() as u64);
    for (index, chunk) in payload.chunks(8).enumerate() {
        write_piece_to_disk(&root, &files, 8, index as u32, chunk).unwrap();
    }
    let hashes = payload.chunks(8).map(sha1_hash).collect();

    let mut hasher = HashTorrent::new(&root, Arc::clone(&files), 8, 2, 16, hashes);
    assert_eq!(hasher.read_cache_entries(), 64);

    hasher.set_read_cache_entries(32);
    assert_eq!(hasher.read_cache_entries(), 64, "values below 64 clamp up");

    hasher.set_read_cache_entries(256);
    assert_eq!(hasher.read_cache_entries(), 256);
    assert!(hasher.hash_all().unwrap().bitfield.is_all_set());

    hasher.set_read_cache_entries(4096);
    assert_eq!(hasher.read_cache_entries(), 1024, "values above 1024 clamp");

    let _ = std::fs::remove_dir_all(&root);
}
