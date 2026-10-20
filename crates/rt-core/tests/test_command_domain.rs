use lt_common::Object;
use rt_core::commands::download::register_download_commands;
use rt_core::commands::exec::register_exec_commands;
use rt_core::commands::file::register_file_commands;
use rt_core::commands::peer::register_peer_commands;
use rt_core::download_factory::DownloadFactory;
use rt_core::download_list::DownloadList;
use rt_core::view_manager::ViewManager;
use rt_rpc::{CommandMap, Target};
use std::sync::{Arc, RwLock};

fn create_test_torrent_object() -> Object {
    let mut info = Object::create_map();
    let _ = info.insert_key("name", Object::from("test_domain_torrent"));
    let _ = info.insert_key("length", Object::from(1048576i64));
    let _ = info.insert_key("piece length", Object::from(262144i64));
    let _ = info.insert_key("pieces", Object::from(vec![0u8; 80]));

    let mut object = Object::create_map();
    let _ = object.insert_key("info", info);
    object
}

fn setup_domain() -> (
    CommandMap,
    Arc<RwLock<DownloadList>>,
    Arc<RwLock<ViewManager>>,
    lt_common::HashString,
) {
    let mut map = CommandMap::new();
    let dl_list = Arc::new(RwLock::new(DownloadList::new()));
    let vm = Arc::new(RwLock::new(ViewManager::new()));

    register_download_commands(&mut map, Arc::clone(&dl_list), Arc::clone(&vm)).unwrap();
    register_file_commands(&mut map, Arc::clone(&dl_list)).unwrap();
    register_peer_commands(&mut map, Arc::clone(&dl_list)).unwrap();

    let obj = create_test_torrent_object();
    let mut factory = DownloadFactory::new();
    factory.load_object(obj);
    let dl = factory.commit(&mut dl_list.write().unwrap()).unwrap();
    let hash = *dl.read().unwrap().info_hash();

    vm.write().unwrap().insert_download_to_all(Arc::clone(&dl));
    // Make visible in main view
    vm.write()
        .unwrap()
        .find_mut("main")
        .unwrap()
        .set_visible(&hash, None);

    (map, dl_list, vm, hash)
}

#[test]
fn test_download_commands() {
    let (map, _dl_list, _vm, hash) = setup_domain();
    let target = Target::Download(hash);

    let name = map
        .call_command("d.name", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!("test_domain_torrent", name.as_str().unwrap());

    let size = map
        .call_command("d.size_bytes", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!(1048576, size.as_value().unwrap());

    let is_open = map
        .call_command("d.is_open", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!(0, is_open.as_value().unwrap());

    // Start download
    map.call_command("d.start", &Object::create_empty(), &target, true)
        .unwrap();
    let is_open_after = map
        .call_command("d.is_open", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!(1, is_open_after.as_value().unwrap());

    // Priority
    map.call_command("d.priority.set", &Object::from("3"), &target, true)
        .unwrap();
    let prio = map
        .call_command("d.priority", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!(3, prio.as_value().unwrap());

    // Directory
    map.call_command(
        "d.directory.set",
        &Object::from("/path/to/data"),
        &target,
        true,
    )
    .unwrap();
    let dir = map
        .call_command("d.directory", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!("/path/to/data", dir.as_str().unwrap());

    // Custom
    let custom_set_args =
        Object::create_list_with(vec![Object::from("tracker_status"), Object::from("active")]);
    map.call_command("d.custom.set", &custom_set_args, &target, true)
        .unwrap();
    let custom_val = map
        .call_command("d.custom", &Object::from("tracker_status"), &target, true)
        .unwrap();
    assert_eq!("active", custom_val.as_str().unwrap());

    // Stop
    map.call_command("d.stop", &Object::create_empty(), &target, true)
        .unwrap();
}

#[test]
fn test_d_multicall2() {
    let (map, _dl_list, _vm, hash) = setup_domain();

    let args = Object::create_list_with(vec![
        Object::from("main"),
        Object::from("d.name"),
        Object::from("d.size_bytes"),
        Object::from("d.hash"),
    ]);

    let res = map
        .call_command("d.multicall2", &args, &Target::None, true)
        .unwrap();

    let rows = res.as_list().unwrap();
    assert_eq!(1, rows.len());

    let row = rows[0].as_list().unwrap();
    assert_eq!("test_domain_torrent", row[0].as_str().unwrap());
    assert_eq!(1048576, row[1].as_value().unwrap());
    assert_eq!(hash.to_hex(), row[2].as_str().unwrap());
}

#[test]
fn test_file_commands_and_multicall() {
    let (map, _dl_list, _vm, hash) = setup_domain();
    let target = Target::File(hash, 0);

    let path = map
        .call_command("f.path", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!("test_domain_torrent", path.as_str().unwrap());

    let size = map
        .call_command("f.size_bytes", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!(1048576, size.as_value().unwrap());

    let chunks = map
        .call_command("f.size_chunks", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!(4, chunks.as_value().unwrap());

    // f.multicall
    let dl_target = Target::Download(hash);
    let mc_args =
        Object::create_list_with(vec![Object::from("f.path"), Object::from("f.size_bytes")]);

    let mc_res = map
        .call_command("f.multicall", &mc_args, &dl_target, true)
        .unwrap();
    let rows = mc_res.as_list().unwrap();
    assert_eq!(1, rows.len());
    let row = rows[0].as_list().unwrap();
    assert_eq!("test_domain_torrent", row[0].as_str().unwrap());
    assert_eq!(1048576, row[1].as_value().unwrap());
}

#[test]
fn test_peer_commands() {
    let (map, dl_list, _vm, hash) = setup_domain();
    let addr: lt_net::SocketAddress = "192.168.1.100:51413".parse().unwrap();
    let target = Target::Peer(hash, addr.clone());

    // Peer commands address live connections only.
    let peer_id =
        lt_common::HashString::from_hex("cccccccccccccccccccccccccccccccccccccccc").unwrap();
    let peer = lt_protocol::connection::PeerConnection::new_with_direction(
        peer_id,
        addr,
        1,
        lt_protocol::connection::PeerDirection::Incoming,
        false,
    );
    dl_list
        .write()
        .unwrap()
        .find(&hash)
        .unwrap()
        .write()
        .unwrap()
        .download_mut()
        .add_peer(peer);

    let ip = map
        .call_command("p.address", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!("192.168.1.100", ip.as_str().unwrap());

    let port = map
        .call_command("p.port", &Object::create_empty(), &target, true)
        .unwrap();
    assert_eq!(51413, port.as_value().unwrap());
}

fn bin_prog(name: &str) -> String {
    for dir in ["/bin", "/usr/bin"] {
        let p = format!("{dir}/{name}");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    name.to_string()
}

#[test]
fn test_execute_command_family() {
    let mut map = CommandMap::new();
    register_exec_commands(&mut map).unwrap();

    let echo_cmd = bin_prog("echo");
    let capture_args =
        Object::create_list_with(vec![Object::from(echo_cmd), Object::from("domain")]);
    let captured = map
        .call_command("execute.capture", &capture_args, &Target::None, true)
        .unwrap();
    assert_eq!(captured.as_string().unwrap(), b"domain\n");

    let false_cmd = bin_prog("false");
    let false_args = Object::create_list_with(vec![Object::from(false_cmd)]);
    assert!(map
        .call_command("execute", &false_args, &Target::None, true)
        .is_err());

    let status = map
        .call_command("execute.nothrow", &false_args, &Target::None, true)
        .unwrap();
    assert_eq!(status.as_value().unwrap(), 1);
}
