use std::sync::{Arc, Mutex, RwLock};
use lt_common::hash_string::HashString;
use lt_common::Object;
use rt_core::download::Download;
use rt_core::view::View;
use rt_core::view_manager::ViewManager;
use lt_engine::constructor::TorrentMetadata;
use lt_engine::download::Download as EngineDownload;
use rt_rpc::command_flags::FLAG_PUBLIC_RPC;
use rt_rpc::{CommandMap, Target};

fn make_test_download(index: u8) -> Arc<RwLock<Download>> {
    let mut hash_bytes = [0u8; 20];
    hash_bytes[19] = index;
    let hash = HashString::new(hash_bytes);

    let metadata = TorrentMetadata {
        info_hash: hash,
        name: format!("test_download_{}", index),
        piece_length: 262144,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 16,
        is_multi_file: false,
        files: Vec::new(),
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    };

    Arc::new(RwLock::new(Download::new(EngineDownload::new(metadata))))
}

#[test]
fn test_filter_dispatches_events_to_the_right_downloads() {
    let mut map = CommandMap::new();

    let matches_filter = Arc::new(Mutex::new(Vec::<HashString>::new()));
    let added_events = Arc::new(Mutex::new(Vec::<HashString>::new()));
    let removed_events = Arc::new(Mutex::new(Vec::<HashString>::new()));

    let mf_clone = Arc::clone(&matches_filter);
    map.insert_func(
        "test.view.filter",
        move |target, _obj| {
            if let Target::Download(hash) = target {
                let list = mf_clone.lock().unwrap();
                let matched = list.contains(hash);
                Ok(Object::from(if matched { 1i64 } else { 0i64 }))
            } else {
                Ok(Object::from(0i64))
            }
        },
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    let add_clone = Arc::clone(&added_events);
    map.insert_func(
        "test.view.event_added",
        move |target, _obj| {
            if let Target::Download(hash) = target {
                add_clone.lock().unwrap().push(*hash);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    let rem_clone = Arc::clone(&removed_events);
    map.insert_func(
        "test.view.event_removed",
        move |target, _obj| {
            if let Target::Download(hash) = target {
                rem_clone.lock().unwrap().push(*hash);
            }
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    let mut view = View::with_name("test_view");

    let d0 = make_test_download(0);
    let d1 = make_test_download(1);
    let d2 = make_test_download(2);
    let d3 = make_test_download(3);

    let h0 = *d0.read().unwrap().info_hash();
    let h1 = *d1.read().unwrap().info_hash();
    let h2 = *d2.read().unwrap().info_hash();
    let h3 = *d3.read().unwrap().info_hash();

    view.insert(Arc::clone(&d0));
    view.insert(Arc::clone(&d1));
    view.insert(Arc::clone(&d2));
    view.insert(Arc::clone(&d3));

    view.set_visible(&h0, Some(&map));
    view.set_visible(&h1, Some(&map));

    assert_eq!(2, view.size_visible());

    // Matches filter set to d2 and d3
    *matches_filter.lock().unwrap() = vec![h2, h3];

    view.set_filter(Object::from("test.view.filter="));
    view.set_event_added(Object::from("test.view.event_added="));
    view.set_event_removed(Object::from("test.view.event_removed="));

    added_events.lock().unwrap().clear();
    removed_events.lock().unwrap().clear();

    view.filter(Some(&map));

    assert_eq!(vec![h2, h3], *added_events.lock().unwrap());
    assert_eq!(vec![h0, h1], *removed_events.lock().unwrap());

    assert_eq!(2, view.size_visible());
    assert_eq!(h2, *view.visible_downloads()[0].read().unwrap().info_hash());
    assert_eq!(h3, *view.visible_downloads()[1].read().unwrap().info_hash());
}

#[test]
fn test_view_focus_navigation() {
    let mut view = View::with_name("focus_test");

    let d0 = make_test_download(0);
    let d1 = make_test_download(1);
    let d2 = make_test_download(2);

    let h0 = *d0.read().unwrap().info_hash();
    let h1 = *d1.read().unwrap().info_hash();
    let h2 = *d2.read().unwrap().info_hash();

    view.insert(d0);
    view.insert(d1);
    view.insert(d2);

    view.set_visible(&h0, None);
    view.set_visible(&h1, None);
    view.set_visible(&h2, None);

    assert_eq!(3, view.size_visible());
    assert_eq!(0, view.focus_index());

    view.next_focus(1);
    assert_eq!(1, view.focus_index());

    view.next_focus(1);
    assert_eq!(2, view.focus_index());

    // Moving beyond bounds stops at size
    view.next_focus(1);
    assert_eq!(3, view.focus_index());

    // prev_focus from boundary
    view.prev_focus(1);
    assert_eq!(2, view.focus_index());

    view.prev_focus(1);
    assert_eq!(1, view.focus_index());

    view.prev_focus(1);
    assert_eq!(0, view.focus_index());

    // prev_focus at 0 rolls over to size - 1
    view.prev_focus(1);
    assert_eq!(2, view.focus_index());
}

#[test]
fn test_view_manager_basics() {
    let mut vm = ViewManager::new();

    assert!(vm.find("main").is_some());
    assert!(vm.find("default").is_some());
    assert!(vm.find("active").is_some());
    assert!(vm.find("started").is_some());
    assert!(vm.find("stopped").is_some());

    let custom = vm.insert("custom_view");
    assert_eq!("custom_view", custom.name());
    custom.set_filter_on(vec!["event.download.inserted".to_string()]);

    let d = make_test_download(5);
    vm.insert_download_to_all(Arc::clone(&d));

    for v in vm.all() {
        assert_eq!(1, v.total_size());
    }

    vm.filter_on_trigger("event.download.inserted", None);
}
