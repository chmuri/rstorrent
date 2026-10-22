use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lt_common::hash_string::HashString;
use lt_common::Object;
use lt_crypto::sha1::sha1_hash;
use rt_core::manager::Manager;
use rt_core::DownloadFactory;

static COUNTER: AtomicU64 = AtomicU64::new(1);

const TRACKER: &str = "http://tracker.example.com/announce";

fn wait_until<F: Fn() -> bool>(label: &str, predicate: F) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct Fixture {
    root: PathBuf,
    session: PathBuf,
    data: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "rstorrent_magnet_session_{}_{}_{}",
            name,
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&root);
        let session = root.join("session");
        let data = root.join("data");
        fs::create_dir_all(&session).unwrap();
        fs::create_dir_all(&data).unwrap();
        Self {
            root,
            session,
            data,
        }
    }

    fn start_manager(&self) -> Manager {
        let manager = Manager::new();
        {
            let mut session = manager.session_manager().write().unwrap();
            session.set_path(self.session.to_str().unwrap()).unwrap();
            session.start().unwrap();
        }
        manager
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn info_bytes(name: &str) -> (Vec<u8>, HashString) {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from(name)).unwrap();
    info.insert_key("piece length", Object::from(16384i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(vec![0u8; 20]))
        .unwrap();
    info.insert_key("length", Object::from(16384i64)).unwrap();

    let bytes = lt_common::object::bencode::encode(&info).unwrap();
    let hash = HashString::new(sha1_hash(&bytes));
    (bytes, hash)
}

fn magnet_uri(name: &str, hash: &HashString) -> String {
    format!(
        "magnet:?xt=urn:btih:{}&dn={}&tr={}",
        hash.to_hex(),
        name,
        TRACKER.replace(':', "%3A").replace('/', "%2F")
    )
}

fn add_magnet(
    manager: &Manager,
    uri: &str,
) -> std::sync::Arc<std::sync::RwLock<rt_core::Download>> {
    let mut factory = DownloadFactory::new();
    factory.load_magnet(uri).unwrap();
    factory.set_start(false);
    let mut list = manager.download_list().write().unwrap();
    factory.commit(&mut list).unwrap()
}

fn session_torrent_path(fixture: &Fixture, hash: &HashString) -> PathBuf {
    fixture
        .session
        .join(format!("{}.torrent", hash.to_hex().to_uppercase()))
}

#[test]
fn magnet_metadata_is_persisted_as_static_torrent_and_restored() {
    let fixture = Fixture::new("metadata");
    let manager = fixture.start_manager();

    let (bytes, hash) = info_bytes("magnet-session");
    let uri = magnet_uri("magnet-session", &hash);

    let download = add_magnet(&manager, &uri);
    assert!(download.read().unwrap().is_metadata_download());

    // Simulate the network layer applying verified metadata to the engine.
    {
        let mut guard = download.write().unwrap();
        guard
            .download_mut()
            .apply_metadata_bytes(
                &bytes,
                hash,
                Some(TRACKER.to_string()),
                vec![vec![TRACKER.to_string()]],
            )
            .expect("metadata should apply");
        guard.set_directory(fixture.data.to_str().unwrap());
        assert!(
            guard.sync_metadata_from_engine(),
            "wrapper must notice the fetched metadata"
        );
        assert!(
            guard.bencode().is_some(),
            "magnet metadata must produce a static torrent dictionary"
        );
    }

    manager.download_list().read().unwrap().start(&download);
    manager.commit().expect("session commit should succeed");

    let torrent_path = session_torrent_path(&fixture, &hash);
    assert!(
        torrent_path.is_file(),
        "session must contain the static .torrent for a magnet download"
    );

    // The persisted torrent must keep the magnet's info-hash.
    let saved = fs::read(&torrent_path).unwrap();
    let object = lt_common::object::bencode::decode(&saved).unwrap();
    let info = object.get_key("info").unwrap();
    let raw_info = lt_common::object::bencode::encode(info).unwrap();
    assert_eq!(HashString::new(sha1_hash(&raw_info)), hash);

    manager.shutdown().expect("initial shutdown");

    let restored_manager = fixture.start_manager();
    assert_eq!(
        restored_manager.restore_session().unwrap(),
        1,
        "magnet download must be restored from its saved .torrent"
    );

    let downloads = restored_manager.download_list().read().unwrap().all();
    assert_eq!(downloads.len(), 1);
    let restored = downloads[0].read().unwrap();
    assert_eq!(restored.name(), "magnet-session");
    assert_eq!(restored.directory(), fixture.data.to_str().unwrap());
    assert!(
        !restored.is_metadata_download(),
        "restored torrent must carry the real metadata"
    );
    drop(restored);

    // Existing payload is verified asynchronously before the transfer starts.
    wait_until("restored download to become active", || {
        downloads[0].read().unwrap().is_active()
    });

    restored_manager.shutdown().unwrap();
}

#[test]
fn empty_resume_with_existing_payload_queues_recheck_on_restore() {
    let fixture = Fixture::new("recheck");
    let manager = fixture.start_manager();

    let (bytes, _hash) = info_bytes("existing-payload");
    let info = lt_common::object::bencode::decode(&bytes).unwrap();
    let mut root = Object::create_map();
    root.insert_key("info", info).unwrap();
    root.insert_key("announce", Object::from(TRACKER)).unwrap();

    let download = manager
        .load_torrent_object(root, false)
        .expect("torrent should load");
    download
        .write()
        .unwrap()
        .set_directory(fixture.data.to_str().unwrap());

    // The payload already exists, but no recheck has run yet: the resume
    // snapshot has an empty bitfield.
    fs::write(fixture.data.join("existing-payload"), vec![0u8; 16384]).unwrap();
    manager.commit().expect("session commit should succeed");
    manager.shutdown().expect("initial shutdown");

    let restored_manager = fixture.start_manager();
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let downloads = restored_manager.download_list().read().unwrap().all();
    let restored = downloads[0].read().unwrap();
    assert!(
        restored.has_pending_hash_pieces(),
        "existing payload must be rechecked instead of re-downloaded"
    );
    drop(restored);

    restored_manager.shutdown().unwrap();
}

#[test]
fn magnet_without_metadata_is_restored_from_persisted_uri() {
    let fixture = Fixture::new("placeholder");
    let manager = fixture.start_manager();

    let (_bytes, hash) = info_bytes("placeholder-session");
    let uri = magnet_uri("placeholder-session", &hash);
    let download = add_magnet(&manager, &uri);
    download
        .write()
        .unwrap()
        .set_directory(fixture.data.to_str().unwrap());

    manager.download_list().read().unwrap().start(&download);
    manager.commit().expect("session commit should succeed");

    let torrent_path = session_torrent_path(&fixture, &hash);
    assert!(
        !torrent_path.exists(),
        "metadata was never fetched, so no static torrent exists yet"
    );

    let sidecar_path = fixture
        .session
        .join(format!("{}.torrent.rtorrent", hash.to_hex().to_uppercase()));
    let sidecar = fs::read(&sidecar_path).unwrap();
    let sidecar_object = lt_common::object::bencode::decode(&sidecar).unwrap();
    let persisted_uri = sidecar_object
        .get_key_string("magnet_uri")
        .expect("magnet_uri must be persisted in the sidecar");
    assert_eq!(std::str::from_utf8(persisted_uri).unwrap(), uri);

    manager.shutdown().expect("initial shutdown");

    let restored_manager = fixture.start_manager();
    assert_eq!(
        restored_manager.restore_session().unwrap(),
        1,
        "placeholder must be recreated from the persisted magnet URI"
    );

    let downloads = restored_manager.download_list().read().unwrap().all();
    assert_eq!(downloads.len(), 1);
    let restored = downloads[0].read().unwrap();
    assert!(restored.is_metadata_download());
    assert_eq!(restored.name(), "placeholder-session");
    assert_eq!(restored.directory(), fixture.data.to_str().unwrap());
    assert_eq!(restored.magnet_uri(), Some(uri.as_str()));
    drop(restored);

    restored_manager.shutdown().unwrap();
}
