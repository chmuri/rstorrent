use std::fs::{self, File, OpenOptions};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lt_common::Object;
use lt_storage::file::Priority;
use rt_core::manager::Manager;

static COUNTER: AtomicU64 = AtomicU64::new(1);

const MTIME_A: i64 = 1_700_000_000;
const MTIME_B: i64 = 1_700_100_000;

struct Fixture {
    root: std::path::PathBuf,
    session: std::path::PathBuf,
    data: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "rtorrent_resume_roundtrip_{}_{}",
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

    fn payload_dir(&self) -> std::path::PathBuf {
        self.data.join("resume-roundtrip")
    }

    fn file_a(&self) -> std::path::PathBuf {
        self.payload_dir().join("a.bin")
    }

    fn file_b(&self) -> std::path::PathBuf {
        self.payload_dir().join("b.bin")
    }

    fn create_payload_files(&self) {
        fs::create_dir_all(self.payload_dir()).unwrap();
        fs::write(self.file_a(), vec![0u8; 16384]).unwrap();
        fs::write(self.file_b(), vec![0u8; 16384]).unwrap();
        set_mtime(&self.file_a(), MTIME_A);
        set_mtime(&self.file_b(), MTIME_B);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
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
    info.insert_key("name", Object::from("resume-roundtrip"))
        .unwrap();
    info.insert_key("piece length", Object::from(16384i64))
        .unwrap();
    // Two 16384-byte pieces.
    info.insert_key("pieces", Object::create_string(vec![0u8; 40]))
        .unwrap();

    let mut files = Object::create_list();
    for name in ["a.bin", "b.bin"] {
        let mut entry = Object::create_map();
        entry.insert_key("length", Object::from(16384i64)).unwrap();
        let mut path = Object::create_list();
        path.insert_back(Object::from(name)).unwrap();
        entry.insert_key("path", path).unwrap();
        files.insert_back(entry).unwrap();
    }
    info.insert_key("files", files).unwrap();

    let mut root = Object::create_map();
    root.insert_key("info", info).unwrap();
    root.insert_key("announce", Object::from("http://tracker/announce"))
        .unwrap();
    root
}

fn start_manager(fixture: &Fixture) -> Manager {
    let manager = Manager::new();
    {
        let mut session = manager.session_manager().write().unwrap();
        session.set_path(fixture.session.to_str().unwrap()).unwrap();
        session.start().unwrap();
    }
    manager
}

fn create_and_save(fixture: &Fixture) -> Manager {
    fixture.create_payload_files();
    let manager = start_manager(fixture);

    let download = manager
        .load_torrent_object(torrent_object(), false)
        .expect("torrent should load");

    {
        let mut download = download.write().unwrap();
        download.set_directory(fixture.data.to_str().unwrap());
        let mut engine = download.download_mut();
        engine.chunk_selector_mut().mark_completed(0);
        engine.chunk_selector_mut().mark_completed(1);
        engine.file_list_mut().files_mut()[0].set_priority(Priority::High);
        engine.file_list_mut().files_mut()[1].set_priority(Priority::Off);
    }

    manager.commit().expect("session save should succeed");
    manager
}

fn resume_path(fixture: &Fixture, hash_hex: &str) -> std::path::PathBuf {
    fixture.session.join(format!(
        "{}.torrent.libtorrent_resume",
        hash_hex.to_uppercase()
    ))
}

#[test]
fn save_restart_load_round_trips_bitfield_priorities_and_mtimes() {
    let fixture = Fixture::new();
    let manager = create_and_save(&fixture);

    let hash_hex = manager.download_list().read().unwrap().all()[0]
        .read()
        .unwrap()
        .info_hash_hex();

    let resume =
        rt_core::session::load_resume_file(resume_path(&fixture, &hash_hex).to_str().unwrap())
            .expect("saved resume should load");
    assert_eq!(resume.file_priorities, vec![Priority::High, Priority::Off]);
    assert_eq!(resume.file_mtimes, vec![MTIME_A, MTIME_B]);
    assert_eq!(resume.completed_bytes, 32768);

    // The saved file also carries the legacy per-file `completed` counters.
    let bytes = fs::read(resume_path(&fixture, &hash_hex)).unwrap();
    let object = lt_common::object::bencode::decode(&bytes).unwrap();
    let extras = lt_storage::resume::ResumeExtras::from_object(&object);
    assert_eq!(extras.file_completed_chunks, vec![Some(1), Some(1)]);

    manager.shutdown().expect("initial shutdown");

    let restored_manager = start_manager(&fixture);
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let restored = restored_manager.download_list().read().unwrap().all()[0].clone();
    let guard = restored.read().unwrap();
    assert_eq!(guard.directory(), fixture.data.to_str().unwrap());

    let engine = guard.download();
    assert!(engine.chunk_selector().completed().get(0));
    assert!(engine.chunk_selector().completed().get(1));
    assert_eq!(engine.resume_mtimes(), &[MTIME_A, MTIME_B]);
    assert_eq!(engine.resume_completed_bytes(), 32768);
    assert_eq!(engine.file_list().files()[0].priority(), Priority::High);
    assert_eq!(engine.file_list().files()[1].priority(), Priority::Off);
    drop(engine);
    drop(guard);

    restored_manager.shutdown().expect("restored shutdown");
}

#[test]
fn active_download_restores_progress_and_totals() {
    let fixture = Fixture::new();
    fixture.create_payload_files();
    let manager = start_manager(&fixture);

    let download = manager
        .load_torrent_object(torrent_object(), false)
        .expect("torrent should load");

    {
        let mut download = download.write().unwrap();
        download.set_directory(fixture.data.to_str().unwrap());
        download.open().unwrap();
        {
            let mut engine = download.download_mut();
            engine.chunk_selector_mut().mark_completed(0);
            // Simulate a real transfer history that must survive the restart.
            engine.set_downloaded_bytes(16384);
            engine.set_uploaded_bytes(4096);
        }
        download.start();
    }

    manager.commit().expect("session save should succeed");
    manager.shutdown().expect("initial shutdown");

    let restored_manager = start_manager(&fixture);
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let restored = restored_manager.download_list().read().unwrap().all()[0].clone();
    let guard = restored.read().unwrap();

    // The completed piece is trusted: resume mtimes match and no recheck was
    // queued for it.
    assert!(!guard.has_pending_hash_pieces());
    let engine = guard.download();
    assert!(engine.chunk_selector().completed().get(0));
    assert!(!engine.chunk_selector().completed().get(1));
    assert_eq!(engine.completed_chunks(), 1);
    drop(engine);

    assert_eq!(guard.completed_bytes(), 16384);
    assert_eq!(guard.downloaded_bytes(), 16384);
    assert_eq!(guard.uploaded_bytes(), 4096);
    drop(guard);

    restored_manager.shutdown().expect("restored shutdown");
}

#[test]
fn changed_file_only_clears_its_own_pieces() {
    let fixture = Fixture::new();
    let manager = create_and_save(&fixture);
    manager.shutdown().expect("initial shutdown");

    // Simulate a payload file modified after the resume snapshot.
    set_mtime(&fixture.file_b(), MTIME_B + 60);

    let restored_manager = start_manager(&fixture);
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let restored = restored_manager.download_list().read().unwrap().all()[0].clone();
    let guard = restored.read().unwrap();
    let engine = guard.download();

    // Piece 0 belongs to a.bin and remains trusted.
    assert!(engine.chunk_selector().completed().get(0));
    // Piece 1 belongs to the changed b.bin and was rechecked.
    assert!(!engine.chunk_selector().completed().get(1));
    assert_eq!(
        engine.resume_mtimes(),
        &[MTIME_A, lt_storage::resume::MTIME_RECHECK]
    );
    // Priorities survive even when progress is invalidated.
    assert_eq!(engine.file_list().files()[0].priority(), Priority::High);
    assert_eq!(engine.file_list().files()[1].priority(), Priority::Off);
    drop(engine);
    drop(guard);

    restored_manager.shutdown().expect("restored shutdown");
}

#[test]
fn missing_file_is_skipped_without_aborting_startup() {
    let fixture = Fixture::new();
    let manager = create_and_save(&fixture);
    manager.shutdown().expect("initial shutdown");

    fs::remove_file(fixture.file_b()).unwrap();

    let restored_manager = start_manager(&fixture);
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let restored = restored_manager.download_list().read().unwrap().all()[0].clone();
    let guard = restored.read().unwrap();
    let engine = guard.download();
    assert!(engine.chunk_selector().completed().get(0));
    assert!(!engine.chunk_selector().completed().get(1));
    assert_eq!(
        engine.resume_mtimes(),
        &[MTIME_A, lt_storage::resume::MTIME_RECHECK]
    );
    drop(engine);
    drop(guard);

    restored_manager.shutdown().expect("restored shutdown");
}

#[test]
fn resized_file_with_matching_mtime_is_rechecked() {
    let fixture = Fixture::new();
    let manager = create_and_save(&fixture);
    manager.shutdown().expect("initial shutdown");

    // Same mtime but a different size must still invalidate the file.
    fs::write(fixture.file_b(), vec![0u8; 1024]).unwrap();
    set_mtime(&fixture.file_b(), MTIME_B);

    let restored_manager = start_manager(&fixture);
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let restored = restored_manager.download_list().read().unwrap().all()[0].clone();
    let guard = restored.read().unwrap();
    let engine = guard.download();
    assert!(engine.chunk_selector().completed().get(0));
    assert!(!engine.chunk_selector().completed().get(1));
    drop(engine);
    drop(guard);

    restored_manager.shutdown().expect("restored shutdown");
}

#[test]
fn oversized_file_only_clears_its_own_pieces() {
    let fixture = Fixture::new();
    let manager = create_and_save(&fixture);
    manager.shutdown().expect("initial shutdown");

    // A larger replacement must invalidate b.bin even when the mtime matches.
    fs::write(fixture.file_b(), vec![0u8; 32768]).unwrap();
    set_mtime(&fixture.file_b(), MTIME_B);

    let restored_manager = start_manager(&fixture);
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let restored = restored_manager.download_list().read().unwrap().all()[0].clone();
    let guard = restored.read().unwrap();
    let engine = guard.download();
    assert!(engine.chunk_selector().completed().get(0));
    assert!(!engine.chunk_selector().completed().get(1));
    assert_eq!(
        engine.resume_mtimes(),
        &[MTIME_A, lt_storage::resume::MTIME_RECHECK]
    );
    assert_eq!(engine.file_list().files()[0].priority(), Priority::High);
    assert_eq!(engine.file_list().files()[1].priority(), Priority::Off);
    drop(engine);
    drop(guard);

    restored_manager.shutdown().expect("restored shutdown");
}

#[test]
fn corrupt_resume_entry_does_not_abort_session_restore() {
    let fixture = Fixture::new();
    let manager = create_and_save(&fixture);

    let hash_hex = manager.download_list().read().unwrap().all()[0]
        .read()
        .unwrap()
        .info_hash_hex();
    manager.shutdown().expect("initial shutdown");

    fs::write(resume_path(&fixture, &hash_hex), b"not-bencode").unwrap();

    let restored_manager = start_manager(&fixture);
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let restored = restored_manager.download_list().read().unwrap().all()[0].clone();
    let guard = restored.read().unwrap();
    let engine = guard.download();
    assert_eq!(engine.resume_completed_bytes(), 0);
    assert_eq!(engine.chunk_selector().completed().size_set(), 0);
    drop(engine);
    drop(guard);

    restored_manager.shutdown().expect("restored shutdown");
}

#[test]
fn file_times_helper_uses_whole_seconds() {
    // Guards the helper above against accidentally writing sub-second times
    // while resume data stores whole UNIX seconds.
    let fixture = Fixture::new();
    fixture.create_payload_files();
    let modified = fs::metadata(fixture.file_a())
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    assert_eq!(modified.as_secs() as i64, MTIME_A);
    let _ = File::open(fixture.file_a()).unwrap();
}

fn create_active_and_save(fixture: &Fixture) -> Manager {
    fixture.create_payload_files();
    let manager = start_manager(fixture);

    let download = manager
        .load_torrent_object(torrent_object(), false)
        .expect("torrent should load");
    {
        let mut download = download.write().unwrap();
        download.set_directory(fixture.data.to_str().unwrap());
        download
            .download_mut()
            .chunk_selector_mut()
            .mark_completed(0);
    }
    manager.download_list().read().unwrap().start(&download);
    manager.commit().expect("session save should succeed");
    manager
}

fn record_events(manager: &Manager) -> Arc<Mutex<Vec<String>>> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    manager
        .download_list()
        .write()
        .unwrap()
        .add_event_listener(Arc::new(move |event, _download| {
            sink.lock().unwrap().push(event.to_string());
        }));
    events
}

#[test]
fn resumed_event_is_emitted_after_accepted_resume_data() {
    let fixture = Fixture::new();
    let manager = create_active_and_save(&fixture);
    manager.shutdown().expect("initial shutdown");

    let restored_manager = start_manager(&fixture);
    let events = record_events(&restored_manager);
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.as_str() == "event.download.resumed"),
        "expected resumed event, got {:?}",
        *events
    );
    drop(events);

    restored_manager.shutdown().expect("restored shutdown");
}

#[test]
fn rejected_resume_data_does_not_emit_resumed_event() {
    let fixture = Fixture::new();
    let manager = create_active_and_save(&fixture);

    let hash_hex = manager.download_list().read().unwrap().all()[0]
        .read()
        .unwrap()
        .info_hash_hex();
    manager.shutdown().expect("initial shutdown");

    fs::write(resume_path(&fixture, &hash_hex), b"not-bencode").unwrap();

    let restored_manager = start_manager(&fixture);
    let events = record_events(&restored_manager);
    assert_eq!(restored_manager.restore_session().unwrap(), 1);

    let events = events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|event| event.as_str() == "event.download.resumed"),
        "resumed must not be emitted for rejected resume data: {:?}",
        *events
    );
    drop(events);

    restored_manager.shutdown().expect("restored shutdown");
}

#[test]
fn uncertain_pieces_from_resume_extras_are_rechecked() {
    use lt_common::bitfield::Bitfield;
    use lt_storage::resume::{ResumeData, ResumeExtras};

    let fixture = Fixture::new();
    fixture.create_payload_files();

    let manager = start_manager(&fixture);
    let download = manager
        .load_torrent_object(torrent_object(), false)
        .expect("torrent should load");
    download
        .write()
        .unwrap()
        .set_directory(fixture.data.to_str().unwrap());

    let mut bitfield = Bitfield::new(2);
    bitfield.set(0);
    bitfield.set(1);

    let resume = ResumeData {
        bitfield: Some(bitfield),
        file_priorities: vec![Priority::Normal, Priority::Normal],
        file_mtimes: vec![MTIME_A, MTIME_B],
        completed_bytes: 32768,
    };

    // Piece 1 was written but not fsynced when the snapshot was taken.
    let mut object = resume.to_object();
    ResumeExtras {
        uncertain_pieces: vec![1],
        ..ResumeExtras::default()
    }
    .write_into(&mut object);

    let path = fixture.session.join("uncertain.resume");
    fs::write(
        &path,
        lt_common::object::bencode::encode(&object).expect("resume should encode"),
    )
    .unwrap();

    let mut download = download.write().unwrap();
    let outcome = rt_core::session::load_resume_for_download(path.to_str().unwrap(), &mut download)
        .expect("resume should load");

    assert_eq!(
        outcome.completed_chunks, 1,
        "uncertain piece must be dropped"
    );
    {
        let engine = download.download();
        assert!(engine.chunk_selector().completed().get(0));
        assert!(!engine.chunk_selector().completed().get(1));
    }
    assert_eq!(download.pending_hash_pieces(), vec![1]);
    assert!(!download.is_hash_checked());

    drop(download);
    manager.shutdown().expect("shutdown");
}
