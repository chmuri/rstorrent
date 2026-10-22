//! Durable-resume regression tests.
//!
//! A piece written by the disk worker is only as durable as the last fsync.
//! Resume snapshots taken in between must record it as uncertain so the next
//! process re-hashes it instead of trusting page-cache contents.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download as EngineDownload;
use lt_storage::disk_thread::DiskThread;
use lt_storage::resume::ResumeExtras;
use rt_core::download::Download;
use rt_core::manager::Manager;

fn metadata_for(payload: &[u8]) -> TorrentMetadata {
    let piece_hash = sha1_hash(payload);
    let mut file_path = TorrentPath::new();
    file_path.push("payload.bin");

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&piece_hash)),
        name: "uncertain".to_string(),
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

fn saved_uncertain(download: &Download) -> Vec<u32> {
    let entry = Manager::build_commit_entry(download).expect("entry should serialize");
    let object = lt_common::object::bencode::decode(&entry.libtorrent_stream)
        .expect("resume stream should decode");
    ResumeExtras::from_object(&object).uncertain_pieces
}

#[test]
fn unsynced_pieces_are_persisted_and_cleared_after_sync() {
    let payload = b"durable-resume-payload".to_vec();
    let root = std::env::temp_dir().join(format!(
        "rstorrent_uncertain_persist_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let thread = Arc::new(DiskThread::spawn(8).unwrap());
    let mut engine = EngineDownload::new(metadata_for(&payload));
    engine.set_disk_thread(Some(Arc::clone(&thread)));
    engine.set_save_directory(&root);
    engine.start();

    assert!(engine
        .receive_block(0, 0, &payload)
        .expect("piece should verify"));

    let deadline = Instant::now() + Duration::from_secs(5);
    while !engine.is_seeder() && Instant::now() < deadline {
        engine.poll_disk_completions();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(engine.is_seeder(), "disk write never completed");

    let mut download = Download::new(engine);

    // The write is on disk but not fsynced: the snapshot must say so.
    assert_eq!(saved_uncertain(&download), vec![0]);

    download
        .download_mut()
        .flush_disk()
        .expect("flush should succeed");

    // After fsync the piece is durable and no longer uncertain.
    assert!(saved_uncertain(&download).is_empty());

    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}
