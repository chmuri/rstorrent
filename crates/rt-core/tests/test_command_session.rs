use std::sync::{Arc, Mutex, RwLock};

use lt_common::Object;
use rt_core::commands::session::register_session_commands;
use rt_core::session::SessionManager;
use rt_core::DownloadList;
use rt_rpc::{CommandMap, Target};

fn call(map: &CommandMap, name: &str, args: Vec<Object>) -> Result<Object, rt_rpc::RpcError> {
    map.call_command(name, &Object::create_list_with(args), &Target::None, true)
}

fn temp_session_dir(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "rtorrent-session-cmd-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn session_commands_round_trip_and_erase_entries() {
    let dir = temp_session_dir("cmds");
    let path = format!("{}/", dir.display());

    let manager = Arc::new(Mutex::new(SessionManager::new()));
    let list = Arc::new(RwLock::new(DownloadList::new()));
    let mut map = CommandMap::new();
    register_session_commands(&mut map, Arc::clone(&manager), Arc::clone(&list)).unwrap();

    assert!(call(&map, "session.path.set", vec![Object::from(path.as_str())]).is_ok());
    assert_eq!(
        call(&map, "session.path", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        path
    );

    manager.lock().unwrap().start().unwrap();

    assert!(call(&map, "session.name.set", vec![Object::from("main")]).is_ok());
    assert_eq!(
        call(&map, "session.name", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "main"
    );

    assert!(call(&map, "session.use_lock.set", vec![Object::from(0i64)]).is_err());
    assert_eq!(
        call(&map, "session.use_lock", vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    assert!(call(&map, "session.on_completion.set", vec![Object::from(0i64)]).is_ok());
    assert_eq!(
        call(&map, "session.on_completion", vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    // Empty download lists commit successfully without creating stray files.
    assert!(call(&map, "session.save", vec![]).is_ok());

    let hash = "0123456789ABCDEF0123456789ABCDEF01234567";
    let torrent = format!("{path}{hash}.torrent");
    let rtorrent = format!("{path}{hash}.torrent.rtorrent");
    let libtorrent = format!("{path}{hash}.torrent.libtorrent_resume");
    std::fs::write(&torrent, b"torrent").unwrap();
    std::fs::write(&rtorrent, b"rtorrent").unwrap();
    std::fs::write(&libtorrent, b"libtorrent").unwrap();

    assert!(manager.lock().unwrap().erase_entry(hash).unwrap());
    assert!(!std::path::Path::new(&torrent).exists());
    assert!(!std::path::Path::new(&rtorrent).exists());
    assert!(!std::path::Path::new(&libtorrent).exists());

    manager.lock().unwrap().stop().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
