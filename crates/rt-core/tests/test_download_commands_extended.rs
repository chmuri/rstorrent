use std::sync::{Arc, RwLock};

use lt_common::hash_string::HashString;
use lt_common::Object;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use rt_core::commands::download::register_download_commands;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use rt_core::view_manager::ViewManager;
use rt_rpc::{CommandMap, Target};

fn metadata() -> TorrentMetadata {
    let info_hash = HashString::from_hex("3131313131313131313131313131313131313131").unwrap();
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");

    TorrentMetadata {
        info_hash,
        name: "extended".to_string(),
        piece_length: 16384,
        pieces: vec![0u8; 20],
        num_pieces: 1,
        total_size: 16384,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path,
            length: 16384,
            offset: 0,
        }],
        announce: Some("http://tracker/announce".into()),
        announce_list: vec![vec!["http://tracker/announce".into()]],
        is_private: false,
        is_meta_download: false,
    }
}

fn setup() -> (CommandMap, Target, Arc<RwLock<Download>>) {
    let download = Arc::new(RwLock::new(Download::from_metadata(metadata())));
    download.write().unwrap().set_directory("/tmp/extended");

    let hash = *download.read().unwrap().info_hash();
    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));
    let list = Arc::new(RwLock::new(list));

    let mut vm = ViewManager::new();
    let main = vm.insert("main");
    main.insert(Arc::clone(&download));
    vm.insert("other");
    let vm = Arc::new(RwLock::new(vm));

    let mut map = CommandMap::new();
    register_download_commands(&mut map, list, vm).unwrap();

    (map, Target::Download(hash), download)
}

fn call(
    map: &CommandMap,
    name: &str,
    target: &Target,
    args: Vec<Object>,
) -> Result<Object, rt_rpc::RpcError> {
    let args = Object::create_list_with(args);
    map.call_command(name, &args, target, true)
}

#[test]
fn extended_getters_report_download_state() {
    let (map, target, _) = setup();

    assert_eq!(
        call(&map, "d.is_meta", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&map, "d.is_multi_file", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&map, "d.chunk_size", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        16384
    );
    assert_eq!(
        call(&map, "d.size_files", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&map, "d.peers_max", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        200
    );
    assert_eq!(
        call(&map, "d.tracker_size", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert!(call(&map, "d.state", &target, vec![])
        .unwrap()
        .as_value()
        .is_ok());
    assert!(call(&map, "d.base_path", &target, vec![])
        .unwrap()
        .as_str()
        .unwrap()
        .contains("extended"));
}

#[test]
fn setters_and_custom_fields_round_trip() {
    let (map, target, download) = setup();

    assert!(call(&map, "d.message.set", &target, vec![Object::from("hello")]).is_ok());
    assert_eq!(
        call(&map, "d.message", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "hello"
    );

    assert!(call(&map, "d.custom1.set", &target, vec![Object::from("tag")]).is_ok());
    assert_eq!(
        call(&map, "d.custom1", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "tag"
    );

    assert!(call(&map, "d.group.set", &target, vec![Object::from(3i64)]).is_ok());
    assert_eq!(
        call(&map, "d.group", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        3
    );

    assert!(call(&map, "d.peers_max.set", &target, vec![Object::from(25i64)]).is_ok());
    assert_eq!(
        download
            .read()
            .unwrap()
            .download()
            .client_config()
            .max_peers(),
        25
    );
    assert!(call(&map, "d.peers_max.set", &target, vec![Object::from(-1i64)]).is_err());
    assert!(call(
        &map,
        "d.peers_max.set",
        &target,
        vec![Object::from(65537i64)]
    )
    .is_err());
    assert!(call(&map, "d.peers_min.set", &target, vec![Object::from(-1i64)]).is_err());
}

#[test]
fn view_commands_manipulate_membership() {
    let (map, target, _) = setup();

    let views = call(&map, "d.views", &target, vec![]).unwrap();
    let names: Vec<&str> = views
        .as_list()
        .unwrap()
        .iter()
        .filter_map(|item| item.as_str().ok())
        .collect();
    assert!(names.contains(&"main"));

    assert_eq!(
        call(&map, "d.views.has", &target, vec![Object::from("main")])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    assert!(call(
        &map,
        "d.views.push_back",
        &target,
        vec![Object::from("other")]
    )
    .is_ok());
    assert_eq!(
        call(&map, "d.views.has", &target, vec![Object::from("other")])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    assert!(call(&map, "d.views.remove", &target, vec![Object::from("other")]).is_ok());
    assert_eq!(
        call(&map, "d.views.has", &target, vec![Object::from("other")])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
}

#[test]
fn peer_accounting_commands_use_chunk_statistics() {
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");
    let metadata = TorrentMetadata {
        info_hash: HashString::from_hex("3232323232323232323232323232323232323232").unwrap(),
        name: "peer-accounting".to_string(),
        piece_length: 4,
        pieces: vec![0u8; 40],
        num_pieces: 2,
        total_size: 8,
        is_multi_file: false,
        files: vec![TorrentFileEntry {
            path,
            length: 8,
            offset: 0,
        }],
        announce: None,
        announce_list: Vec::new(),
        is_private: false,
        is_meta_download: false,
    };

    let download = Arc::new(RwLock::new(Download::from_metadata(metadata)));
    let hash = *download.read().unwrap().info_hash();
    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));
    let list = Arc::new(RwLock::new(list));
    let mut vm = ViewManager::new();
    let main = vm.insert("main");
    main.insert(Arc::clone(&download));
    let vm = Arc::new(RwLock::new(vm));

    let mut map = CommandMap::new();
    register_download_commands(&mut map, list, vm).unwrap();
    let target = Target::Download(hash);

    let partial_id = HashString::new([0x01; 20]);
    let mut partial = lt_protocol::connection::PeerConnection::new(
        partial_id,
        lt_net::addr::SocketAddress::default(),
        2,
    );
    partial.bitfield.set(0);
    let seeder_id = HashString::new([0x02; 20]);
    let mut seeder = lt_protocol::connection::PeerConnection::new(
        seeder_id,
        lt_net::addr::SocketAddress::default(),
        2,
    );
    seeder.bitfield.set(0);
    seeder.bitfield.set(1);

    {
        let mut guard = download.write().unwrap();
        let mut engine = guard.download_mut();
        assert!(engine.add_peer(partial));
        assert!(engine.add_peer(seeder));
    }

    assert_eq!(
        call(&map, "d.peers_accounted", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&map, "d.peers_complete", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    download
        .write()
        .unwrap()
        .download_mut()
        .remove_peer(&partial_id);
    assert_eq!(
        call(&map, "d.peers_accounted", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&map, "d.peers_complete", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
}
