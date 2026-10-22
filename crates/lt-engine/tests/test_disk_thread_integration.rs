use std::sync::Arc;
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download;
use lt_storage::disk_thread::DiskThread;

fn metadata_for(payload: &[u8], name: &str) -> TorrentMetadata {
    let piece_hash = sha1_hash(payload);
    let mut file_path = TorrentPath::new();
    file_path.push("payload.bin");
    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece_hash)),
        name: name.to_string(),
        piece_length: payload.len() as u32,
        pieces: piece_hash.to_vec(),
        num_pieces: 1,
        total_size: payload.len() as u64,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: file_path,
            length: payload.len() as u64,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

#[test]
fn verified_piece_is_written_by_disk_thread() {
    let payload = b"async-disk-thread-payload".to_vec();
    let metadata = metadata_for(&payload, "async_disk_thread");
    let mut download = Download::new(metadata);

    let root = std::env::temp_dir().join(format!(
        "rtorrent_engine_disk_thread_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let thread = Arc::new(DiskThread::spawn(8).unwrap());
    download.set_disk_thread(Some(Arc::clone(&thread)));
    download.set_save_directory(&root);
    download.start();

    let verified = download
        .receive_block(0, 0, &payload)
        .expect("piece should verify");
    assert!(verified, "complete piece should report verified");

    // The write may still be queued or may already have completed.
    assert!(
        download.pending_disk_writes() > 0 || download.is_seeder(),
        "piece must be either pending on disk or already complete"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while !download.is_seeder() && Instant::now() < deadline {
        download.poll_disk_completions();
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(download.is_seeder(), "disk write never completed");
    assert_eq!(download.pending_disk_writes(), 0);
    assert_eq!(
        download.uncertain_pieces(),
        vec![0],
        "a completed write is not durable until it is synced"
    );

    let written = std::fs::read(root.join("payload.bin")).unwrap();
    assert_eq!(written, payload);

    download.flush_disk().expect("flush should succeed");
    assert!(
        download.uncertain_pieces().is_empty(),
        "fsync must make the piece trustworthy"
    );
    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_save_directory_completes_without_touching_disk() {
    let payload = b"no-save-directory".to_vec();
    let metadata = metadata_for(&payload, "no_save_dir");
    let mut download = Download::new(metadata);

    // Keep the global worker out of this test's path: no save directory means
    // there is nothing to persist anyway.
    download.set_disk_thread(None);
    download.start();

    assert!(download.receive_block(0, 0, &payload).unwrap());
    assert!(download.is_seeder());
    assert_eq!(download.pending_disk_writes(), 0);
}

#[test]
fn multi_file_pieces_are_written_relative_to_save_directory() {
    // 16 bytes split as 5 + 11 across two files, piece length 8.
    let payload: Vec<u8> = (0..16).collect();
    let piece0 = payload[0..8].to_vec();
    let piece1 = payload[8..16].to_vec();

    let mut pieces = Vec::new();
    pieces.extend_from_slice(&sha1_hash(&piece0));
    pieces.extend_from_slice(&sha1_hash(&piece1));

    let mut file_a = TorrentPath::new();
    file_a.push("dir/a.bin");
    let mut file_b = TorrentPath::new();
    file_b.push("dir/b.bin");

    let metadata = TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&pieces)),
        name: "multi_file_disk".to_string(),
        piece_length: 8,
        pieces,
        num_pieces: 2,
        total_size: 16,
        is_multi_file: true,
        files: vec![
            TorrentFileEntry {
                path: file_a,
                length: 5,
                offset: 0,
            },
            TorrentFileEntry {
                path: file_b,
                length: 11,
                offset: 5,
            },
        ],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    };

    let root = std::env::temp_dir().join(format!(
        "rtorrent_engine_disk_thread_multi_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let thread = Arc::new(DiskThread::spawn(8).unwrap());
    let mut download = Download::new(metadata);
    download.set_disk_thread(Some(Arc::clone(&thread)));
    download.set_save_directory(&root);
    download.start();

    assert!(download.receive_block(0, 0, &piece0).unwrap());
    assert!(download.receive_block(1, 0, &piece1).unwrap());

    let deadline = Instant::now() + Duration::from_secs(5);
    while !download.is_seeder() && Instant::now() < deadline {
        download.poll_disk_completions();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(download.is_seeder());

    let a = std::fs::read(root.join("dir/a.bin")).unwrap();
    let b = std::fs::read(root.join("dir/b.bin")).unwrap();
    assert_eq!(a, payload[0..5]);
    assert_eq!(b, payload[5..16]);

    download.flush_disk().unwrap();
    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

fn many_piece_metadata(piece_length: u32, pieces: usize, name: &str) -> TorrentMetadata {
    let mut hashes = Vec::new();
    let mut payload = Vec::new();
    for index in 0..pieces {
        let piece = vec![index as u8; piece_length as usize];
        hashes.extend_from_slice(&sha1_hash(&piece));
        payload.extend_from_slice(&piece);
    }

    let mut file_path = TorrentPath::new();
    file_path.push("many.bin");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&hashes)),
        name: name.to_string(),
        piece_length,
        pieces: hashes,
        num_pieces: pieces,
        total_size: payload.len() as u64,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: file_path,
            length: payload.len() as u64,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

#[test]
fn backpressured_writes_are_buffered_and_all_complete() {
    let piece_length = 4u32;
    let pieces = 32usize;
    let metadata = many_piece_metadata(piece_length, pieces, "backpressure");
    let mut download = Download::new(metadata);

    let root = std::env::temp_dir().join(format!(
        "rtorrent_engine_disk_backpressure_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    // Capacity one forces receive_block to hit the queue-full path even while
    // the worker is busy. The engine must buffer, not write inline or drop.
    let thread = Arc::new(DiskThread::spawn(1).unwrap());
    download.set_disk_thread(Some(Arc::clone(&thread)));
    download.set_save_directory(&root);
    download.start();

    for index in 0..pieces {
        let data = vec![index as u8; piece_length as usize];
        assert!(
            download.receive_block(index as u32, 0, &data).unwrap(),
            "each block is a complete piece"
        );
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    while !download.is_seeder() && Instant::now() < deadline {
        download.poll_disk_completions();
        std::thread::sleep(Duration::from_millis(5));
    }

    assert!(download.is_seeder(), "backpressured writes never drained");
    assert_eq!(download.pending_disk_writes(), 0);

    let written = std::fs::read(root.join("many.bin")).unwrap();
    assert_eq!(written.len(), pieces * piece_length as usize);
    for index in 0..pieces {
        let start = index * piece_length as usize;
        assert_eq!(
            &written[start..start + piece_length as usize],
            &vec![index as u8; piece_length as usize][..]
        );
    }

    let stats = download.disk_stats().expect("disk stats");
    assert_eq!(stats.writes_completed, pieces as u64);
    assert_eq!(stats.bytes_written, (pieces * piece_length as usize) as u64);

    download.flush_disk().unwrap();
    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn async_hash_results_apply_and_stale_generations_are_discarded() {
    let piece_length = 4u32;
    let pieces = 2usize;
    let metadata = many_piece_metadata(piece_length, pieces, "async_hash");
    let mut download = Download::new(metadata);

    let root =
        std::env::temp_dir().join(format!("rtorrent_engine_disk_hash_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let thread = Arc::new(DiskThread::spawn(8).unwrap());
    download.set_disk_thread(Some(Arc::clone(&thread)));
    download.set_save_directory(&root);
    download.start();

    for index in 0..pieces {
        let data = vec![index as u8; piece_length as usize];
        assert!(download.receive_block(index as u32, 0, &data).unwrap());
    }
    download.flush_disk().unwrap();

    let applied = download
        .submit_hash_check(&(0..pieces as u32).collect::<Vec<_>>())
        .unwrap();
    assert_eq!(applied, pieces);

    let deadline = Instant::now() + Duration::from_secs(5);
    while download.pending_disk_hashes() > 0 && Instant::now() < deadline {
        download.poll_disk_hash_completions();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(download.pending_disk_hashes(), 0);
    assert_eq!(download.completed_chunks(), pieces);
    assert_eq!(download.discarded_hash_results(), 0);

    // Submit another pass and invalidate its generation before the results
    // are harvested. The replies must be ignored.
    let submitted = download.submit_hash_check(&[0]).unwrap();
    assert_eq!(submitted, 1);
    let old_generation = download.disk_generation();
    assert_eq!(download.bump_disk_generation(), old_generation + 1);

    let deadline = Instant::now() + Duration::from_secs(5);
    while download.pending_disk_hashes() > 0 && Instant::now() < deadline {
        download.poll_disk_hash_completions();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(download.pending_disk_hashes(), 0);
    assert_eq!(download.discarded_hash_results(), 1);

    download.flush_disk().unwrap();
    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}
