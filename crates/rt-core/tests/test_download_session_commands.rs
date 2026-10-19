use std::fs::{self, OpenOptions};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, UNIX_EPOCH};

use lt_common::Object;
use lt_storage::file::Priority;
use rt_core::commands::download::{
    register_download_commands, register_download_session_commands,
};
use rt_core::manager::Manager;
use rt_core::session::{load_resume_file, SessionManager};
use rt_core::view_manager::ViewManager;
use rt_rpc::{CommandMap, Target};

static COUNTER: AtomicU64 = AtomicU64::new(1);

const MTIME: i64 = 1_700_000_000;

struct Fixture {
    root: std::path::PathBuf,
    session: std::path::PathBuf,
    data: std::path::PathBuf,
    manager: Manager,
    session_manager: Arc<Mutex<SessionManager>>,
    map: CommandMap,
    target: Target,
    torrent_name: String,
}

impl Fixture {
    fn new() -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "rtorrent_cmd_session_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&root);
        let session = root.join("session");
        let data = root.join("data");
        fs::create_dir_all(&session).unwrap();
        fs::create_dir_all(&data).unwrap();

        let manager = Manager::new();
        let download = manager
            .load_torrent_object(torrent_object(), false)
            .expect("torrent should load");

        let payload = data.join("cmd-session");
        fs::write(&payload, vec![0u8; 32768]).unwrap();
        set_mtime(&payload, MTIME);

        let hash = {
            let mut guard = download.write().unwrap();
            guard.set_directory(data.to_str().unwrap());
            let hash = *guard.info_hash();
            let mut engine = guard.download_mut();
            engine.chunk_selector_mut().mark_completed(0);
            engine.chunk_selector_mut().mark_completed(1);
            engine.file_list_mut().files_mut()[0].set_priority(Priority::High);
            hash
        };

        let mut session_state = SessionManager::new();
        session_state.set_path(session.to_str().unwrap()).unwrap();
        session_state.start().unwrap();
        let session_manager = Arc::new(Mutex::new(session_state));

        let mut map = CommandMap::new();
        let view_manager = Arc::new(RwLock::new(ViewManager::new()));
        register_download_commands(
            &mut map,
            Arc::clone(manager.download_list()),
            view_manager,
        )
        .unwrap();
        register_download_session_commands(
            &mut map,
            Arc::clone(manager.download_list()),
            Arc::clone(&session_manager),
        )
        .unwrap();

        Self {
            root,
            session,
            data,
            manager,
            session_manager,
            map,
            target: Target::Download(hash),
            torrent_name: "cmd-session".to_string(),
        }
    }

    fn hash_hex(&self) -> String {
        let download = self.manager.download_list().read().unwrap().all()[0].clone();
        let hash = download.read().unwrap().info_hash_hex();
        hash.to_uppercase()
    }

    fn base_path(&self) -> std::path::PathBuf {
        self.session.join(format!("{}.torrent", self.hash_hex()))
    }

    fn resume_path(&self) -> std::path::PathBuf {
        self.session
            .join(format!("{}.torrent.libtorrent_resume", self.hash_hex()))
    }

    fn rtorrent_path(&self) -> std::path::PathBuf {
        self.session
            .join(format!("{}.torrent.rtorrent", self.hash_hex()))
    }

    fn call(&self, name: &str) -> Result<Object, rt_rpc::RpcError> {
        self.map
            .call_command(name, &Object::create_empty(), &self.target, true)
    }

    fn download(&self) -> Arc<RwLock<rt_core::download::Download>> {
        self.manager.download_list().read().unwrap().all()[0].clone()
    }

    fn stop_session(&self) {
        self.session_manager.lock().unwrap().stop().unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        {
            let mut session = self.session_manager.lock().unwrap();
            if session.is_active() {
                let _ = session.stop();
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn set_mtime(path: &std::path::Path, secs: i64) {
    let file = OpenOptions::new().write(true).open(path).unwrap();
    let times = fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(secs as u64));
    file.set_times(times).unwrap();
}

fn torrent_object() -> Object {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from("cmd-session")).unwrap();
    info.insert_key("piece length", Object::from(16384i64)).unwrap();
    // Two 16384-byte pieces.
    info.insert_key("pieces", Object::create_string(vec![0u8; 40]))
        .unwrap();
    info.insert_key("length", Object::from(32768i64)).unwrap();

    let mut root = Object::create_map();
    root.insert_key("info", info).unwrap();
    root.insert_key("announce", Object::from("http://tracker/announce"))
        .unwrap();
    root
}

#[test]
fn save_resume_writes_dynamic_streams_without_static_torrent() {
    let fixture = Fixture::new();

    fixture
        .call("d.save_resume")
        .expect("save_resume should succeed");

    assert!(
        !fixture.base_path().exists(),
        "resume must not write the torrent"
    );
    assert!(fixture.rtorrent_path().is_file());
    assert!(fixture.resume_path().is_file());

    let resume = load_resume_file(fixture.resume_path().to_str().unwrap()).unwrap();
    let bitfield = resume.bitfield.expect("saved bitfield");
    assert!(bitfield.get(0));
    assert!(bitfield.get(1));
    assert_eq!(resume.file_priorities, vec![Priority::High]);
    assert_eq!(resume.file_mtimes, vec![MTIME]);
    assert_eq!(resume.completed_bytes, 32768);

    // The full variant adds the static torrent without losing resume data.
    fixture
        .call("d.save_full_session")
        .expect("save_full_session should succeed");
    assert!(fixture.base_path().is_file());
    assert!(fixture.rtorrent_path().is_file());

    let stored = load_resume_file(fixture.resume_path().to_str().unwrap()).unwrap();
    assert_eq!(stored.file_priorities, vec![Priority::High]);
    assert_eq!(stored.completed_bytes, 32768);
}

#[test]
fn save_commands_report_an_inactive_session() {
    let fixture = Fixture::new();
    fixture.stop_session();

    let err = fixture.call("d.save_resume").unwrap_err();
    assert!(
        err.to_string().contains("not active"),
        "unexpected error: {err}"
    );
    assert!(!fixture.resume_path().exists());

    let err = fixture.call("d.save_full_session").unwrap_err();
    assert!(
        err.to_string().contains("not active"),
        "unexpected error: {err}"
    );
    assert!(!fixture.base_path().exists());
}

#[test]
fn save_resume_updates_commit_without_rewriting_torrent_and_restores() {
    let fixture = Fixture::new();

    // Establish a full session generation exactly as shutdown/session.commit
    // would, then let the command update only the dynamic streams.
    let entry = {
        let download = fixture.download();
        let guard = download.read().unwrap();
        Manager::build_commit_entry(&guard).unwrap()
    };
    fixture
        .session_manager
        .lock()
        .unwrap()
        .commit(&[entry])
        .expect("initial commit should succeed");
    let committed_torrent = fs::read(fixture.base_path()).unwrap();

    {
        let download = fixture.download();
        let mut guard = download.write().unwrap();
        guard
            .download_mut()
            .file_list_mut()
            .files_mut()[0]
            .set_priority(Priority::Normal);
    }

    fixture
        .call("d.save_resume")
        .expect("save_resume should succeed");

    assert_eq!(
        fs::read(fixture.base_path()).unwrap(),
        committed_torrent,
        "d.save_resume must leave the committed torrent untouched"
    );

    let resume = load_resume_file(fixture.resume_path().to_str().unwrap()).unwrap();
    assert_eq!(resume.file_priorities, vec![Priority::Normal]);
    assert_eq!(resume.completed_bytes, 32768);

    assert_eq!(
        fixture
            .session_manager
            .lock()
            .unwrap()
            .get_valid_entries()
            .unwrap()
            .len(),
        1
    );

    fixture.stop_session();

    let restored_manager = Manager::new();
    {
        let mut session = restored_manager.session_manager().write().unwrap();
        session
            .set_path(fixture.session.to_str().unwrap())
            .unwrap();
        session.start().unwrap();
    }
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let restored = restored_manager.download_list().read().unwrap().all()[0].clone();
    let guard = restored.read().unwrap();
    assert_eq!(guard.directory(), fixture.data.to_str().unwrap());
    assert_eq!(
        guard.download().file_list().files()[0].priority(),
        Priority::Normal
    );
    assert!(guard.download().chunk_selector().completed().get(0));
    assert!(guard.download().chunk_selector().completed().get(1));
    drop(guard);

    assert_eq!(restored_manager.download_list().read().unwrap().len(), 1);
    let name = restored.read().unwrap().name().to_string();
    assert_eq!(name, fixture.torrent_name);

    restored_manager.shutdown().unwrap();
}
