//! Magnet downloads must verify a payload that already exists on disk once
//! their metadata arrives, instead of downloading it again from zero.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_common::object::Object;
use lt_crypto::sha1::sha1_hash;
use rt_core::download_list::DownloadList;
use rt_core::DownloadFactory;

static COUNTER: AtomicU64 = AtomicU64::new(1);

const PIECE_LENGTH: usize = 1024;
const NAME: &str = "payload.bin";

fn payload() -> Vec<u8> {
    (0..(3 * PIECE_LENGTH)).map(|i| (i % 251) as u8).collect()
}

fn info_bytes() -> (Vec<u8>, HashString) {
    let payload = payload();
    let mut pieces = Vec::new();
    for chunk in payload.chunks(PIECE_LENGTH) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }

    let mut info = Object::create_map();
    info.insert_key("name", Object::from(NAME)).unwrap();
    info.insert_key("piece length", Object::from(PIECE_LENGTH as i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(pieces))
        .unwrap();
    info.insert_key("length", Object::from(payload.len() as i64))
        .unwrap();

    let bytes = lt_common::object::bencode::encode(&info).unwrap();
    let hash = HashString::new(sha1_hash(&bytes));
    (bytes, hash)
}

fn temp_root(label: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "rstorrent_magnet_existing_{}_{}_{}",
        label,
        std::process::id(),
        id
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn wait_until<F: Fn() -> bool>(label: &str, predicate: F) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct Fixture {
    root: PathBuf,
    download: Arc<std::sync::RwLock<rt_core::Download>>,
    list: DownloadList,
    events: Arc<Mutex<Vec<String>>>,
}

fn fixture(label: &str, existing: Option<&[u8]>) -> Fixture {
    let root = temp_root(label);
    if let Some(bytes) = existing {
        fs::write(root.join(NAME), bytes).unwrap();
    }

    let (_, info_hash) = info_bytes();
    let uri = format!("magnet:?xt=urn:btih:{}&dn={}", info_hash.to_hex(), NAME);

    let mut factory = DownloadFactory::new();
    factory.load_magnet(&uri).unwrap();
    factory.set_start(false);

    let mut list = DownloadList::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    list.add_event_listener(Arc::new(move |event, _| {
        sink.lock().unwrap().push(event.to_string());
    }));

    let download = factory.commit(&mut list).unwrap();
    download
        .write()
        .unwrap()
        .set_directory(root.to_str().unwrap());
    list.start(&download);

    Fixture {
        root,
        download,
        list,
        events,
    }
}

impl Fixture {
    /// Simulates `ut_metadata` delivering the verified info dictionary.
    fn deliver_metadata(&self) {
        let (bytes, info_hash) = info_bytes();
        let mut guard = self.download.write().unwrap();
        guard
            .download_mut()
            .apply_metadata_bytes(&bytes, info_hash, None, Vec::new())
            .expect("metadata should apply");
        assert!(
            guard.sync_metadata_from_engine(),
            "wrapper must notice the fetched metadata"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn complete_existing_payload_is_verified_and_seeded() {
    let fixture = fixture("complete", Some(&payload()));
    fixture.deliver_metadata();

    assert!(
        fixture
            .download
            .write()
            .unwrap()
            .take_metadata_recheck_pending(),
        "metadata must request a deferred recheck"
    );
    assert!(
        !fixture
            .download
            .write()
            .unwrap()
            .take_metadata_recheck_pending(),
        "the deferred recheck marker is one-shot"
    );

    assert!(fixture.list.begin_metadata_recheck(&fixture.download));

    wait_until("hash pass to finish", || {
        !fixture.download.read().unwrap().is_hash_checking()
    });

    let guard = fixture.download.read().unwrap();
    assert!(guard.is_done(), "complete payload should be seeded");
    assert_eq!(guard.completed_bytes(), payload().len() as u64);
    assert_eq!(guard.state_name(), "Seeding");
    drop(guard);

    let events = fixture.events.lock().unwrap().clone();
    assert!(
        events
            .iter()
            .any(|event| event == "event.download.hash_queued"),
        "the check must be observable: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event == "event.download.hash_done"),
        "the check must complete: {events:?}"
    );
    assert_eq!(
        fs::read(fixture.root.join(NAME)).unwrap(),
        payload(),
        "verification must not rewrite the payload"
    );
}

#[test]
fn partial_existing_payload_keeps_verified_pieces() {
    let fixture = fixture("partial", Some(&payload()[..2 * PIECE_LENGTH]));
    fixture.deliver_metadata();

    assert!(fixture
        .download
        .write()
        .unwrap()
        .take_metadata_recheck_pending());
    assert!(fixture.list.begin_metadata_recheck(&fixture.download));

    wait_until("hash pass to finish", || {
        !fixture.download.read().unwrap().is_hash_checking()
    });

    let guard = fixture.download.read().unwrap();
    assert!(!guard.is_done(), "partial payload must keep downloading");
    assert_eq!(guard.completed_bytes(), 2 * PIECE_LENGTH as u64);
    assert_eq!(guard.state_name(), "Downloading");
    assert!(!guard.has_pending_hash_pieces());
}

#[test]
fn paused_metadata_recheck_waits_for_the_next_start() {
    let fixture = fixture("paused", Some(&payload()));
    fixture.deliver_metadata();
    assert!(fixture
        .download
        .write()
        .unwrap()
        .take_metadata_recheck_pending());

    // The supervisor may run after the user paused the download. The queued
    // pieces must survive and the pass must not run behind a paused download.
    fixture.list.pause(&fixture.download);
    assert!(fixture.download.read().unwrap().is_paused());
    assert!(
        !fixture.list.begin_metadata_recheck(&fixture.download),
        "a paused download must not start hashing"
    );
    assert!(
        fixture.download.read().unwrap().has_pending_hash_pieces(),
        "the queued pieces must be kept for the next start"
    );
    assert!(
        !fixture
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event == "event.download.hash_queued"),
        "nothing may be hashed while paused"
    );

    fixture.list.start(&fixture.download);

    wait_until("hash pass to finish", || {
        !fixture.download.read().unwrap().is_hash_checking()
    });

    let guard = fixture.download.read().unwrap();
    assert!(guard.is_done(), "the payload should verify and seed");
    assert_eq!(guard.completed_bytes(), payload().len() as u64);
    assert_eq!(guard.state_name(), "Seeding");
}

#[test]
fn corrupt_existing_payload_is_downloaded_again() {
    let fixture = fixture("corrupt", Some(&vec![0u8; payload().len()]));
    fixture.deliver_metadata();
    assert!(fixture
        .download
        .write()
        .unwrap()
        .take_metadata_recheck_pending());
    assert!(fixture.list.begin_metadata_recheck(&fixture.download));

    wait_until("hash pass to finish", || {
        !fixture.download.read().unwrap().is_hash_checking()
    });

    let guard = fixture.download.read().unwrap();
    assert!(!guard.is_done(), "corrupt bytes must not seed");
    assert_eq!(guard.completed_bytes(), 0);
    assert_eq!(guard.state_name(), "Downloading");
    assert!(
        !guard.has_pending_hash_pieces(),
        "a completed pass must not leave the request queue closed"
    );
    drop(guard);

    // The verification pass must not rewrite the file; only a real download
    // may replace the bad pieces.
    assert_eq!(
        fs::read(fixture.root.join(NAME)).unwrap(),
        vec![0u8; payload().len()]
    );
}

#[test]
fn fresh_magnet_without_files_does_not_start_a_recheck() {
    let fixture = fixture("fresh", None);
    fixture.deliver_metadata();

    assert!(
        !fixture
            .download
            .write()
            .unwrap()
            .take_metadata_recheck_pending(),
        "nothing on disk means nothing to verify"
    );
    assert!(!fixture.list.begin_metadata_recheck(&fixture.download));

    let events = fixture.events.lock().unwrap().clone();
    assert!(
        !events
            .iter()
            .any(|event| event == "event.download.hash_queued"),
        "a fresh magnet must not hash: {events:?}"
    );
}
