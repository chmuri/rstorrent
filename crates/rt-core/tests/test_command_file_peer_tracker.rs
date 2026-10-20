use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};

use lt_common::hash_string::HashString;
use lt_common::object::flags::{FLAG_AS_BINARY, FLAG_BASE64};
use lt_common::Object;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use lt_net::addr::SocketAddress;
use lt_protocol::connection::{PeerConnection, PeerDirection};
use lt_storage::file::FileFlags;
use lt_tracker::state::{TrackerEvent, TrackerFlags};
use rt_core::commands::file::register_file_commands;
use rt_core::commands::peer::register_peer_commands;
use rt_core::commands::tracker::{register_tracker_commands, DhtConfig};
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use rt_rpc::{CommandMap, Target};

fn metadata() -> TorrentMetadata {
    let info_hash = HashString::from_hex("4242424242424242424242424242424242424242").unwrap();
    let mut first = lt_common::path::Path::new();
    first.push("dir/a.bin");
    let mut second = lt_common::path::Path::new();
    second.push("dir/b.bin");

    TorrentMetadata {
        info_hash,
        name: "file-peer-tracker".to_string(),
        piece_length: 16384,
        pieces: vec![0u8; 40],
        num_pieces: 2,
        total_size: 32768,
        is_multi_file: true,
        files: vec![
            TorrentFileEntry {
                path: first,
                length: 16384,
                offset: 0,
            },
            TorrentFileEntry {
                path: second,
                length: 16384,
                offset: 16384,
            },
        ],
        announce: Some("http://tracker-a/announce".into()),
        announce_list: vec![
            vec!["http://tracker-a/announce".into()],
            vec!["udp://tracker-b:6969/announce".into()],
        ],
        is_private: false,
        is_meta_download: false,
    }
}

struct Fixture {
    map: CommandMap,
    hash: HashString,
    download: Arc<RwLock<Download>>,
}

fn setup() -> Fixture {
    setup_with(metadata())
}

fn setup_with(metadata: TorrentMetadata) -> Fixture {
    let download = Arc::new(RwLock::new(Download::from_metadata(metadata)));
    let hash = *download.read().unwrap().info_hash();

    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));
    let list = Arc::new(RwLock::new(list));

    let mut map = CommandMap::new();
    register_file_commands(&mut map, Arc::clone(&list)).unwrap();
    register_peer_commands(&mut map, Arc::clone(&list)).unwrap();
    register_tracker_commands(
        &mut map,
        Arc::new(RwLock::new(DhtConfig::new())),
        Arc::clone(&list),
    )
    .unwrap();

    Fixture {
        map,
        hash,
        download,
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
fn file_commands_use_the_storage_layer() {
    let fixture = setup();
    let target = Target::File(fixture.hash, 0);

    assert_eq!(
        call(&fixture.map, "f.path", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "dir/a.bin"
    );
    assert_eq!(
        call(&fixture.map, "f.offset", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "f.size_chunks", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    assert!(call(
        &fixture.map,
        "f.priority.set",
        &target,
        vec![Object::from(2i64)]
    )
    .is_ok());
    assert_eq!(
        call(&fixture.map, "f.priority", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        2
    );

    assert!(call(&fixture.map, "f.set_create_queued", &target, vec![]).is_ok());
    assert_eq!(
        call(&fixture.map, "f.is_create_queued", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert!(call(&fixture.map, "f.unset_create_queued", &target, vec![]).is_ok());
}

#[test]
fn peer_commands_inspect_and_disconnect() {
    let fixture = setup();
    let address = SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51413)));
    let peer_id = HashString::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();

    {
        let mut peer = PeerConnection::new_with_direction(
            peer_id,
            address.clone(),
            2,
            PeerDirection::Incoming,
            true,
        );
        peer.bitfield.set(0);
        peer.bitfield.set(1);
        peer.add_downloaded(1234);
        peer.add_uploaded(5678);
        fixture
            .download
            .write()
            .unwrap()
            .download_mut()
            .add_peer(peer);
    }

    let target = Target::Peer(fixture.hash, address);
    assert_eq!(
        call(&fixture.map, "p.is_encrypted", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "p.is_incoming", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "p.down_total", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1234
    );
    assert_eq!(
        call(&fixture.map, "p.up_total", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        5678
    );
    assert_eq!(
        call(&fixture.map, "p.is_seed", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    assert!(call(
        &fixture.map,
        "p.snubbed.set",
        &target,
        vec![Object::from(1i64)]
    )
    .is_ok());
    assert_eq!(
        call(&fixture.map, "p.is_snubbed", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    assert!(call(&fixture.map, "p.disconnect", &target, vec![]).is_ok());
    assert_eq!(
        call(&fixture.map, "p.is_encrypted", &target, vec![]).is_err(),
        true
    );
}

fn command_names(map: &CommandMap) -> Vec<String> {
    map.iter().map(|(name, _)| name.clone()).collect()
}

fn assert_commands(map: &CommandMap, expected: &[&str]) {
    let names = command_names(map);
    for name in expected {
        assert!(
            names.iter().any(|candidate| candidate.as_str() == *name),
            "missing command {name}"
        );
    }
}

fn parm_for<'a>(map: &'a CommandMap, name: &str) -> &'a str {
    map.iter()
        .find(|(key, _)| key.as_str() == name)
        .map(|(_, entry)| entry.parm.as_str())
        .unwrap_or_else(|| panic!("command {name} is not registered"))
}

#[test]
fn file_command_inventory_and_invalid_targets() {
    let fixture = setup();
    assert_commands(
        &fixture.map,
        &[
            "f.path",
            "f.path_components",
            "f.path_components.hex",
            "f.path_components.base64",
            "f.path_components.base64_as_binary",
            "f.path_components.as_binary",
            "f.path_components.or_base64",
            "f.path_components.or_as_binary",
            "f.path_depth",
            "f.frozen_path",
            "f.frozen_path.hex",
            "f.frozen_path.base64",
            "f.frozen_path.base64_as_binary",
            "f.frozen_path.as_binary",
            "f.frozen_path.or_base64",
            "f.frozen_path.or_as_binary",
            "f.frozen_path.realpath.or_empty",
            "f.frozen_path.realpath.or_throw",
            "f.size_bytes",
            "f.size_chunks",
            "f.completed_chunks",
            "f.offset",
            "f.range_first",
            "f.range_second",
            "f.priority",
            "f.priority.set",
            "f.priority_str",
            "f.is_created",
            "f.is_open",
            "f.is_create_queued",
            "f.create_queued",
            "f.is_resize_queued",
            "f.resize_queued",
            "f.set_create_queued",
            "f.unset_create_queued",
            "f.set_resize_queued",
            "f.unset_resize_queued",
            "f.prioritize_first",
            "f.prioritize_first.enable",
            "f.prioritize_first.disable",
            "f.prioritize_last",
            "f.prioritize_last.enable",
            "f.prioritize_last.disable",
            "f.last_touched",
            "f.match_depth_prev",
            "f.match_depth_next",
            "f.multicall",
        ],
    );

    assert_eq!(parm_for(&fixture.map, "f.priority.set"), "<val>");
    assert_eq!(parm_for(&fixture.map, "f.set_create_queued"), "<value>");
    assert_eq!(parm_for(&fixture.map, "f.unset_resize_queued"), "<value>");
    assert_eq!(parm_for(&fixture.map, "f.path"), "");

    let target = Target::File(fixture.hash, 0);
    let components = call(&fixture.map, "f.path_components", &target, vec![]).unwrap();
    let components: Vec<&str> = components
        .as_list()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap())
        .collect();
    assert_eq!(components, vec!["dir", "a.bin"]);
    assert_eq!(
        call(&fixture.map, "f.path_depth", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        2
    );
    assert_eq!(
        call(&fixture.map, "f.path_components.hex", &target, vec![])
            .unwrap()
            .as_list()
            .unwrap()[0]
            .as_str()
            .unwrap(),
        "646972"
    );
    assert_eq!(
        call(&fixture.map, "f.path_components.base64", &target, vec![])
            .unwrap()
            .as_list()
            .unwrap()[1]
            .as_str()
            .unwrap(),
        "YS5iaW4="
    );
    assert_eq!(
        call(
            &fixture.map,
            "f.offset",
            &Target::File(fixture.hash, 1),
            vec![]
        )
        .unwrap()
        .as_value()
        .unwrap(),
        16384
    );
    assert_eq!(
        call(&fixture.map, "f.range_first", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "f.range_second", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "f.last_touched", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "f.frozen_path", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "/dir/a.bin"
    );
    assert_eq!(
        call(&fixture.map, "f.frozen_path.hex", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "2f6469722f612e62696e"
    );
    let base64_binary = call(
        &fixture.map,
        "f.frozen_path.base64_as_binary",
        &target,
        vec![],
    )
    .unwrap();
    assert_eq!(base64_binary.as_str().unwrap(), "L2Rpci9hLmJpbg==");
    assert_ne!(base64_binary.flags & FLAG_BASE64, 0);
    assert_ne!(base64_binary.flags & FLAG_AS_BINARY, 0);

    // `f.is_created` follows libtorrent: padding files count as created,
    // regular files must exist, and directories are not created files.
    {
        let created_path =
            std::env::temp_dir().join(format!("rtorrent-rpc04-created-{}", fixture.hash.to_hex()));
        let _ = std::fs::remove_file(&created_path);
        let _ = std::fs::remove_dir_all(&created_path);
        fixture
            .download
            .write()
            .unwrap()
            .download_mut()
            .file_list_mut()
            .files_mut()[0]
            .set_frozen_path(created_path.to_string_lossy().into_owned());
        assert_eq!(
            call(&fixture.map, "f.is_created", &target, vec![])
                .unwrap()
                .as_value()
                .unwrap(),
            0
        );

        std::fs::create_dir_all(&created_path).unwrap();
        assert_eq!(
            call(&fixture.map, "f.is_created", &target, vec![])
                .unwrap()
                .as_value()
                .unwrap(),
            0
        );
        std::fs::remove_dir_all(&created_path).unwrap();

        std::fs::write(&created_path, b"created").unwrap();
        assert_eq!(
            call(&fixture.map, "f.is_created", &target, vec![])
                .unwrap()
                .as_value()
                .unwrap(),
            1
        );
        let _ = std::fs::remove_file(&created_path);

        fixture
            .download
            .write()
            .unwrap()
            .download_mut()
            .file_list_mut()
            .files_mut()[0]
            .set_flags(FileFlags::ATTR_PADDING);
        assert_eq!(
            call(&fixture.map, "f.is_created", &target, vec![])
                .unwrap()
                .as_value()
                .unwrap(),
            1
        );
        fixture
            .download
            .write()
            .unwrap()
            .download_mut()
            .file_list_mut()
            .files_mut()[0]
            .unset_flags(FileFlags::ATTR_PADDING);
    }
    {
        let missing =
            std::env::temp_dir().join(format!("rtorrent-rpc04-missing-{}", fixture.hash.to_hex()));
        assert!(!missing.exists());
        fixture
            .download
            .write()
            .unwrap()
            .download_mut()
            .file_list_mut()
            .files_mut()[0]
            .set_frozen_path(missing.to_string_lossy().into_owned());
    }
    assert_eq!(
        call(
            &fixture.map,
            "f.frozen_path.realpath.or_empty",
            &target,
            vec![]
        )
        .unwrap()
        .as_str()
        .unwrap(),
        ""
    );
    assert!(call(
        &fixture.map,
        "f.frozen_path.realpath.or_throw",
        &target,
        vec![]
    )
    .is_err());

    // Prioritize flags are independent from the numeric priority.
    call(
        &fixture.map,
        "f.priority.set",
        &target,
        vec![Object::from(2i64)],
    )
    .unwrap();
    call(&fixture.map, "f.prioritize_first.enable", &target, vec![]).unwrap();
    assert_eq!(
        call(&fixture.map, "f.prioritize_first", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "f.priority", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        2
    );
    call(&fixture.map, "f.prioritize_first.disable", &target, vec![]).unwrap();
    assert_eq!(
        call(&fixture.map, "f.prioritize_first", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    call(&fixture.map, "f.set_create_queued", &target, vec![]).unwrap();
    assert_eq!(
        call(&fixture.map, "f.create_queued", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    call(&fixture.map, "f.unset_create_queued", &target, vec![]).unwrap();
    assert_eq!(
        call(&fixture.map, "f.create_queued", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    assert!(call(
        &fixture.map,
        "f.priority.set",
        &target,
        vec![Object::from(9i64)]
    )
    .is_err());
    assert!(call(&fixture.map, "f.priority.set", &target, vec![]).is_err());
    assert!(call(
        &fixture.map,
        "f.priority.set",
        &target,
        vec![Object::from("high")]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "f.priority.set",
        &target,
        vec![Object::from(-1i64)]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "f.set_create_queued",
        &target,
        vec![Object::from("x")]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "f.set_resize_queued",
        &target,
        vec![Object::from("x")]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "f.offset",
        &Target::File(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "f.path",
        &Target::File(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "f.is_created",
        &Target::File(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "f.offset",
        &Target::Tracker(fixture.hash, 0),
        vec![]
    )
    .is_err());
    // File commands are strictly file-targeted; a download target must not
    // silently select file zero.
    assert!(call(
        &fixture.map,
        "f.offset",
        &Target::Download(fixture.hash),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "f.path",
        &Target::Download(fixture.hash),
        vec![]
    )
    .is_err());

    // The multicall map must be cloned after the extended surface is added.
    let mc = call(
        &fixture.map,
        "f.multicall",
        &Target::Download(fixture.hash),
        vec![Object::from("f.path_depth"), Object::from("f.frozen_path")],
    )
    .unwrap();
    let rows = mc.as_list().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].as_list().unwrap()[0].as_value().unwrap(), 2);

    // The legacy C++ form carries the download hash as the ignored first
    // argument.
    let mc = call(
        &fixture.map,
        "f.multicall",
        &Target::Download(fixture.hash),
        vec![
            Object::from(fixture.hash.to_hex()),
            Object::from("f.path_depth"),
        ],
    )
    .unwrap();
    let rows = mc.as_list().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].as_list().unwrap().len(), 1);
    assert_eq!(rows[0].as_list().unwrap()[0].as_value().unwrap(), 2);
}

#[test]
fn file_size_chunks_uses_the_piece_range() {
    let mut md = metadata();
    md.info_hash = HashString::from_hex("4343434343434343434343434343434343434343").unwrap();
    md.total_size = 24000;
    md.num_pieces = 2;
    md.pieces = vec![0u8; 40];
    let mut first = lt_common::path::Path::new();
    first.push("dir/a.bin");
    let mut second = lt_common::path::Path::new();
    second.push("dir/b.bin");
    md.files = vec![
        TorrentFileEntry {
            path: first,
            length: 12000,
            offset: 0,
        },
        TorrentFileEntry {
            path: second,
            length: 12000,
            offset: 12000,
        },
    ];

    let fixture = setup_with(md);
    let target = Target::File(fixture.hash, 1);

    // The second file starts inside chunk 0 and ends inside chunk 2, so it
    // touches two chunks even though ceil(12000 / 16384) == 1.
    assert_eq!(
        call(&fixture.map, "f.offset", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        12000
    );
    assert_eq!(
        call(&fixture.map, "f.range_first", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "f.range_second", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        2
    );
    assert_eq!(
        call(&fixture.map, "f.size_chunks", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        2
    );
}

#[test]
fn peer_command_inventory_and_invalid_targets() {
    let fixture = setup();
    assert_commands(
        &fixture.map,
        &[
            "p.address",
            "p.port",
            "p.client_version",
            "p.id",
            "p.id_html",
            "p.options_str",
            "p.listen_port",
            "p.queue_depth",
            "p.completed_percent",
            "p.up_rate",
            "p.up_total",
            "p.down_rate",
            "p.down_total",
            "p.peer_rate",
            "p.peer_total",
            "p.is_connected",
            "p.is_handshake",
            "p.is_blocked",
            "p.is_restart",
            "p.is_encrypted",
            "p.is_incoming",
            "p.is_outgoing",
            "p.is_choked",
            "p.is_interested",
            "p.is_obfuscated",
            "p.is_preferred",
            "p.is_snubbed",
            "p.snubbed",
            "p.is_banned",
            "p.banned",
            "p.is_unwanted",
            "p.is_seed",
            "p.failed_counter",
            "p.transfer_counter",
            "p.last_connection",
            "p.last_handshake",
            "p.snubbed.set",
            "p.preferred.set",
            "p.banned.set",
            "p.unwanted.set",
            "p.disconnect",
            "p.disconnect_delayed",
            "p.call_target",
            "p.multicall",
        ],
    );

    assert_eq!(
        parm_for(&fixture.map, "p.call_target"),
        "<hash>, <peer_id>, <command>, [args...]"
    );
    assert_eq!(parm_for(&fixture.map, "p.disconnect"), "");
    assert_eq!(parm_for(&fixture.map, "p.snubbed.set"), "<value>");

    let address = SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51413)));
    let peer_id = HashString::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    {
        let mut peer = PeerConnection::new_with_direction(
            peer_id,
            address.clone(),
            2,
            PeerDirection::Incoming,
            true,
        );
        peer.bitfield.set(0);
        peer.bitfield.set(1);
        peer.add_downloaded(1234);
        peer.add_uploaded(5678);
        peer.options = [0, 1, 2, 3, 4, 5, 0x10, 1];
        fixture
            .download
            .write()
            .unwrap()
            .download_mut()
            .add_peer(peer);
    }

    let target = Target::Peer(fixture.hash, address.clone());
    assert_eq!(
        call(&fixture.map, "p.id", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        peer_id.to_hex()
    );
    assert_eq!(
        call(&fixture.map, "p.id_html", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "%AA".repeat(20)
    );
    // Reserved handshake bytes are retained on PeerConnection/PeerInfo, so
    // p.options_str is a real mask rather than a placeholder.
    assert_eq!(
        call(&fixture.map, "p.options_str", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "0001020304051001"
    );
    assert_eq!(
        call(&fixture.map, "p.listen_port", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        51413
    );
    assert_eq!(
        call(&fixture.map, "p.queue_depth", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "p.is_connected", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "p.is_incoming", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "p.is_outgoing", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "p.is_obfuscated", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "p.completed_percent", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        100
    );
    assert_eq!(
        call(&fixture.map, "p.peer_total", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1234
    );
    assert!(
        call(&fixture.map, "p.peer_rate", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap()
            >= 0
    );
    assert_eq!(
        call(&fixture.map, "p.snubbed", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "p.banned", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "p.is_unwanted", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    call(
        &fixture.map,
        "p.preferred.set",
        &target,
        vec![Object::from(1i64)],
    )
    .unwrap();
    assert_eq!(
        call(&fixture.map, "p.is_preferred", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    // is_unwanted and is_banned are independent PeerInfo flags.
    call(
        &fixture.map,
        "p.unwanted.set",
        &target,
        vec![Object::from(1i64)],
    )
    .unwrap();
    assert_eq!(
        call(&fixture.map, "p.is_unwanted", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "p.banned", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    call(
        &fixture.map,
        "p.banned.set",
        &target,
        vec![Object::from(1i64)],
    )
    .unwrap();
    assert_eq!(
        call(&fixture.map, "p.banned", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "p.is_unwanted", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    call(
        &fixture.map,
        "p.banned.set",
        &target,
        vec![Object::from(0i64)],
    )
    .unwrap();
    assert_eq!(
        call(&fixture.map, "p.is_unwanted", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    call(
        &fixture.map,
        "p.unwanted.set",
        &target,
        vec![Object::from(0i64)],
    )
    .unwrap();
    assert_eq!(
        call(&fixture.map, "p.is_unwanted", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    assert!(call(
        &fixture.map,
        "p.snubbed.set",
        &target,
        vec![Object::from("not-a-number")]
    )
    .is_err());
    assert!(call(&fixture.map, "p.snubbed.set", &target, vec![]).is_err());
    assert!(call(&fixture.map, "p.preferred.set", &target, vec![]).is_err());
    assert!(call(&fixture.map, "p.banned.set", &target, vec![]).is_err());
    assert!(call(&fixture.map, "p.unwanted.set", &target, vec![]).is_err());
    assert!(call(
        &fixture.map,
        "p.id",
        &Target::Download(fixture.hash),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "p.queue_depth",
        &Target::Download(fixture.hash),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "p.address",
        &Target::Download(fixture.hash),
        vec![]
    )
    .is_err());

    let unknown = SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 12345)));
    let unknown_target = Target::Peer(fixture.hash, unknown);
    assert!(call(&fixture.map, "p.id", &unknown_target, vec![]).is_err());
    assert!(call(&fixture.map, "p.address", &unknown_target, vec![]).is_err());
    assert!(call(&fixture.map, "p.port", &unknown_target, vec![]).is_err());
    assert!(call(&fixture.map, "p.queue_depth", &unknown_target, vec![]).is_err());
    assert!(call(&fixture.map, "p.disconnect", &unknown_target, vec![]).is_err());

    // p.call_target dispatches by peer id rather than by address.
    let hash_hex = fixture.hash.to_hex();
    let peer_id_hex = peer_id.to_hex();
    assert_eq!(
        call(
            &fixture.map,
            "p.call_target",
            &Target::None,
            vec![
                Object::from(hash_hex.clone()),
                Object::from(peer_id_hex.clone()),
                Object::from("p.down_total"),
            ],
        )
        .unwrap()
        .as_value()
        .unwrap(),
        1234
    );
    call(
        &fixture.map,
        "p.call_target",
        &Target::None,
        vec![
            Object::from(hash_hex.clone()),
            Object::from(peer_id_hex.clone()),
            Object::from("p.snubbed.set"),
            Object::from(1i64),
        ],
    )
    .unwrap();
    assert_eq!(
        call(&fixture.map, "p.is_snubbed", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert!(call(
        &fixture.map,
        "p.call_target",
        &Target::None,
        vec![
            Object::from(hash_hex.clone()),
            Object::from(peer_id_hex.clone())
        ],
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "p.call_target",
        &Target::None,
        vec![
            Object::from("not-a-hash"),
            Object::from(peer_id_hex.clone()),
            Object::from("p.id"),
        ],
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "p.call_target",
        &Target::None,
        vec![
            Object::from(hash_hex.clone()),
            Object::from("deadbeef"),
            Object::from("p.id"),
        ],
    )
    .is_err());
    let other_peer_id = HashString::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
    assert!(call(
        &fixture.map,
        "p.call_target",
        &Target::None,
        vec![
            Object::from(hash_hex.clone()),
            Object::from(other_peer_id.to_hex()),
            Object::from("p.id"),
        ],
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "p.call_target",
        &Target::None,
        vec![
            Object::from(hash_hex.clone()),
            Object::from(peer_id_hex.clone()),
            Object::from("p.does_not_exist"),
        ],
    )
    .is_err());

    let mc = call(
        &fixture.map,
        "p.multicall",
        &Target::Download(fixture.hash),
        vec![
            Object::from("p.options_str"),
            Object::from("p.peer_total"),
            Object::from("p.queue_depth"),
        ],
    )
    .unwrap();
    let rows = mc.as_list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].as_list().unwrap()[0].as_str().unwrap(),
        "0001020304051001"
    );
    assert_eq!(rows[0].as_list().unwrap()[2].as_value().unwrap(), 0);

    // Both the C++ form (hash first) and the target-oriented form work.
    let mc = call(
        &fixture.map,
        "p.multicall",
        &Target::Download(fixture.hash),
        vec![Object::from(hash_hex), Object::from("p.down_total")],
    )
    .unwrap();
    let rows = mc.as_list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].as_list().unwrap()[0].as_value().unwrap(), 1234);

    call(&fixture.map, "p.disconnect", &target, vec![]).unwrap();
    assert!(call(&fixture.map, "p.id", &target, vec![]).is_err());
    assert!(call(&fixture.map, "p.disconnect", &target, vec![]).is_err());
}

#[test]
fn namespace_commands_reject_wrong_targets_and_missing_values() {
    let fixture = setup();
    let download_target = Target::Download(fixture.hash);
    let file_target = Target::File(fixture.hash, 0);
    let peer_target = Target::Peer(
        fixture.hash,
        SocketAddress::from(SocketAddr::from((Ipv4Addr::LOCALHOST, 51413))),
    );
    let tracker_target = Target::Tracker(fixture.hash, 0);

    // Every file/peer/tracker command validates its target namespace. Arity
    // errors are acceptable for the multicall dispatchers, which are also
    // intentionally excluded from direct invocation by the loop below.
    for name in command_names(&fixture.map) {
        if !(name.starts_with("f.") || name.starts_with("p.") || name.starts_with("t.")) {
            continue;
        }
        if name.ends_with(".multicall") || name == "p.call_target" {
            continue;
        }
        assert!(
            call(&fixture.map, &name, &download_target, vec![]).is_err(),
            "{name} accepted a Download target"
        );
    }

    for name in ["f.priority.set"] {
        assert!(
            call(&fixture.map, name, &file_target, vec![]).is_err(),
            "{name} accepted a missing value"
        );
    }
    for name in [
        "p.snubbed.set",
        "p.preferred.set",
        "p.banned.set",
        "p.unwanted.set",
    ] {
        assert!(
            call(&fixture.map, name, &peer_target, vec![]).is_err(),
            "{name} accepted a missing value without a connected peer"
        );
    }
    for name in [
        "t.is_enabled.set",
        "t.min_interval.set",
        "t.normal_interval.set",
    ] {
        assert!(
            call(&fixture.map, name, &tracker_target, vec![]).is_err(),
            "{name} accepted a missing value"
        );
    }
}

#[test]
fn tracker_commands_read_and_toggle_state() {
    let fixture = setup();
    let target = Target::Tracker(fixture.hash, 0);

    assert_eq!(
        call(&fixture.map, "t.is_enabled", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "t.group", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert!(
        call(&fixture.map, "t.normal_interval", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap()
            > 0
    );
    assert_eq!(
        call(&fixture.map, "t.id", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );

    // t.is_open follows is_requesting, unlike t.is_usable/is_in_use.
    {
        let mut download = fixture.download.write().unwrap();
        let mut engine = download.download_mut();
        let tracker = engine
            .tracker_controller_mut()
            .tracker_list
            .at_mut(0)
            .unwrap();
        tracker.state_mut().flags.insert(TrackerFlags::REQUESTING);
        tracker.state_mut().latest_event = TrackerEvent::Started;
    }
    assert_eq!(
        call(&fixture.map, "t.is_open", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "t.is_busy", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    // The starting-request flag also counts as requesting in libtorrent.
    {
        let mut download = fixture.download.write().unwrap();
        let mut engine = download.download_mut();
        let tracker = engine
            .tracker_controller_mut()
            .tracker_list
            .at_mut(0)
            .unwrap();
        tracker.state_mut().flags.remove(TrackerFlags::REQUESTING);
        tracker
            .state_mut()
            .flags
            .insert(TrackerFlags::STARTING_REQUEST);
    }
    assert_eq!(
        call(&fixture.map, "t.is_open", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "t.is_busy", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    {
        let mut download = fixture.download.write().unwrap();
        let mut engine = download.download_mut();
        let tracker = engine
            .tracker_controller_mut()
            .tracker_list
            .at_mut(0)
            .unwrap();
        let state = tracker.state_mut();
        state.flags.remove(TrackerFlags::REQUESTING);
        state.flags.remove(TrackerFlags::STARTING_REQUEST);
        state.latest_event = TrackerEvent::Completed;
        state.latest_new_peers = 5;
        state.latest_sum_peers = 9;
        state.counters.success_time_last = 100;
        state.counters.success_counter = 2;
        state.counters.failed_time_last = 50;
        state.counters.failed_counter = 3;
        state.counters.scrape_time_last = 25;
        state.counters.scrape_counter = 4;
        state.scrape_complete = 6;
        state.scrape_incomplete = 7;
        state.scrape_downloaded = 8;
        state.min_interval = 300;
        state.normal_interval = 600;
    }
    assert_eq!(
        call(&fixture.map, "t.is_open", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "t.latest_event", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "t.latest_new_peers", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        5
    );
    assert_eq!(
        call(&fixture.map, "t.latest_sum_peers", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        9
    );
    assert_eq!(
        call(&fixture.map, "t.success_time_last", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        100
    );
    assert_eq!(
        call(&fixture.map, "t.success_time_next", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        700
    );
    assert_eq!(
        call(&fixture.map, "t.success_counter", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        2
    );
    assert_eq!(
        call(&fixture.map, "t.failed_time_last", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        50
    );
    assert_eq!(
        call(&fixture.map, "t.failed_time_next", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        70
    );
    assert_eq!(
        call(&fixture.map, "t.failed_counter", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        3
    );
    assert_eq!(
        call(&fixture.map, "t.activity_time_last", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        50
    );
    assert_eq!(
        call(&fixture.map, "t.activity_time_next", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        70
    );
    assert_eq!(
        call(&fixture.map, "t.scrape_time_last", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        25
    );
    assert_eq!(
        call(&fixture.map, "t.scrape_counter", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        4
    );
    assert_eq!(
        call(&fixture.map, "t.scrape_complete", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        6
    );
    assert_eq!(
        call(&fixture.map, "t.scrape_incomplete", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        7
    );
    assert_eq!(
        call(&fixture.map, "t.scrape_downloaded", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        8
    );
    // The successful scrape was at 25s; normal_interval (600) dominates.
    assert_eq!(
        call(&fixture.map, "t.scrape_time_next", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        625
    );
    assert_eq!(
        call(&fixture.map, "t.scrape_time_next_minimum", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        625
    );
    assert_eq!(
        call(&fixture.map, "t.scrape_failed_time_last", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "t.scrape_failed_counter", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "t.is_in_use", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&fixture.map, "t.is_deleted", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    assert!(call(&fixture.map, "t.disable", &target, vec![]).is_ok());
    assert_eq!(
        call(&fixture.map, "t.is_enabled", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert!(call(&fixture.map, "t.enable", &target, vec![]).is_ok());
}

#[test]
fn tracker_command_inventory_and_invalid_targets() {
    let fixture = setup();
    assert_commands(
        &fixture.map,
        &[
            "t.url",
            "t.type",
            "t.id",
            "t.is_busy",
            "t.is_deleted",
            "t.is_disownable",
            "t.is_enabled",
            "t.is_enabled.set",
            "t.is_extra_tracker",
            "t.is_in_use",
            "t.is_open",
            "t.is_scrapable",
            "t.is_usable",
            "t.can_scrape",
            "t.enable",
            "t.disable",
            "t.group",
            "t.latest_event",
            "t.latest_new_peers",
            "t.latest_sum_peers",
            "t.normal_interval",
            "t.normal_interval.set",
            "t.min_interval",
            "t.min_interval.set",
            "t.activity_time_next",
            "t.activity_time_last",
            "t.success_time_next",
            "t.success_time_last",
            "t.success_counter",
            "t.failed_time_next",
            "t.failed_time_last",
            "t.failed_counter",
            "t.scrape_time_last",
            "t.scrape_time_next",
            "t.scrape_time_next_minimum",
            "t.scrape_failed_time_last",
            "t.scrape_failed_counter",
            "t.scrape_counter",
            "t.scrape_complete",
            "t.scrape_incomplete",
            "t.scrape_downloaded",
            "t.multicall",
        ],
    );

    assert_eq!(parm_for(&fixture.map, "t.id"), "");
    assert_eq!(parm_for(&fixture.map, "t.enable"), "");
    assert_eq!(parm_for(&fixture.map, "t.is_enabled.set"), "<value>");
    assert_eq!(parm_for(&fixture.map, "t.min_interval.set"), "<value>");
    assert_eq!(parm_for(&fixture.map, "t.normal_interval.set"), "<value>");

    let target = Target::Tracker(fixture.hash, 0);
    assert_eq!(
        call(&fixture.map, "t.url", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "http://tracker-a/announce"
    );
    assert_eq!(
        call(&fixture.map, "t.type", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(
            &fixture.map,
            "t.url",
            &Target::Tracker(fixture.hash, 1),
            vec![]
        )
        .unwrap()
        .as_str()
        .unwrap(),
        "udp://tracker-b:6969/announce"
    );
    assert_eq!(
        call(
            &fixture.map,
            "t.type",
            &Target::Tracker(fixture.hash, 1),
            vec![]
        )
        .unwrap()
        .as_value()
        .unwrap(),
        2
    );
    assert_eq!(
        call(&fixture.map, "t.is_scrapable", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        call(&fixture.map, "t.can_scrape", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap()
    );
    assert_eq!(
        call(&fixture.map, "t.group", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "t.latest_event", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&fixture.map, "t.activity_time_last", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert!(
        call(&fixture.map, "t.failed_time_next", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap()
            >= 0
    );

    call(
        &fixture.map,
        "t.min_interval.set",
        &target,
        vec![Object::from(100i64)],
    )
    .unwrap();
    assert_eq!(
        call(&fixture.map, "t.min_interval", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        300
    );
    call(
        &fixture.map,
        "t.normal_interval.set",
        &target,
        vec![Object::from(100i64)],
    )
    .unwrap();
    assert_eq!(
        call(&fixture.map, "t.normal_interval", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        600
    );

    call(
        &fixture.map,
        "t.is_enabled.set",
        &target,
        vec![Object::from(0i64)],
    )
    .unwrap();
    assert_eq!(
        call(&fixture.map, "t.is_enabled", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    call(
        &fixture.map,
        "t.is_enabled.set",
        &target,
        vec![Object::from(1i64)],
    )
    .unwrap();
    assert_eq!(
        call(&fixture.map, "t.is_enabled", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    assert!(call(
        &fixture.map,
        "t.url",
        &Target::Tracker(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "t.type",
        &Target::Tracker(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "t.id",
        &Target::Tracker(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "t.group",
        &Target::Tracker(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "t.can_scrape",
        &Target::Tracker(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "t.is_in_use",
        &Target::Tracker(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "t.scrape_time_next",
        &Target::Tracker(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "t.disable",
        &Target::Tracker(fixture.hash, 99),
        vec![]
    )
    .is_err());
    assert!(call(
        &fixture.map,
        "t.url",
        &Target::Download(fixture.hash),
        vec![]
    )
    .is_err());
    assert!(call(&fixture.map, "t.is_enabled.set", &target, vec![]).is_err());
    assert!(call(&fixture.map, "t.min_interval.set", &target, vec![]).is_err());
    assert!(call(&fixture.map, "t.normal_interval.set", &target, vec![]).is_err());
    assert!(call(
        &fixture.map,
        "t.multicall",
        &Target::Download(fixture.hash),
        vec![]
    )
    .is_err());

    let mc = call(
        &fixture.map,
        "t.multicall",
        &Target::Download(fixture.hash),
        vec![
            Object::from("t.min_interval"),
            Object::from("t.group"),
            Object::from("t.id"),
        ],
    )
    .unwrap();
    let rows = mc.as_list().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].as_list().unwrap().len(), 3);
    assert_eq!(rows[0].as_list().unwrap()[2].as_str().unwrap(), "");
    assert_eq!(rows[1].as_list().unwrap()[1].as_value().unwrap(), 1);

    // Legacy form: a leading hash is ignored, not returned as a column.
    let mc = call(
        &fixture.map,
        "t.multicall",
        &Target::Download(fixture.hash),
        vec![Object::from(fixture.hash.to_hex()), Object::from("t.url")],
    )
    .unwrap();
    let rows = mc.as_list().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].as_list().unwrap().len(), 1);
    assert_eq!(
        rows[1].as_list().unwrap()[0].as_str().unwrap(),
        "udp://tracker-b:6969/announce"
    );
}
