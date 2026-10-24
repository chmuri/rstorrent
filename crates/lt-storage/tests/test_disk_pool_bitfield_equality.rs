//! Pre-pool gate: worker count must not change the completed bitfield.
//!
//! This is the parameterized data-safety test required before any disk-pool
//! code lands. It exercises the exact partition rule a pool would use — piece
//! `i` goes to worker `i % workers` — and derives the completed bitfield from
//! the bytes that reached disk. `disk_threads = 4` must produce byte-identical
//! data and the same full bitfield as `disk_threads = 1`.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use lt_common::bitfield::Bitfield;
use lt_crypto::sha1::sha1_hash;
use lt_storage::disk_thread::DiskThread;
use lt_storage::writer::TargetFileInfo;

const PIECE_LEN: usize = 256;
const PIECES: usize = 32;

fn temp_root(workers: usize) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_pool_bitfield_{}_{}_{}",
        workers,
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn piece(index: usize) -> Vec<u8> {
    let mut data = vec![index as u8; PIECE_LEN];
    data[..8].copy_from_slice(&(index as u64).to_be_bytes());
    data
}

fn expected_bitfield() -> Vec<u8> {
    let mut bitfield = Bitfield::new(PIECES);
    bitfield.set_all();
    bitfield.clear_tail();
    bitfield.as_bytes().to_vec()
}

/// Writes every piece through `workers` disk threads, then returns the file
/// bytes and the completed bitfield derived by re-hashing each piece on disk.
fn write_with_workers(workers: usize) -> (Vec<u8>, Vec<u8>) {
    assert!(workers > 0);

    let root = temp_root(workers);
    let files = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("payload.bin"),
        length: (PIECE_LEN * PIECES) as u64,
        offset: 0,
    }]);

    let threads: Vec<Arc<DiskThread>> = (0..workers)
        .map(|_| Arc::new(DiskThread::spawn(64).unwrap()))
        .collect();

    let mut submitters = Vec::with_capacity(workers);
    for worker in 0..workers {
        let threads = threads.clone();
        let root = root.clone();
        let files = Arc::clone(&files);
        submitters.push(std::thread::spawn(move || {
            for index in (worker..PIECES).step_by(workers) {
                threads[index % workers]
                    .submit_write_block(
                        &root,
                        Arc::clone(&files),
                        PIECE_LEN as u32,
                        index as u32,
                        0,
                        piece(index),
                    )
                    .expect("write should be accepted")
                    .wait()
                    .unwrap_or_else(|err| panic!("piece {index} failed: {err}"));
            }
        }));
    }

    for submitter in submitters {
        submitter.join().expect("submitter must not panic");
    }
    for thread in &threads {
        thread.sync_all().unwrap();
    }

    let bytes = fs::read(root.join("payload.bin")).unwrap();
    let mut completed = Bitfield::new(PIECES);
    for index in 0..PIECES {
        let start = index * PIECE_LEN;
        let end = start + PIECE_LEN;
        if bytes
            .get(start..end)
            .map(|data| sha1_hash(data) == sha1_hash(&piece(index)))
            .unwrap_or(false)
        {
            completed.set(index);
        }
    }

    for thread in &threads {
        thread.shutdown();
    }
    let _ = fs::remove_dir_all(&root);

    (bytes, completed.as_bytes().to_vec())
}

#[test]
fn bitfield_is_identical_for_one_and_four_disk_workers() {
    let (one_bytes, one_bitfield) = write_with_workers(1);
    let (four_bytes, four_bitfield) = write_with_workers(4);

    assert_eq!(
        four_bytes, one_bytes,
        "worker count must not change the bytes that reach disk"
    );
    assert_eq!(
        four_bitfield, one_bitfield,
        "worker count must not change the completed bitfield"
    );
    assert_eq!(one_bitfield, expected_bitfield());
    assert!(Bitfield::from_bytes(&one_bitfield, PIECES).is_all_set());
}
