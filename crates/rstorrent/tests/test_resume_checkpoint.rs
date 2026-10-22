//! `Control::save_resume_checkpoint_state` regression tests.
//!
//! The forced-exit path calls this bounded saver directly instead of the full
//! session commit. It must persist the resume pair, skip mid-verification
//! downloads, and stay a cheap no-op without an active session.

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download as EngineDownload;
use rstorrent::control::Control;
use rt_core::download::Download;

static COUNTER: AtomicU64 = AtomicU64::new(1);

const PAYLOAD: &[u8] = b"AAAAAAAABBBBBBBBCCCCCCCC";

struct Fixture {
    base: std::path::PathBuf,
    session_dir: std::path::PathBuf,
    payload_dir: std::path::PathBuf,
    control: Control,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "rstorrent_control_checkpoint_{}_{}_{}",
            tag,
            std::process::id(),
            id
        ));
        let session_dir = base.join("session");
        let payload_dir = base.join("payload");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&session_dir).unwrap();
        fs::create_dir_all(&payload_dir).unwrap();

        let control = Control::new();
        control
            .session_manager
            .lock()
            .unwrap()
            .set_path(session_dir.to_str().unwrap())
            .unwrap();

        Self {
            base,
            session_dir,
            payload_dir,
            control,
        }
    }

    fn checkpoint(&self) -> std::io::Result<()> {
        Control::save_resume_checkpoint_state(
            std::sync::Arc::clone(&self.control.session_manager),
            std::sync::Arc::clone(&self.control.download_list),
        )
    }

    fn insert(&self, download: Download) -> std::sync::Arc<std::sync::RwLock<Download>> {
        let download = std::sync::Arc::new(std::sync::RwLock::new(download));
        assert!(self
            .control
            .download_list
            .write()
            .unwrap()
            .insert(std::sync::Arc::clone(&download)));
        download
    }

    fn sidecar(&self, hash: &str) -> std::path::PathBuf {
        self.session_dir
            .join(format!("{}.torrent.rtorrent", hash.to_uppercase()))
    }

    fn resume(&self, hash: &str) -> std::path::PathBuf {
        self.session_dir
            .join(format!("{}.torrent.libtorrent_resume", hash.to_uppercase()))
    }

    fn torrent(&self, hash: &str) -> std::path::PathBuf {
        self.session_dir
            .join(format!("{}.torrent", hash.to_uppercase()))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(mut manager) = self.control.session_manager.lock() {
            if manager.is_active() {
                let _ = manager.stop();
            }
        }
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn torrent_path(value: &str) -> TorrentPath {
    let mut path = TorrentPath::new();
    path.push(value);
    path
}

fn metadata(name: &str) -> TorrentMetadata {
    let mut pieces = Vec::new();
    for chunk in PAYLOAD.chunks(8) {
        pieces.extend_from_slice(&sha1_hash(chunk));
    }

    TorrentMetadata {
        info_hash: HashString::new(sha1_hash(name.as_bytes())),
        name: name.to_string(),
        piece_length: 8,
        pieces,
        num_pieces: 3,
        total_size: PAYLOAD.len() as u64,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: torrent_path("payload.bin"),
            length: PAYLOAD.len() as u64,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

fn download(fixture: &Fixture, name: &str) -> Download {
    let mut download = Download::new(EngineDownload::new(metadata(name)));
    download.set_directory(fixture.payload_dir.to_str().unwrap());
    download
}

#[test]
fn checkpoint_state_is_a_noop_without_an_active_session() {
    let fixture = Fixture::new("inactive");
    let download = fixture.insert(download(&fixture, "inactive"));
    let hash = download.read().unwrap().info_hash_hex();

    // Unused session manager: no path configured.
    assert!(fixture.checkpoint().is_ok());
    assert!(!fixture.sidecar(&hash).exists());
    assert!(!fixture.resume(&hash).exists());

    // Configured but not started.
    fixture
        .control
        .session_manager
        .lock()
        .unwrap()
        .start()
        .unwrap();
    fixture
        .control
        .session_manager
        .lock()
        .unwrap()
        .stop()
        .unwrap();

    assert!(fixture.checkpoint().is_ok());
    assert!(!fixture.sidecar(&hash).exists());
    assert!(!fixture.resume(&hash).exists());
}

#[test]
fn checkpoint_state_writes_the_resume_pair_only() {
    let fixture = Fixture::new("writes");
    fixture
        .control
        .session_manager
        .lock()
        .unwrap()
        .start()
        .unwrap();

    let download = fixture.insert(download(&fixture, "writes"));
    let hash = download.read().unwrap().info_hash_hex();
    let torrent_path = fixture.torrent(&hash);
    fs::write(&torrent_path, b"torrent").unwrap();

    fixture.checkpoint().expect("checkpoint should succeed");

    assert_eq!(fs::read(&torrent_path).unwrap(), b"torrent");
    let sidecar = fs::read(fixture.sidecar(&hash)).unwrap();
    let object = lt_common::object::bencode::decode(&sidecar).expect("bencoded sidecar");
    assert_eq!(
        object.get_key("directory").unwrap().as_str().unwrap(),
        fixture.payload_dir.to_str().unwrap()
    );
    assert!(lt_common::object::bencode::decode(&fs::read(fixture.resume(&hash)).unwrap()).is_ok());
}

#[test]
fn checkpoint_state_skips_downloads_under_verification() {
    let fixture = Fixture::new("skips");
    fixture
        .control
        .session_manager
        .lock()
        .unwrap()
        .start()
        .unwrap();

    let mut hashing = download(&fixture, "hashing");
    hashing.set_hash_checking(true);
    let hashing = fixture.insert(hashing);
    let hashing_hash = hashing.read().unwrap().info_hash_hex();

    let mut erasing = download(&fixture, "erasing");
    erasing.set_erasing(true);
    let erasing = fixture.insert(erasing);
    let erasing_hash = erasing.read().unwrap().info_hash_hex();

    let idle = fixture.insert(download(&fixture, "idle"));
    let idle_hash = idle.read().unwrap().info_hash_hex();

    fixture.checkpoint().expect("checkpoint should succeed");

    assert!(!fixture.sidecar(&hashing_hash).exists());
    assert!(!fixture.resume(&hashing_hash).exists());
    assert!(!fixture.sidecar(&erasing_hash).exists());
    assert!(!fixture.resume(&erasing_hash).exists());
    assert!(fixture.sidecar(&idle_hash).exists());
    assert!(fixture.resume(&idle_hash).exists());
}

#[test]
fn checkpoint_state_reports_the_first_error_but_saves_the_rest() {
    let fixture = Fixture::new("errors");
    fixture
        .control
        .session_manager
        .lock()
        .unwrap()
        .start()
        .unwrap();

    let broken = fixture.insert(download(&fixture, "broken"));
    let broken_hash = broken.read().unwrap().info_hash_hex();
    let healthy = fixture.insert(download(&fixture, "healthy"));
    let healthy_hash = healthy.read().unwrap().info_hash_hex();

    fs::create_dir_all(fixture.sidecar(&broken_hash)).unwrap();

    let err = fixture
        .checkpoint()
        .expect_err("a failed publish must be reported");
    assert!(
        err.to_string().contains("directory"),
        "unexpected error: {err}"
    );

    // The remaining downloads were still checkpointed.
    assert!(fixture.sidecar(&healthy_hash).exists());
    assert!(fixture.resume(&healthy_hash).exists());
    assert!(!fixture.resume(&broken_hash).exists());
}

#[tokio::test]
async fn paused_event_triggers_an_immediate_checkpoint() {
    use rstorrent::cli::Cli;

    let base =
        std::env::temp_dir().join(format!("rstorrent_control_listener_{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let session_dir = base.join("session");
    let data_dir = base.join("data");
    fs::create_dir_all(&session_dir).unwrap();
    fs::create_dir_all(&data_dir).unwrap();

    let mut control = Control::new();
    let cli = Cli {
        session: Some(session_dir.to_string_lossy().to_string()),
        directory: Some(data_dir.to_string_lossy().to_string()),
        no_rc: true,
        ..Default::default()
    };
    control.initialize(&cli).expect("initialize should succeed");

    let metadata = TorrentMetadata {
        info_hash: HashString::new(sha1_hash(PAYLOAD)),
        name: "listener".to_string(),
        piece_length: PAYLOAD.len() as u32,
        pieces: sha1_hash(PAYLOAD).to_vec(),
        num_pieces: 1,
        total_size: PAYLOAD.len() as u64,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path: {
                let mut path = TorrentPath::new();
                path.push("payload.bin");
                path
            },
            length: PAYLOAD.len() as u64,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    };

    let mut download = Download::new(EngineDownload::new(metadata));
    download.set_directory(data_dir.to_str().unwrap());
    let hash = download.info_hash_hex().to_uppercase();
    let download = std::sync::Arc::new(std::sync::RwLock::new(download));
    assert!(control
        .download_list
        .write()
        .unwrap()
        .insert(std::sync::Arc::clone(&download)));

    {
        let list = control.download_list.read().unwrap();
        list.start(&download);
        list.try_stop(&download).expect("stop should succeed");
    }

    // `event.download.paused` must persist the sidecar without an explicit save.
    assert!(
        session_dir
            .join(format!("{hash}.torrent.rtorrent"))
            .is_file(),
        "paused event did not checkpoint the download"
    );

    let _ = control.shutdown().await;
    let _ = fs::remove_dir_all(&base);
}
