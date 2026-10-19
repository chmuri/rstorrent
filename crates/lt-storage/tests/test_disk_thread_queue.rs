//! Integration coverage for the asynchronous disk queue.
//!
//! These tests drive the public `DiskThread` API the same way `lt-engine`
//! does: submit work, observe bounded-queue backpressure, then verify the
//! bytes that reach disk and the order in which replies arrive.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lt_storage::disk_thread::{DiskThread, DiskThreadStats};
use lt_storage::writer::TargetFileInfo;
use lt_storage::StorageError;

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_disk_queue_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn dual_files() -> Arc<Vec<TargetFileInfo>> {
    Arc::new(vec![
        TargetFileInfo {
            path: PathBuf::from("dir/a.bin"),
            length: 5,
            offset: 0,
        },
        TargetFileInfo {
            path: PathBuf::from("dir/b.bin"),
            length: 11,
            offset: 5,
        },
    ])
}

#[test]
fn multi_file_writes_and_reads_preserve_order() {
    let root = temp_root("multi");
    let files = dual_files();
    let thread = DiskThread::spawn(16).unwrap();

    // Two pieces, each crossing the 5-byte file boundary.
    let piece0 = b"abcdefgh".to_vec();
    let piece1 = b"ijklmnop".to_vec();

    let first = thread
        .submit_write(&root, Arc::clone(&files), 8, 0, piece0.clone())
        .unwrap();
    let second = thread
        .submit_write(&root, Arc::clone(&files), 8, 1, piece1.clone())
        .unwrap();

    first.wait().unwrap();
    second.wait().unwrap();

    assert_eq!(fs::read(root.join("dir/a.bin")).unwrap(), b"abcde");
    assert_eq!(fs::read(root.join("dir/b.bin")).unwrap(), b"fghijklmnop");

    // A block inside the first piece still spans the 5-byte file boundary.
    let read = thread
        .read_block(&root, Arc::clone(&files), 8, 0, 3, 5)
        .unwrap();
    assert_eq!(read, b"defgh");

    // Async read replies carry the same bytes and the request correlation id.
    let handle = thread
        .submit_read(&root, files, 8, 1, 1, 4)
        .unwrap();
    assert!(handle.id() > 0);
    assert_eq!(handle.wait().unwrap(), b"jklm");

    let stats: DiskThreadStats = thread.stats();
    assert_eq!(stats.writes_completed, 2);
    assert_eq!(stats.bytes_written, 16);
    assert!(stats.reads_completed >= 2);

    thread.shutdown();
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn padding_files_are_virtual_zeroes_and_never_opened() {
    let root = temp_root("padding");
    let files = Arc::new(vec![
        TargetFileInfo {
            path: PathBuf::from("a.bin"),
            length: 2,
            offset: 0,
        },
        TargetFileInfo {
            path: PathBuf::from("_____padding_file_0"),
            length: 2,
            offset: 2,
        },
        TargetFileInfo {
            path: PathBuf::from("b.bin"),
            length: 2,
            offset: 4,
        },
    ]);
    let thread = DiskThread::spawn(4).unwrap();

    thread
        .submit_write(&root, Arc::clone(&files), 6, 0, vec![1, 2, 9, 9, 5, 6])
        .unwrap()
        .wait()
        .unwrap();

    assert_eq!(fs::read(root.join("a.bin")).unwrap(), vec![1, 2]);
    assert_eq!(fs::read(root.join("b.bin")).unwrap(), vec![5, 6]);
    assert!(
        !root.join("_____padding_file_0").exists(),
        "padding files must not consume a descriptor or a path"
    );

    let read = thread.read_block(&root, files, 6, 0, 0, 6).unwrap();
    assert_eq!(read, vec![1, 2, 0, 0, 5, 6]);

    thread.shutdown();
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn open_creates_non_empty_files_and_close_flushes() {
    let root = temp_root("open");
    let files = dual_files();
    let thread = DiskThread::spawn(4).unwrap();

    thread.open(&root, Arc::clone(&files)).unwrap();
    assert!(root.join("dir/a.bin").exists());
    assert!(root.join("dir/b.bin").exists());

    thread.close().unwrap();
    // Closing is idempotent at the command layer.
    thread.close().unwrap();

    thread.shutdown();
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn partial_block_write_does_not_complete_neighbouring_bytes() {
    let root = temp_root("partial");
    let files = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("payload.bin"),
        length: 16,
        offset: 0,
    }]);
    let thread = DiskThread::spawn(4).unwrap();

    thread
        .submit_write(&root, Arc::clone(&files), 16, 0, vec![0x11; 16])
        .unwrap()
        .wait()
        .unwrap();

    // Write only the middle 8 bytes of the piece.
    thread
        .submit_write_block(&root, Arc::clone(&files), 16, 0, 4, b"ABCDEFGH".to_vec())
        .unwrap()
        .wait()
        .unwrap();

    let content = fs::read(root.join("payload.bin")).unwrap();
    assert_eq!(&content[0..4], &[0x11; 4]);
    assert_eq!(&content[4..12], b"ABCDEFGH");
    assert_eq!(&content[12..16], &[0x11; 4]);

    thread.shutdown();
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn shutdown_drains_every_accepted_write() {
    let root = temp_root("shutdown");
    let files = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("drain.bin"),
        length: 128,
        offset: 0,
    }]);
    let thread = DiskThread::spawn(16).unwrap();

    for index in 0..16u32 {
        thread
            .submit_write_block(
                &root,
                Arc::clone(&files),
                128,
                0,
                index * 8,
                vec![index as u8; 8],
            )
            .expect("the worker must accept or apply backpressure");
    }

    // Handles are intentionally dropped; shutdown alone must drain the queue.
    thread.shutdown();

    let content = fs::read(root.join("drain.bin")).unwrap();
    assert_eq!(content.len(), 128);
    for index in 0..16usize {
        assert_eq!(
            &content[index * 8..(index + 1) * 8],
            &[index as u8; 8],
            "block {index} was not drained before shutdown"
        );
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn bounded_queue_reports_full_and_recovers() {
    let root = temp_root("bounded");
    let files = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("bounded.bin"),
        length: 65536,
        offset: 0,
    }]);
    let thread = DiskThread::spawn(1).unwrap();

    let mut accepted = 0;
    let mut rejected = 0;
    for _ in 0..512 {
        match thread.submit_write_block(
            &root,
            Arc::clone(&files),
            65536,
            0,
            0,
            vec![0x42; 65536],
        ) {
            Ok(_handle) => {
                // Do not wait: dropping the handle keeps the queue full while
                // the worker is still writing earlier commands.
                accepted += 1;
            }
            Err(StorageError::DiskQueueFull) => {
                rejected += 1;
                break;
            }
            Err(err) => panic!("unexpected error: {err}"),
        }
    }

    assert!(accepted > 0);
    assert!(rejected > 0, "the bounded queue should report backpressure");

    // Draining the worker must make room for more commands.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match thread.submit_write_block(&root, Arc::clone(&files), 65536, 0, 0, vec![0x24; 16]) {
            Ok(handle) => {
                handle.wait().unwrap();
                break;
            }
            Err(StorageError::DiskQueueFull) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(StorageError::DiskQueueFull) => panic!("queue never drained"),
            Err(err) => panic!("unexpected error: {err}"),
        }
    }

    thread.shutdown();
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn hash_replies_carry_the_requested_generation() {
    let root = temp_root("hash");
    let files = Arc::new(vec![TargetFileInfo {
        path: PathBuf::from("hash.bin"),
        length: 8,
        offset: 0,
    }]);
    let thread = DiskThread::spawn(4).unwrap();

    thread
        .submit_write(&root, Arc::clone(&files), 8, 0, b"abcdefgh".to_vec())
        .unwrap()
        .wait()
        .unwrap();

    let old = thread
        .submit_hash(&root, Arc::clone(&files), 8, 0, 8, 1)
        .unwrap();
    let new = thread
        .submit_hash(&root, Arc::clone(&files), 8, 0, 8, 2)
        .unwrap();

    let old = old.wait().unwrap();
    let new = new.wait().unwrap();
    assert_eq!(old.generation, 1);
    assert_eq!(new.generation, 2);
    assert_eq!(old.hash, new.hash);
    assert_eq!(old.hash, lt_crypto::sha1::sha1_hash(b"abcdefgh"));

    let stats = thread.stats();
    assert_eq!(stats.hashes_completed, 2);

    thread.shutdown();
    let _ = fs::remove_dir_all(&root);
}
