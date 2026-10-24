//! Phase 5.1 — concurrent writers on one disk worker.
//!
//! A future disk pool may route pieces to different workers, but today every
//! command is serialised through one thread. Before that boundary moves, pin
//! the invariant that matters: concurrent submitters writing disjoint piece
//! ranges must all complete, and reading the file back must reproduce every
//! byte of every piece.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use lt_storage::disk_thread::DiskThread;
use lt_storage::writer::TargetFileInfo;

const PIECE_LEN: usize = 1024;
const PIECES: usize = 32;
const WRITERS: usize = 8;

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_disk_concurrent_{}_{}",
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

/// Piece payload: a distinguishable header plus a byte filled with the index.
fn piece(index: usize) -> Vec<u8> {
    let mut data = vec![index as u8; PIECE_LEN];
    data[..8].copy_from_slice(&(index as u64).to_be_bytes());
    data
}

#[test]
fn concurrent_writers_on_one_worker_produce_the_exact_payload() {
    let root = temp_root();
    let files = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("concurrent.bin"),
        length: (PIECE_LEN * PIECES) as u64,
        offset: 0,
    }]);

    let thread = Arc::new(DiskThread::spawn(256).unwrap());

    let mut writers = Vec::with_capacity(WRITERS);
    for worker in 0..WRITERS {
        let thread = Arc::clone(&thread);
        let root = root.clone();
        let files = Arc::clone(&files);
        writers.push(std::thread::spawn(move || {
            for index in (worker..PIECES).step_by(WRITERS) {
                thread
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

    for writer in writers {
        writer.join().expect("writer thread must not panic");
    }

    // Serialise all pending writes before reading the file back.
    thread.sync_all().unwrap();

    let expected: Vec<u8> = (0..PIECES).flat_map(piece).collect();
    assert_eq!(
        fs::read(root.join("concurrent.bin")).unwrap(),
        expected,
        "all pieces must land at their own offsets"
    );

    for index in 0..PIECES {
        let read = thread
            .read_block(
                &root,
                Arc::clone(&files),
                PIECE_LEN as u32,
                index as u32,
                0,
                PIECE_LEN as u32,
            )
            .unwrap();
        assert_eq!(read, piece(index), "piece {index} read back incorrectly");
    }

    thread.shutdown();
    let _ = fs::remove_dir_all(&root);
}
