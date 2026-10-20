use std::sync::{Arc, Mutex, RwLock};

use lt_common::hash_string::HashString;
use lt_common::Object;
use rt_core::commands::log::register_log_commands;
use rt_core::commands::scheduler::register_scheduler_commands;
use rt_core::commands::view::register_view_commands;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use rt_core::view_manager::ViewManager;
use rt_rpc::command_flags::FLAG_PUBLIC_RPC;
use rt_rpc::{CommandMap, CommandScheduler, Target};

fn metadata(name: &str) -> lt_engine::constructor::TorrentMetadata {
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");
    lt_engine::constructor::TorrentMetadata {
        info_hash: HashString::new([0x42u8; 20]),
        name: name.to_string(),
        piece_length: 4,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 4,
        is_multi_file: false,
        files: vec![lt_engine::constructor::TorrentFileEntry {
            path,
            length: 4,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    }
}

fn call(
    map: &CommandMap,
    name: &str,
    target: &Target,
    args: Vec<Object>,
) -> Result<Object, rt_rpc::RpcError> {
    map.call_command(name, &Object::create_list_with(args), target, true)
}

#[test]
fn view_commands_add_filter_and_toggle_visibility() {
    let download = Arc::new(RwLock::new(Download::from_metadata(metadata("alpha"))));
    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));
    let list = Arc::new(RwLock::new(list));

    let mut manager = ViewManager::new();
    manager.insert("test");
    manager.insert_download_to_all(Arc::clone(&download));
    let manager = Arc::new(RwLock::new(manager));

    let mut map = CommandMap::new();
    let name_list = Arc::clone(&list);
    map.insert_func(
        "d.name",
        move |target, _obj| {
            let Target::Download(hash) = target else {
                return Err(rt_rpc::RpcError::InputError("download target".to_string()));
            };
            let download = name_list
                .read()
                .unwrap()
                .find(hash)
                .ok_or_else(|| rt_rpc::RpcError::InputError("not found".to_string()))?;
            let name = download.read().unwrap().name().to_string();
            Ok(Object::from(name))
        },
        FLAG_PUBLIC_RPC,
        "",
        "download name",
    )
    .unwrap();

    let equals_list = Arc::clone(&list);
    map.insert_func(
        "d.name.equals",
        move |target, obj| {
            let expected = rt_rpc::convert_to_string(obj)?;
            let Target::Download(hash) = target else {
                return Err(rt_rpc::RpcError::InputError("download target".to_string()));
            };
            let download = equals_list
                .read()
                .unwrap()
                .find(hash)
                .ok_or_else(|| rt_rpc::RpcError::InputError("not found".to_string()))?;
            let name = download.read().unwrap().name().to_string();
            Ok(Object::from(if name == expected { 1i64 } else { 0 }))
        },
        FLAG_PUBLIC_RPC,
        "<name>",
        "compare download name",
    )
    .unwrap();

    let snapshot = Arc::new(RwLock::new(map.clone()));
    register_view_commands(&mut map, Arc::clone(&manager), Arc::clone(&list), snapshot).unwrap();

    let target = Target::Download(*download.read().unwrap().info_hash());
    assert!(call(
        &map,
        "view.add",
        &Target::None,
        vec![Object::from("custom")]
    )
    .is_ok());
    let names = call(&map, "view.list", &Target::None, vec![]).unwrap();
    assert!(names
        .as_list()
        .unwrap()
        .iter()
        .any(|item| item.as_str().unwrap() == "custom"));

    assert_eq!(
        call(&map, "view.size", &Target::None, vec![Object::from("test")])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    assert!(call(
        &map,
        "view.filter",
        &Target::None,
        vec![Object::from("test"), Object::from("d.name.equals=alpha")]
    )
    .is_ok());
    assert!(call(
        &map,
        "view.filter_all",
        &Target::None,
        vec![Object::from("test")]
    )
    .is_ok());
    assert_eq!(
        call(&map, "view.size", &Target::None, vec![Object::from("test")])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    // Non-matching filter hides the download; set_visible overrides it.
    assert!(call(
        &map,
        "view.filter",
        &Target::None,
        vec![Object::from("test"), Object::from("d.name.equals=beta")]
    )
    .is_ok());
    assert!(call(
        &map,
        "view.filter_all",
        &Target::None,
        vec![Object::from("test")]
    )
    .is_ok());
    assert_eq!(
        call(&map, "view.size", &Target::None, vec![Object::from("test")])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert!(call(
        &map,
        "view.set_visible",
        &target,
        vec![Object::from("test")]
    )
    .is_ok());
    assert_eq!(
        call(&map, "view.size", &Target::None, vec![Object::from("test")])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
}

#[test]
fn log_commands_route_output_to_a_file() {
    let dir = std::env::temp_dir().join(format!(
        "rtorrent-log-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("events.log");
    let output_name = format!("test-output-{}", std::process::id());

    let mut map = CommandMap::new();
    register_log_commands(&mut map).unwrap();

    assert!(call(
        &map,
        "log.open_file.flush",
        &Target::None,
        vec![
            Object::from(output_name.as_str()),
            Object::from(file.to_string_lossy().as_ref())
        ]
    )
    .is_ok());
    assert!(call(
        &map,
        "log.add_output",
        &Target::None,
        vec![Object::from("events"), Object::from(output_name.as_str())]
    )
    .is_ok());
    assert!(call(
        &map,
        "log.print",
        &Target::None,
        vec![Object::from("events"), Object::from("hello-log")]
    )
    .is_ok());
    assert!(call(
        &map,
        "log.close",
        &Target::None,
        vec![Object::from(output_name.as_str())]
    )
    .is_ok());

    let content = std::fs::read_to_string(&file).unwrap();
    assert!(content.contains("hello-log"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn scheduler_commands_are_idempotent_and_removable() {
    let scheduler = Arc::new(Mutex::new(CommandScheduler::new()));
    let mut map = CommandMap::new();
    register_scheduler_commands(&mut map, Arc::clone(&scheduler)).unwrap();

    assert!(call(
        &map,
        "schedule",
        &Target::None,
        vec![
            Object::from("job"),
            Object::from("1"),
            Object::from("2"),
            Object::from("print=hello"),
        ]
    )
    .is_ok());
    assert!(scheduler.lock().unwrap().find("job").is_some());

    // A second schedule with if_absent must not replace the existing item.
    assert!(call(
        &map,
        "schedule.if_absent",
        &Target::None,
        vec![
            Object::from("job"),
            Object::from("5"),
            Object::from("9"),
            Object::from("print=other"),
        ]
    )
    .is_ok());
    assert_eq!(
        scheduler.lock().unwrap().find("job").unwrap().interval(),
        std::time::Duration::from_secs(2)
    );

    // schedule2 is a redirect to schedule and can replace the key.
    assert!(call(
        &map,
        "schedule2",
        &Target::None,
        vec![
            Object::from("job"),
            Object::from("1"),
            Object::from("3"),
            Object::from("print=updated"),
        ]
    )
    .is_ok());
    assert_eq!(
        scheduler.lock().unwrap().find("job").unwrap().interval(),
        std::time::Duration::from_secs(3)
    );

    assert!(call(
        &map,
        "schedule.remove",
        &Target::None,
        vec![Object::from("job")]
    )
    .is_ok());
    assert!(scheduler.lock().unwrap().find("job").is_none());
}
