//! Restore-time recheck policy.
//!
//! `Control::initialize` must verify the payload when there is no usable
//! resume data, and must not rehash a torrent whose resume data was accepted.

use lt_common::Object;
use rstorrent::{Cli, Control};
use rt_core::manager::Manager;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// `Control::initialize` touches process-global state (disk pool, ports).
static SERIAL: Mutex<()> = Mutex::new(());

const NAME: &str = "recheck-payload.bin";
const SIZE: u64 = 16384;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "rstorrent_recheck_{label}_{}_{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn free_port() -> u16 {
    let probe = TcpListener::bind(("127.0.0.1", 0)).expect("bind probe");
    probe.local_addr().expect("probe addr").port()
}

fn torrent_object() -> Object {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from(NAME)).unwrap();
    info.insert_key("piece length", Object::from(SIZE as i64))
        .unwrap();
    info.insert_key("pieces", Object::create_string(vec![0u8; 20]))
        .unwrap();
    info.insert_key("length", Object::from(SIZE as i64))
        .unwrap();

    let mut root = Object::create_map();
    root.insert_key("info", info).unwrap();
    root.insert_key("announce", Object::from("http://tracker/announce"))
        .unwrap();
    root
}

/// Builds a session directory holding one torrent, its payload and (unless
/// `with_resume` is false) a resume record with one completed piece.
fn prepare_session(root: &TestDir, with_resume: bool) -> (PathBuf, PathBuf) {
    let data = root.path.join("data");
    let session = root.path.join("session");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&session).unwrap();
    fs::write(data.join(NAME), vec![0u8; SIZE as usize]).unwrap();

    let manager = Manager::new();
    {
        let mut manager_session = manager.session_manager().write().unwrap();
        manager_session
            .set_path(&session.to_string_lossy())
            .unwrap();
        manager_session.start().unwrap();
    }
    let download = manager
        .load_torrent_object(torrent_object(), false)
        .expect("load torrent");
    {
        let mut guard = download.write().unwrap();
        guard.set_directory(data.to_string_lossy().to_string());
        guard.download_mut().chunk_selector_mut().mark_completed(0);
    }
    manager.session_save().expect("save session");

    // `shutdown()` persists the session again, so the resume record has to be
    // removed afterwards to model a session that never completed a piece.
    manager.shutdown().expect("shutdown");
    if !with_resume {
        for entry in fs::read_dir(&session).unwrap().flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .ends_with(".libtorrent_resume")
            {
                fs::remove_file(entry.path()).unwrap();
            }
        }
    }

    (session, data)
}

fn cli_for(session: &Path, data: &Path) -> Cli {
    let port = free_port();
    Cli {
        session: Some(session.to_string_lossy().into_owned()),
        directory: Some(data.to_string_lossy().into_owned()),
        no_rc: true,
        daemon: true,
        port_range: Some(format!("{port}-{port}")),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_trusted_resume_restores_without_queueing_a_recheck() {
    let _serial = SERIAL.lock().unwrap();
    let root = TestDir::new("trusted");
    let (session, data) = prepare_session(&root, true);

    let mut control = Control::new();
    control
        .initialize(&cli_for(&session, &data))
        .expect("initialize");

    assert_eq!(
        control.queued_rechecks.load(Ordering::Relaxed),
        0,
        "a validated resume must not force a recheck"
    );
    assert_eq!(control.download_list.read().unwrap().len(), 1);
}

#[tokio::test]
async fn a_missing_resume_still_verifies_the_existing_payload() {
    let _serial = SERIAL.lock().unwrap();
    let root = TestDir::new("missing");
    let (session, data) = prepare_session(&root, false);

    let mut control = Control::new();
    control
        .initialize(&cli_for(&session, &data))
        .expect("initialize");

    assert_eq!(
        control.queued_rechecks.load(Ordering::Relaxed),
        1,
        "an existing payload without resume data must be verified"
    );
}
