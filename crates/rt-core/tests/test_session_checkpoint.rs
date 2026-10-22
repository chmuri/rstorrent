//! `session.checkpoint` regression tests.
//!
//! The periodic checkpoint rewrites only `<hash>.torrent.rtorrent` and the
//! resume stream. It must never touch the static `.torrent` file, must skip
//! downloads whose bitfield is still being rewritten (hashing or erasing), and
//! must be a no-op when there is no active session.

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use lt_common::hash_string::HashString;
use lt_common::path::Path as TorrentPath;
use lt_common::Object;
use lt_crypto::sha1::sha1_hash;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_engine::download::Download as EngineDownload;
use rt_core::commands::session::register_session_commands;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use rt_core::session::SessionManager;
use rt_rpc::{CommandMap, Target};

static COUNTER: AtomicU64 = AtomicU64::new(1);

const PAYLOAD: &[u8] = b"AAAAAAAABBBBBBBBCCCCCCCC";

struct Fixture {
    session_dir: std::path::PathBuf,
    payload_dir: std::path::PathBuf,
    manager: Arc<Mutex<SessionManager>>,
    list: Arc<RwLock<DownloadList>>,
    map: CommandMap,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!(
            "rstorrent_checkpoint_cmd_{}_{}_{}",
            tag,
            std::process::id(),
            id
        ));
        let session_dir = base.join("session");
        let payload_dir = base.join("payload");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&session_dir).unwrap();
        fs::create_dir_all(&payload_dir).unwrap();

        let manager = Arc::new(Mutex::new(SessionManager::new()));
        manager
            .lock()
            .unwrap()
            .set_path(session_dir.to_str().unwrap())
            .unwrap();

        let list = Arc::new(RwLock::new(DownloadList::new()));
        let mut map = CommandMap::new();
        register_session_commands(&mut map, Arc::clone(&manager), Arc::clone(&list)).unwrap();

        Self {
            session_dir,
            payload_dir,
            manager,
            list,
            map,
        }
    }

    fn start_session(&self) {
        self.manager.lock().unwrap().start().unwrap();
    }

    fn run(&self, command: &str) {
        let result = self.map.call_command(
            command,
            &Object::create_list_with(Vec::new()),
            &Target::None,
            true,
        );
        assert!(result.is_ok(), "{command} failed: {:?}", result.err());
    }

    fn insert(&self, download: Download) -> Arc<RwLock<Download>> {
        let download = Arc::new(RwLock::new(download));
        assert!(self.list.write().unwrap().insert(Arc::clone(&download)));
        download
    }

    fn torrent(&self, download: &Download) -> std::path::PathBuf {
        self.session_dir.join(format!(
            "{}.torrent",
            download.info_hash_hex().to_uppercase()
        ))
    }

    fn sidecar(&self, download: &Download) -> std::path::PathBuf {
        self.session_dir.join(format!(
            "{}.torrent.rtorrent",
            download.info_hash_hex().to_uppercase()
        ))
    }

    fn resume(&self, download: &Download) -> std::path::PathBuf {
        self.session_dir.join(format!(
            "{}.torrent.libtorrent_resume",
            download.info_hash_hex().to_uppercase()
        ))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(mut manager) = self.manager.lock() {
            if manager.is_active() {
                let _ = manager.stop();
            }
        }
        let _ = fs::remove_dir_all(self.session_dir.parent().unwrap());
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

fn decode_sidecar(path: &std::path::Path) -> Object {
    let bytes = fs::read(path).unwrap();
    lt_common::object::bencode::decode(&bytes).expect("sidecar should be bencoded")
}

#[test]
fn checkpoint_writes_resume_streams_without_touching_the_torrent() {
    let fixture = Fixture::new("writes");
    fixture.start_session();

    let download = fixture.insert(download(&fixture, "writes"));
    let guard = download.read().unwrap();

    // A committed torrent file must survive a resume-only checkpoint.
    let torrent_path = fixture.torrent(&guard);
    fs::write(&torrent_path, b"torrent").unwrap();

    fixture.run("session.checkpoint");

    assert_eq!(fs::read(&torrent_path).unwrap(), b"torrent");

    let sidecar = decode_sidecar(&fixture.sidecar(&guard));
    assert_eq!(
        sidecar.get_key("directory").unwrap().as_str().unwrap(),
        fixture.payload_dir.to_str().unwrap()
    );

    let resume_bytes = fs::read(fixture.resume(&guard)).unwrap();
    assert!(!resume_bytes.is_empty(), "resume stream must be written");
    assert!(
        lt_common::object::bencode::decode(&resume_bytes).is_ok(),
        "resume stream must be bencoded"
    );
}

#[test]
fn checkpoint_refreshes_a_stale_resume_pair() {
    let fixture = Fixture::new("stale");
    fixture.start_session();

    let download = fixture.insert(download(&fixture, "stale"));
    let guard = download.read().unwrap();

    fs::write(fixture.sidecar(&guard), b"garbage").unwrap();
    fs::write(fixture.resume(&guard), b"garbage").unwrap();

    fixture.run("session.checkpoint");

    let sidecar = decode_sidecar(&fixture.sidecar(&guard));
    assert_eq!(
        sidecar.get_key("directory").unwrap().as_str().unwrap(),
        fixture.payload_dir.to_str().unwrap()
    );
    assert!(lt_common::object::bencode::decode(&fs::read(fixture.resume(&guard)).unwrap()).is_ok());
}

#[test]
fn checkpoint_skips_downloads_that_are_hashing_or_erasing() {
    let fixture = Fixture::new("skips");
    fixture.start_session();

    let hashing = download(&fixture, "hashing");
    let erasing = download(&fixture, "erasing");
    let idle = download(&fixture, "idle");

    let hashing_paths = {
        let mut hashing = hashing;
        hashing.set_hash_checking(true);
        let sidecar = fixture.sidecar(&hashing);
        let resume = fixture.resume(&hashing);
        fixture.insert(hashing);
        (sidecar, resume)
    };

    let erasing_paths = {
        let mut erasing = erasing;
        erasing.set_erasing(true);
        let sidecar = fixture.sidecar(&erasing);
        let resume = fixture.resume(&erasing);
        fixture.insert(erasing);
        (sidecar, resume)
    };

    let idle_download = fixture.insert(idle);

    fixture.run("session.checkpoint");

    assert!(
        !hashing_paths.0.exists() && !hashing_paths.1.exists(),
        "a mid-hash download must not persist a half-verified bitfield"
    );
    assert!(
        !erasing_paths.0.exists() && !erasing_paths.1.exists(),
        "an erasing download must not be checkpointed"
    );

    let idle = idle_download.read().unwrap();
    assert!(fixture.sidecar(&idle).exists());
    assert!(fixture.resume(&idle).exists());
}

#[test]
fn checkpoint_is_a_noop_without_an_active_session() {
    let fixture = Fixture::new("inactive");
    let download = fixture.insert(download(&fixture, "inactive"));
    let guard = download.read().unwrap();

    // SessionManager::new() is unused: path is empty.
    fixture.run("session.checkpoint");
    assert!(!fixture.sidecar(&guard).exists());
    assert!(!fixture.resume(&guard).exists());

    // Configured but not started is also inactive.
    fixture.start_session();
    fixture.manager.lock().unwrap().stop().unwrap();

    fixture.run("session.checkpoint");
    assert!(!fixture.sidecar(&guard).exists());
    assert!(!fixture.resume(&guard).exists());
}

#[test]
fn checkpoint_with_an_empty_download_list_is_ok() {
    let fixture = Fixture::new("empty");
    fixture.start_session();

    fixture.run("session.checkpoint");

    let session_files: Vec<_> = fs::read_dir(&fixture.session_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with("rtorrent.lock"))
        .collect();
    assert!(
        session_files.is_empty(),
        "checkpoint must not create entries: {session_files:?}"
    );
}

#[test]
fn checkpoint_continues_after_an_entry_fails_to_publish() {
    let fixture = Fixture::new("errors");
    fixture.start_session();

    let broken = fixture.insert(download(&fixture, "broken"));
    let healthy = fixture.insert(download(&fixture, "healthy"));

    // A directory at the publish destination makes the atomic rename fail.
    fs::create_dir_all(fixture.sidecar(&broken.read().unwrap())).unwrap();

    // One unwritable download must not abort the periodic task.
    fixture.run("session.checkpoint");

    assert!(fixture.sidecar(&healthy.read().unwrap()).exists());
    assert!(fixture.resume(&healthy.read().unwrap()).exists());
    assert!(!fixture.resume(&broken.read().unwrap()).exists());
}
