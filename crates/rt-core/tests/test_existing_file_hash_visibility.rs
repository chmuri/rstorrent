//! Regression tests for hashing pre-existing payloads.
//!
//! The hash pass must be observable (`HASHING`) instead of silently blocking
//! `d.start`, and it must run when the directory was set before the torrent was
//! added.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download as EngineDownload;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn torrent_path(value: &str) -> TorrentPath {
    let mut path = TorrentPath::new();
    path.push(value);
    path
}

fn payload() -> Vec<u8> {
    (0..24).collect()
}

fn metadata() -> TorrentMetadata {
    let payload = payload();
    let mut pieces = Vec::new();
    for chunk in payload.chunks(8) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&pieces)),
        name: "hash_visibility".to_string(),
        piece_length: 8,
        pieces,
        num_pieces: 3,
        total_size: 24,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: torrent_path("payload.bin"),
            length: 24,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

fn temp_root(label: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "rstorrent_hash_visibility_{}_{}_{}",
        label,
        std::process::id(),
        id
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn wait_until<F: Fn() -> bool>(label: &str, predicate: F) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Debug, Clone)]
struct HashObservation {
    checking: bool,
    state: String,
    chunks_hashed: u32,
    completed_bytes: u64,
}

#[test]
fn preexisting_payload_is_hashed_visibly_when_directory_was_set_first() {
    let root = temp_root("visible");
    std::fs::write(root.join("payload.bin"), payload()).unwrap();

    let mut download = Download::new(EngineDownload::new(metadata()));
    // The regression: the directory is known before the torrent is added.
    download.set_directory(root.to_str().unwrap());
    assert!(download.queue_recheck_if_files_exist());
    assert!(download.has_pending_hash_pieces());

    let download = Arc::new(RwLock::new(download));
    let mut list = DownloadList::new();

    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let observations: Arc<Mutex<Vec<HashObservation>>> = Arc::new(Mutex::new(Vec::new()));

    let captured_events = Arc::clone(&events);
    let captured_observations = Arc::clone(&observations);
    list.add_event_listener(Arc::new(move |event, dl| {
        captured_events.lock().unwrap().push(event.to_string());
        if event == "event.download.hash_queued" {
            let guard = dl.read().unwrap();
            captured_observations.lock().unwrap().push(HashObservation {
                checking: guard.is_hash_checking(),
                state: format!("{:?}", guard.state()),
                chunks_hashed: guard.chunks_hashed(),
                completed_bytes: guard.completed_bytes(),
            });
        }
    }));

    list.start(&download);

    // The queued event is emitted synchronously with the HASHING flag set, so
    // the UI has a state to render before the pass completes.
    let observed = observations.lock().unwrap().clone();
    assert_eq!(observed.len(), 1, "expected exactly one hash_queued event");
    assert!(observed[0].checking, "hash_checking must be set");
    assert_eq!(observed[0].state, "Checking");
    assert_eq!(observed[0].chunks_hashed, 0);
    assert_eq!(
        observed[0].completed_bytes, 0,
        "nothing is verified when the pass is queued"
    );

    wait_until("hash pass to finish", || {
        let guard = download.read().unwrap();
        !guard.is_hash_checking()
    });

    {
        let guard = download.read().unwrap();
        assert!(guard.is_hash_checked());
        assert!(!guard.is_hash_failed());
        assert!(guard.is_done());
        assert_eq!(guard.completed_bytes(), 24);
        assert!(guard.pending_hash_pieces().is_empty());
    }

    let events = events.lock().unwrap().clone();
    let queued = events
        .iter()
        .position(|event| event == "event.download.hash_queued")
        .expect("hash_queued missing");
    let done = events
        .iter()
        .position(|event| event == "event.download.hash_done")
        .expect("hash_done missing");
    assert!(queued < done, "queued must precede done: {events:?}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn partial_payload_is_verified_and_keeps_only_matching_pieces() {
    let root = temp_root("partial");
    let mut payload = payload();
    payload[20] ^= 0xFF; // Corrupt the last piece.
    std::fs::write(root.join("payload.bin"), &payload).unwrap();

    let mut download = Download::new(EngineDownload::new(metadata()));
    download.set_directory(root.to_str().unwrap());
    assert!(download.queue_recheck_if_files_exist());

    let download = Arc::new(RwLock::new(download));
    let list = DownloadList::new();
    list.start(&download);

    wait_until("hash pass to finish", || {
        !download.read().unwrap().is_hash_checking()
    });

    {
        let guard = download.read().unwrap();
        assert!(guard.is_hash_checked());
        assert!(!guard.is_done());
        assert_eq!(guard.completed_bytes(), 16);
        assert!(guard.pending_hash_pieces().is_empty());
    }

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn start_without_existing_files_does_not_enter_hashing() {
    let root = temp_root("empty");
    let mut download = Download::new(EngineDownload::new(metadata()));
    download.set_directory(root.to_str().unwrap());
    assert!(!download.queue_recheck_if_files_exist());

    let download = Arc::new(RwLock::new(download));
    let mut list = DownloadList::new();

    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    list.add_event_listener(Arc::new(move |event, _dl| {
        captured.lock().unwrap().push(event.to_string());
    }));

    list.start(&download);

    assert!(!download.read().unwrap().is_hash_checking());
    assert!(!events
        .lock()
        .unwrap()
        .iter()
        .any(|event| event == "event.download.hash_queued"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn repeated_start_during_hashing_queues_and_resumes_once() {
    let root = temp_root("repeat");
    std::fs::write(root.join("payload.bin"), payload()).unwrap();

    let mut download = Download::new(EngineDownload::new(metadata()));
    download.set_directory(root.to_str().unwrap());
    assert!(download.queue_recheck_if_files_exist());

    let download = Arc::new(RwLock::new(download));
    let mut list = DownloadList::new();

    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&events);
    list.add_event_listener(Arc::new(move |event, _dl| {
        captured.lock().unwrap().push(event.to_string());
    }));

    // Every interleaving of a second start with the pass must be a no-op: it
    // may land before the worker takes the queued pieces, while the engine is
    // checking, or after the transfer resumed.
    list.start(&download);
    list.start(&download);
    list.start(&download);

    wait_until("hash pass to finish", || {
        !download.read().unwrap().is_hash_checking()
    });

    assert!(download.read().unwrap().is_active());
    assert!(download.read().unwrap().is_hash_checked());

    let events = events.lock().unwrap();
    let count = |name: &str| events.iter().filter(|event| *event == name).count();
    assert_eq!(count("event.download.hash_queued"), 1, "{events:?}");
    assert_eq!(count("event.download.hash_done"), 1, "{events:?}");
    assert_eq!(
        count("event.download.resumed"),
        1,
        "verified transfer must resume exactly once: {events:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

fn metadata_sized(payload: &[u8], piece_length: usize) -> TorrentMetadata {
    let mut pieces = Vec::new();
    for chunk in payload.chunks(piece_length) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&pieces)),
        name: "sized".to_string(),
        piece_length: piece_length as u32,
        pieces,
        num_pieces: payload.len().div_ceil(piece_length),
        total_size: payload.len() as u64,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: torrent_path("payload.bin"),
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
fn completed_bytes_clamps_the_short_final_piece() {
    let root = temp_root("clamp");
    let payload: Vec<u8> = (0..20).collect();
    std::fs::write(root.join("payload.bin"), &payload).unwrap();

    let mut download = Download::new(EngineDownload::new(metadata_sized(&payload, 8)));
    download.set_directory(root.to_str().unwrap());
    assert!(download.queue_recheck_if_files_exist());

    let download = Arc::new(RwLock::new(download));
    let list = DownloadList::new();
    list.start(&download);

    wait_until("hash pass to finish", || {
        !download.read().unwrap().is_hash_checking()
    });

    let guard = download.read().unwrap();
    assert!(guard.is_done());
    assert_eq!(
        guard.completed_bytes(),
        20,
        "a 3x8 piece grid over 20 bytes must not round up to 24"
    );
    assert_eq!(guard.metadata().num_pieces, 3);

    let _ = std::fs::remove_dir_all(&root);
}
