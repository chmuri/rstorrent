//! Phase 5.1 — disk writes must not change the completed bitfield.
//!
//! The worker completes a piece as soon as `write(2)` returns; durability is
//! deferred to `sync_all`. Removing a per-piece fsync must therefore leave the
//! in-memory bitfield identical before and after the sync. This is the
//! data-safety gate for any future multi-worker disk pool: a piece is either
//! written completely or not at all, independent of which worker wrote it.

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

const PIECE_LEN: usize = 8;
const PIECES: usize = 4;

fn piece(index: usize) -> Vec<u8> {
    vec![b'a' + index as u8; PIECE_LEN]
}

fn metadata_for(payload: &[u8]) -> TorrentMetadata {
    let mut file_path = TorrentPath::new();
    file_path.push("payload.bin");

    let pieces: Vec<u8> = (0..PIECES)
        .flat_map(|index| sha1_hash(&piece(index)).to_vec())
        .collect();

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&pieces)),
        name: "bitfield-durability".to_string(),
        piece_length: PIECE_LEN as u32,
        pieces,
        num_pieces: PIECES,
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

fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rstorrent_bitfield_durability_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn completed_bitfield_is_identical_before_and_after_flush() {
    let payload: Vec<u8> = (0..PIECES).flat_map(piece).collect();
    let root = temp_root("sync");

    let thread = Arc::new(DiskThread::spawn(16).unwrap());
    let mut engine = EngineDownload::new(metadata_for(&payload));
    engine.set_disk_thread(Some(Arc::clone(&thread)));
    engine.set_save_directory(&root);
    engine.start();

    for index in 0..PIECES {
        assert!(
            engine
                .receive_block(index as u32, 0, &piece(index))
                .expect("piece should verify"),
            "piece {index} should be accepted"
        );
    }

    // Drain the queued writes without calling fsync. A hard deadline guards
    // against a stalled worker; no wall-clock measurement is asserted.
    let deadline = Instant::now() + Duration::from_secs(30);
    while engine.pending_disk_writes() > 0 || engine.disk_sync_queue_depth() > 0 {
        engine.poll_disk_completions();
        assert!(
            Instant::now() < deadline,
            "disk writes did not drain within the 30 s safety deadline"
        );
        std::thread::yield_now();
    }

    assert!(engine.is_seeder(), "every piece should be complete");

    // Full bitfield, not a piece count: four MSB-first bits set.
    let before = engine.completed_bitfield_bytes();
    assert_eq!(before, vec![0xF0]);

    let mut download = Download::new(engine);

    // Nothing was fsynced per piece, so a resume snapshot must distrust all
    // four pieces and re-hash them after a crash.
    assert_eq!(saved_uncertain(&download), vec![0, 1, 2, 3]);

    download
        .download_mut()
        .flush_disk()
        .expect("flush should succeed");

    let after = download.download().completed_bitfield_bytes();
    assert_eq!(
        after, before,
        "syncing must not change which pieces the bitfield claims"
    );
    assert!(
        saved_uncertain(&download).is_empty(),
        "a completed sync makes every written piece durable"
    );

    assert_eq!(std::fs::read(root.join("payload.bin")).unwrap(), payload);

    thread.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}
