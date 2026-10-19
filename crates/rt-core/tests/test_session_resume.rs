use lt_common::Object;
use lt_storage::file::Priority;
use rt_core::manager::Manager;
use rt_core::session::load_resume_file;

fn torrent_object() -> Object {
    let mut info = Object::create_map();
    info.insert_key("name", Object::from("resume-test")).unwrap();
    info.insert_key("piece length", Object::from(16384i64)).unwrap();
    info.insert_key("pieces", Object::create_string(vec![0u8; 20])).unwrap();
    info.insert_key("length", Object::from(16384i64)).unwrap();

    let mut root = Object::create_map();
    root.insert_key("info", info).unwrap();
    root.insert_key("announce", Object::from("http://tracker/announce"))
        .unwrap();
    root
}

#[test]
fn session_save_writes_full_resume_data() {
    let directory = std::env::temp_dir().join(format!(
        "rtorrent_resume_session_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();

    let manager = Manager::new();
    {
        let mut session = manager.session_manager().write().unwrap();
        session
            .set_path(&directory.to_string_lossy())
            .unwrap();
        session.start().unwrap();
    }

    let download = manager
        .load_torrent_object(torrent_object(), false)
        .expect("torrent should load");

    {
        let mut download = download.write().unwrap();
        download.set_directory(directory.to_string_lossy().to_string());

        let mut engine = download.download_mut();
        engine.chunk_selector_mut().mark_completed(0);
        engine.file_list_mut().files_mut()[0].set_priority(Priority::High);
    }

    manager.session_save().expect("session save should succeed");

    let hash_hex = download.read().unwrap().info_hash_hex();
    let session_path = {
        manager
            .session_manager()
            .read()
            .unwrap()
            .path()
            .to_string()
    };
    let resume_path = format!(
        "{}{}.torrent.libtorrent_resume",
        session_path,
        hash_hex.to_uppercase()
    );
    let resume = load_resume_file(&resume_path).expect("resume file should load");

    let bitfield = resume.bitfield.expect("bitfield");
    assert!(bitfield.get(0));
    assert_eq!(resume.file_priorities, vec![Priority::High]);
    assert_eq!(resume.completed_bytes, 16384);

    manager.shutdown().expect("shutdown");
    let _ = std::fs::remove_dir_all(&directory);
}
