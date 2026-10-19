use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use lt_common::bitfield::Bitfield;
use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download as EngineDownload;
use lt_storage::resume::ResumeData;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;

const BYTES: &[u8] = b"AAAAAAAABBBBBBBBCCCCCCCC";

fn torrent_path(value: &str) -> TorrentPath {
    let mut path = TorrentPath::new();
    path.push(value);
    path
}

fn metadata() -> TorrentMetadata {
    let mut pieces = Vec::new();
    for chunk in BYTES.chunks(8) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(&pieces)),
        name: "hash_events".to_string(),
        piece_length: 8,
        pieces,
        num_pieces: 3,
        total_size: 24,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: torrent_path("data.bin"),
            length: 24,
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
        "rtorrent_rtcore_hash_events_{}_{}",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write_all(root: &std::path::Path) {
    let metadata = metadata();
    let target_files: Vec<lt_storage::TargetFileInfo> = metadata
        .files
        .iter()
        .map(|file| lt_storage::TargetFileInfo {
            path: PathBuf::from(
                file.path.as_string().trim_start_matches('/'),
            ),
            length: file.length,
            offset: file.offset,
        })
        .collect();

    for (index, chunk) in BYTES.chunks(8).enumerate() {
        lt_storage::write_piece_to_disk(
            root,
            &target_files,
            8,
            index as u32,
            chunk,
        )
        .unwrap();
    }
}

fn make_download(root: Option<&std::path::Path>) -> Download {
    let mut engine = EngineDownload::new(metadata());
    engine.set_disk_thread(None);
    let mut download = Download::new(engine);
    if let Some(root) = root {
        download.set_directory(root.to_string_lossy().into_owned());
    }
    download
}

fn capture_events(
    list: &mut DownloadList,
) -> Arc<Mutex<Vec<String>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    list.add_event_listener(Arc::new(move |event, _| {
        sink.lock().unwrap().push(event.to_string());
    }));
    events
}

fn events_after_insert(events: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.as_str() != "event.download.inserted")
        .cloned()
        .collect()
}

#[test]
fn successful_check_emits_queued_then_done_and_finished() {
    let root = temp_root("success");
    write_all(&root);

    let mut list = DownloadList::new();
    let events = capture_events(&mut list);
    let download = Arc::new(RwLock::new(make_download(Some(&root))));
    assert!(list.insert(Arc::clone(&download)));

    list.check_hash(&download);

    assert!(download.read().unwrap().is_hash_checked());
    assert!(!download.read().unwrap().is_hash_failed());
    assert_eq!(
        events_after_insert(&events),
        vec![
            "event.download.hash_queued".to_string(),
            "event.download.hash_done".to_string(),
            "event.download.finished".to_string(),
        ]
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rehash_with_bad_pieces_emits_hash_done_and_records_failure() {
    let root = temp_root("bad_piece");
    write_all(&root);

    let path = root.join("data.bin");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[9] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();

    let mut list = DownloadList::new();
    let events = capture_events(&mut list);
    let download = Arc::new(RwLock::new(make_download(Some(&root))));
    assert!(list.insert(Arc::clone(&download)));

    list.check_hash(&download);

    let guard = download.read().unwrap();
    assert!(guard.is_hash_checked());
    assert!(guard.is_hash_failed());
    assert_eq!(guard.download().completed_chunks(), 2);
    drop(guard);

    // A normal recheck completes even when chunks are bad; `hash_done` is
    // still the terminal event and no `finished` event is emitted.
    assert_eq!(
        events_after_insert(&events),
        vec![
            "event.download.hash_queued".to_string(),
            "event.download.hash_done".to_string(),
        ]
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn final_check_failure_emits_hash_final_failed() {
    let root = temp_root("final_failure");
    write_all(&root);

    let mut list = DownloadList::new();
    let events = capture_events(&mut list);
    let download = Arc::new(RwLock::new(make_download(Some(&root))));
    assert!(list.insert(Arc::clone(&download)));

    // First pass validates the complete payload.
    list.check_hash(&download);
    assert!(download.read().unwrap().is_done());

    // Corrupt a piece and run the completion hash pass.
    let path = root.join("data.bin");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[16] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();

    list.check_hash_final(&download);

    let guard = download.read().unwrap();
    assert!(!guard.is_done());
    assert!(guard.is_hash_failed());
    drop(guard);

    let events = events_after_insert(&events);
    assert_eq!(
        events.last().map(String::as_str),
        Some("event.download.hash_final_failed")
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "event.download.hash_final_failed")
            .count(),
        1
    );
    // The failing final pass must not emit a second `hash_done`.
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "event.download.hash_done")
            .count(),
        1
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unavailable_files_emit_hash_failed_without_hash_done() {
    let mut list = DownloadList::new();
    let events = capture_events(&mut list);
    // No save directory and no files: the check cannot be completed.
    let download = Arc::new(RwLock::new(make_download(None)));
    assert!(list.insert(Arc::clone(&download)));

    list.check_hash(&download);

    let guard = download.read().unwrap();
    assert!(!guard.is_hash_checked());
    drop(guard);

    assert_eq!(
        events_after_insert(&events),
        vec![
            "event.download.hash_queued".to_string(),
            "event.download.hash_failed".to_string(),
        ]
    );
}

#[test]
fn resume_pending_recheck_gates_hash_checked_until_start() {
    let root = temp_root("resume_pending");
    write_all(&root);

    let path = root.join("data.bin");
    let mtime = std::fs::metadata(&path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let mut bitfield = Bitfield::new(3);
    bitfield.set_all();
    let resume = ResumeData {
        bitfield: Some(bitfield),
        file_priorities: Vec::new(),
        // Mismatching mtime forces the whole file range to be rechecked.
        file_mtimes: vec![mtime - 100],
        completed_bytes: 24,
    };

    let mut list = DownloadList::new();
    let events = capture_events(&mut list);
    let download = Arc::new(RwLock::new(make_download(Some(&root))));
    {
        let mut guard = download.write().unwrap();
        guard.open().unwrap();
        guard.apply_resume(&resume).unwrap();
    }
    assert!(list.insert(Arc::clone(&download)));

    let guard = download.read().unwrap();
    assert!(!guard.is_hash_checked());
    assert_eq!(guard.pending_hash_pieces(), vec![0, 1, 2]);
    drop(guard);

    list.start(&download);

    let guard = download.read().unwrap();
    assert!(guard.is_hash_checked());
    assert!(!guard.is_hash_failed());
    assert!(guard.is_done());
    assert!(guard.pending_hash_pieces().is_empty());
    drop(guard);

    let events = events_after_insert(&events);
    let queued = events
        .iter()
        .position(|event| event.as_str() == "event.download.hash_queued")
        .expect("hash_queued missing");
    let done = events
        .iter()
        .position(|event| event.as_str() == "event.download.hash_done")
        .expect("hash_done missing");
    assert!(queued < done);

    let _ = std::fs::remove_dir_all(&root);
}
