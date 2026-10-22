use lt_common::Object;
use rt_core::download_factory::DownloadFactory;
use rt_core::download_list::DownloadList;
use std::sync::{Arc, Mutex};

fn create_test_torrent_object() -> Object {
    let mut info = Object::create_map();
    let _ = info.insert_key("name", Object::from("test_download_list"));
    let _ = info.insert_key("length", Object::from(16i64));
    let _ = info.insert_key("piece length", Object::from(262144i64));
    let _ = info.insert_key("pieces", Object::from(vec![0u8; 20]));

    let mut object = Object::create_map();
    let _ = object.insert_key("info", info);
    object
}

fn insert_download(list: &mut DownloadList) -> String {
    let object = create_test_torrent_object();
    let mut factory = DownloadFactory::new();
    factory.load_object(object);
    let dl = factory.commit(list).unwrap();
    let hex = dl.read().unwrap().info_hash_hex();
    hex.to_lowercase()
}

#[test]
fn test_find_hex() {
    let mut list = DownloadList::new();
    let hex = insert_download(&mut list);

    assert!(list.find_hex(&hex).is_some());
    // Also uppercase
    assert!(list.find_hex(&hex.to_uppercase()).is_some());
}

#[test]
fn test_find_hex_wrong_length() {
    let mut list = DownloadList::new();
    let hex = insert_download(&mut list);

    assert!(list.find_hex(&format!("{}f", hex)).is_none());
    assert!(list.find_hex(&format!("{}{}", hex, hex)).is_none());
    assert!(list.find_hex(&hex[..39]).is_none());
}

#[test]
fn test_duplicate_insert_fails() {
    let mut list = DownloadList::new();
    let object = create_test_torrent_object();

    let mut f1 = DownloadFactory::new();
    f1.load_object(object.clone());
    assert!(f1.commit(&mut list).is_ok());

    let mut f2 = DownloadFactory::new();
    f2.load_object(object);
    assert!(f2.commit(&mut list).is_err());
}

#[test]
fn test_erase_and_events() {
    let mut list = DownloadList::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = Arc::clone(&events);

    list.add_event_listener(Arc::new(move |event, dl| {
        let name = dl.read().unwrap().name().to_string();
        events_clone.lock().unwrap().push((event.to_string(), name));
    }));

    let hex = insert_download(&mut list);
    assert_eq!(1, list.len());

    let dl = list.find_hex(&hex).unwrap();
    list.open(&dl);
    list.start(&dl);
    list.stop(&dl);
    list.close(&dl);

    let erased = list.erase_hex(&hex);
    assert!(erased.is_some());
    assert_eq!(0, list.len());

    let evs = events.lock().unwrap();
    assert_eq!("event.download.inserted", evs[0].0);
    assert_eq!("event.download.opened", evs[1].0);
    assert_eq!("event.download.resumed", evs[2].0);
    assert_eq!("event.download.paused", evs[3].0);
    assert_eq!("event.download.closed", evs[4].0);
    assert_eq!("event.download.erased", evs[5].0);
}

#[test]
fn lifecycle_helpers_and_direct_close() {
    let mut list = DownloadList::new();
    let hex = insert_download(&mut list);

    assert!(!list.is_empty());
    assert_eq!(list.all().len(), 1);
    assert!(list
        .find(&lt_common::HashString::from_hex(&hex.to_uppercase()).unwrap())
        .is_some());

    let dl = list.find_hex(&hex).unwrap();

    // A closed download cannot start and emits no lifecycle event.
    assert!(list.try_start(&dl).is_err());

    list.open(&dl);
    list.start(&dl);
    list.pause(&dl);
    list.resume(&dl);
    list.try_stop(&dl).unwrap();
    list.close_directly(&dl);
    assert!(!dl.read().unwrap().is_open());

    // A hash pass without a configured directory still completes safely.
    list.check_hash(&dl);
    list.check_hash_final(&dl);

    // Re-entrant erase is rejected without a second event stream.
    dl.write().unwrap().set_erasing(true);
    assert!(list.erase_hex(&hex).is_none());
    dl.write().unwrap().set_erasing(false);

    list.notify_metadata_updated(&dl);

    list.clear();
    assert!(list.is_empty());
}

#[test]
fn start_and_hash_guards_while_verification_is_running() {
    use lt_storage::file::Priority;
    use lt_storage::resume::{ResumeData, MTIME_RECHECK};

    let mut list = DownloadList::new();
    let hex = insert_download(&mut list);
    let dl = list.find_hex(&hex).unwrap();
    list.open(&dl);

    // A directory with no payload makes resume validation queue the piece.
    let dir = std::env::temp_dir().join(format!(
        "rstorrent_download_list_guards_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dl.write().unwrap().set_directory(dir.to_str().unwrap());

    // Queue a deferred recheck, then pretend the pass is already running.
    let resume = ResumeData {
        bitfield: Some(lt_common::bitfield::Bitfield::new(1)),
        file_priorities: vec![Priority::Normal],
        file_mtimes: vec![MTIME_RECHECK],
        completed_bytes: 0,
    };
    dl.write().unwrap().apply_resume(&resume).unwrap();
    assert!(dl.read().unwrap().has_pending_hash_pieces());

    dl.write().unwrap().set_hash_checking(true);
    list.start(&dl); // already checking: no second pass, no event
    list.check_hash(&dl); // already checking: early return
    list.check_hash_final(&dl);
    dl.write().unwrap().set_hash_checking(false);
    let _ = std::fs::remove_dir_all(&dir);
}
