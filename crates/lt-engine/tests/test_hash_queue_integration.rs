use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download;
use lt_storage::disk_thread::DiskThread;
use lt_storage::writer::{write_piece_to_disk, TargetFileInfo};

fn torrent_path(value: &str) -> TorrentPath {
    let mut path = TorrentPath::new();
    path.push(value);
    path
}

fn metadata(piece_length: u32, pieces_count: usize, name: &str) -> TorrentMetadata {
    let payload: Vec<u8> = (0..piece_length as usize * pieces_count)
        .map(|value| value as u8)
        .collect();
    let mut pieces = Vec::new();
    for chunk in payload.chunks(piece_length as usize) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }

    TorrentMetadata {
        info_hash: HashString::new([0x42u8; 20]),
        name: name.to_string(),
        piece_length,
        pieces,
        num_pieces: pieces_count,
        total_size: payload.len() as u64,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: torrent_path("data.bin"),
            length: payload.len() as u64,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

fn temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rtorrent_hash_queue_{}_{}",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn target_files(metadata: &TorrentMetadata) -> Vec<TargetFileInfo> {
    metadata
        .files
        .iter()
        .map(|file| TargetFileInfo {
            path: PathBuf::from(file.path.as_string().trim_start_matches('/')),
            length: file.length,
            offset: file.offset,
        })
        .collect()
}

fn write_pieces(metadata: &TorrentMetadata, root: &PathBuf) {
    let files = target_files(metadata);
    let piece_length = metadata.piece_length;
    let payload: Vec<u8> = (0..metadata.total_size as usize)
        .map(|value| value as u8)
        .collect();

    for (index, chunk) in payload.chunks(piece_length as usize).enumerate() {
        write_piece_to_disk(root, &files, piece_length, index as u32, chunk).unwrap();
    }
}

fn wait_for<F>(download: &mut Download, mut done: F)
where
    F: FnMut(&Download) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(download) && Instant::now() < deadline {
        download.poll_disk_hash_completions();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(done(download), "hash queue did not drain before timeout");
}

#[test]
fn recheck_uses_priority_queue_and_clears_it() {
    let piece_length = 8u32;
    let pieces = 3usize;
    let metadata = metadata(piece_length, pieces, "recheck_queue");
    let root = temp_root("recheck");
    write_pieces(&metadata, &root);

    let thread = Arc::new(DiskThread::spawn(8).unwrap());
    let mut download = Download::new(metadata);
    download.set_disk_thread(Some(Arc::clone(&thread)));
    download.set_save_directory(&root);
    download.open().unwrap();
    download.try_start().unwrap();

    let result = download.recheck().expect("recheck");
    assert_eq!(result.checked, pieces as u32);
    assert_eq!(result.failed, 0);

    let stats = download.hash_queue_stats();
    assert_eq!(stats.enqueued, pieces as u64);
    assert_eq!(stats.completed, pieces as u64);
    assert_eq!(stats.stale_results, 0);
    assert_eq!(download.hash_queue_size(), 0);
    assert_eq!(download.pending_disk_hashes(), 0);
    assert!(download.is_seeder());

    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn recheck_without_disk_thread_uses_hash_torrent_fallback() {
    let metadata = metadata(8, 2, "recheck_fallback");
    let root = temp_root("fallback");
    write_pieces(&metadata, &root);

    let mut download = Download::new(metadata);
    download.set_disk_thread(None);
    download.set_save_directory(&root);
    download.open().unwrap();
    download.try_start().unwrap();

    let result = download.recheck().expect("recheck");
    assert_eq!(result.checked, 2);
    assert_eq!(result.failed, 0);
    let stats = download.hash_queue_stats();
    assert_eq!(stats.enqueued, 2);
    assert_eq!(stats.completed, 2);
    assert_eq!(download.hash_queue_size(), 0);
    assert!(download.is_seeder());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn duplicate_async_submissions_are_deduplicated() {
    let piece_length = 4u32;
    let pieces = 2usize;
    let metadata = metadata(piece_length, pieces, "dedup_queue");
    let root = temp_root("dedup");
    write_pieces(&metadata, &root);

    let thread = Arc::new(DiskThread::spawn(8).unwrap());
    let mut download = Download::new(metadata);
    download.set_disk_thread(Some(Arc::clone(&thread)));
    download.set_save_directory(&root);
    download.open().unwrap();
    download.try_start().unwrap();

    let submitted = download.submit_hash_check(&[0, 0, 1, 1]).unwrap();
    assert_eq!(submitted, 2);
    // The first pair is still in flight (results are not polled yet), so the
    // second call must not schedule duplicate hashes.
    assert_eq!(download.submit_hash_check(&[0, 1]).unwrap(), 0);
    assert!(download.hash_queue_stats().duplicates >= 2);

    wait_for(&mut download, |download| {
        download.pending_disk_hashes() == 0
    });

    assert_eq!(download.completed_chunks(), pieces);
    let stats = download.hash_queue_stats();
    assert_eq!(stats.enqueued, 2);
    assert_eq!(stats.completed, 2);
    assert_eq!(stats.stale_results, 0);
    assert_eq!(download.hash_queue_size(), 0);

    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn removing_in_flight_hash_discards_the_result() {
    let metadata = metadata(4, 1, "remove_queue");
    let root = temp_root("remove");
    write_pieces(&metadata, &root);

    let thread = Arc::new(DiskThread::spawn(4).unwrap());
    let mut download = Download::new(metadata);
    download.set_disk_thread(Some(Arc::clone(&thread)));
    download.set_save_directory(&root);
    download.open().unwrap();
    download.try_start().unwrap();

    assert_eq!(download.submit_hash_check(&[0]).unwrap(), 1);
    assert!(download.remove_hash_check(0));
    assert_eq!(download.hash_queue_size(), 1); // still executing on the worker

    wait_for(&mut download, |download| {
        download.pending_disk_hashes() == 0
    });

    assert_eq!(download.completed_chunks(), 0);
    assert_eq!(download.discarded_hash_results(), 1);
    let stats = download.hash_queue_stats();
    assert_eq!(stats.completed, 0);
    assert_eq!(stats.cancelled_in_flight, 1);
    assert_eq!(stats.stale_results, 1);
    assert_eq!(download.hash_queue_size(), 0);

    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn stop_clears_queue_and_invalidates_results() {
    let metadata = metadata(4, 3, "stop_queue");
    let root = temp_root("stop");
    write_pieces(&metadata, &root);

    // Capacity one leaves two jobs queued behind the in-flight hash.
    let thread = Arc::new(DiskThread::spawn(1).unwrap());
    let mut download = Download::new(metadata);
    download.set_disk_thread(Some(Arc::clone(&thread)));
    download.set_save_directory(&root);
    download.open().unwrap();
    download.try_start().unwrap();

    assert_eq!(download.submit_hash_check(&[0, 1, 2]).unwrap(), 3);
    assert!(download.hash_queue_size() >= 1);
    assert!(download.pending_disk_hashes() >= 1);
    let generation = download.disk_generation();
    download.stop();

    assert_eq!(download.hash_queue_size(), 0);
    assert_eq!(download.pending_disk_hashes(), 0);
    assert_ne!(download.disk_generation(), generation);
    assert_eq!(download.poll_disk_hash_completions(), 0);
    assert_eq!(download.completed_chunks(), 0);

    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn close_clears_queue_and_invalidates_results() {
    let metadata = metadata(4, 3, "close_queue");
    let root = temp_root("close");
    write_pieces(&metadata, &root);

    let thread = Arc::new(DiskThread::spawn(1).unwrap());
    let mut download = Download::new(metadata);
    download.set_disk_thread(Some(Arc::clone(&thread)));
    download.set_save_directory(&root);
    download.open().unwrap();
    download.try_start().unwrap();

    assert_eq!(download.submit_hash_check(&[0, 1, 2]).unwrap(), 3);
    assert!(download.hash_queue_size() >= 1);
    download.close().unwrap();

    assert!(download.is_closed());
    assert_eq!(download.hash_queue_size(), 0);
    assert_eq!(download.pending_disk_hashes(), 0);
    assert_eq!(download.poll_disk_hash_completions(), 0);
    assert_eq!(download.completed_chunks(), 0);

    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}
