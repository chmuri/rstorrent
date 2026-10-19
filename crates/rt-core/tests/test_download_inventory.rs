use std::sync::{Arc, RwLock};

use lt_common::hash_string::HashString;
use lt_common::object::flags::{FLAG_AS_BINARY, FLAG_BASE64, FLAG_HEX};
use lt_common::Object;
use lt_engine::constructor::{TorrentFileEntry, TorrentMetadata};
use rt_core::commands::download::register_download_commands;
use rt_core::download::Download;
use rt_core::download_list::DownloadList;
use rt_core::view_manager::ViewManager;
use rt_rpc::{CommandMap, RpcError, Target};

fn metadata() -> TorrentMetadata {
    let info_hash = HashString::from_hex("5151515151515151515151515151515151515151").unwrap();
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
    main.set_visible(&hash, None);
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
) -> Result<Object, RpcError> {
    let args = Object::create_list_with(args);
    map.call_command(name, &args, target, true)
}

fn parm_for<'a>(map: &'a CommandMap, name: &str) -> &'a str {
    map.iter()
        .find(|(key, _)| key.as_str() == name)
        .map(|(_, entry)| entry.parm.as_str())
        .unwrap_or_else(|| panic!("command {name} is not registered"))
}

#[test]
fn inventory_contains_new_download_commands_and_arities() {
    let (map, _target, _) = setup();

    let names = [
        "d.name.hex",
        "d.name.base64",
        "d.name.base64_as_binary",
        "d.name.as_binary",
        "d.name.or_base64",
        "d.name.or_as_binary",
        "d.base_path.hex",
        "d.base_path.base64",
        "d.base_path.base64_as_binary",
        "d.base_path.as_binary",
        "d.base_path.or_base64",
        "d.base_path.or_as_binary",
        "d.base_path.realpath.or_empty",
        "d.base_path.realpath.or_throw",
        "d.base_filename.hex",
        "d.base_filename.base64",
        "d.base_filename.base64_as_binary",
        "d.base_filename.as_binary",
        "d.base_filename.or_base64",
        "d.base_filename.or_as_binary",
        "d.directory.realpath.or_empty",
        "d.directory.realpath.or_throw",
        "d.loaded_file.realpath.or_empty",
        "d.loaded_file.realpath.or_throw",
        "d.loaded_file.set",
        "d.tied_to_file",
        "d.tied_to_file.set",
        "d.tied_to_file.realpath.or_empty",
        "d.tied_to_file.realpath.or_throw",
        "d.delete_tied",
        "d.bitfield",
        "d.chunks_seen",
        "d.chunks_hashed",
        "d.free_diskspace",
        "d.max_file_size",
        "d.max_file_size.set",
        "d.max_size_pex",
        "d.max_size_pex.set",
        "d.peer_exchange",
        "d.peer_exchange.set",
        "d.size_pex",
        "d.skip.rate",
        "d.skip.total",
        "d.hashing",
        "d.hashing_failed",
        "d.hashing_failed.set",
        "d.uploads_max",
        "d.uploads_max.set",
        "d.downloads_max",
        "d.downloads_max.set",
        "d.uploads_min",
        "d.uploads_min.set",
        "d.downloads_min",
        "d.downloads_min.set",
        "d.save_resume",
        "d.save_full_session",
        "d.update_priorities",
        "d.disconnect.seeders",
        "d.create_link",
        "d.delete_link",
        "d.tracker.has_active",
        "d.tracker.has_active_not_scrape",
        "d.tracker.has_usable",
        "d.tracker.insert",
        "d.tracker.send_scrape",
        "d.tracker_announce",
        "d.tracker_announce.force",
        "d.tracker_numwant.set",
        "d.tracker_focus",
        "d.multicall",
        "d.multicall2",
        "d.multicall.filtered",
        "d.creation_date",
        "d.local_id",
        "d.local_id_html",
        "d.group.name",
        "d.hashing.set",
        "d.tracker_url",
        "d.tracker_type",
        "d.tracker_group",
        "d.tracker_is_enabled",
        "d.tracker_is_usable",
        "d.tracker_is_extra",
        "d.complete.set",
        "d.connection_leech.set",
        "d.mode",
        "d.mode.set",
        "d.ignore_commands",
        "d.ignore_commands.set",
        "d.custom_throw",
        "d.custom.if_z",
        "d.custom.keys",
        "d.custom.items",
        "d.up.choke_heuristics",
        "d.up.choke_heuristics.set",
        "d.up.choke_heuristics.leech",
        "d.up.choke_heuristics.leech.set",
        "d.up.choke_heuristics.seed",
        "d.up.choke_heuristics.seed.set",
        "d.down.choke_heuristics",
        "d.down.choke_heuristics.set",
        "d.down.choke_heuristics.leech",
        "d.down.choke_heuristics.leech.set",
        "d.down.choke_heuristics.seed",
        "d.down.choke_heuristics.seed.set",
        "d.timestamp.started.elapsed",
        "d.timestamp.finished.elapsed",
        "d.directory.base.set",
        "d.directory.hex",
        "d.directory.base64",
        "d.directory.as_binary",
        "d.directory_base.hex",
        "d.directory_base.or_base64",
        "d.loaded_file.hex",
        "d.loaded_file.base64_as_binary",
        "d.loaded_file.or_as_binary",
        "d.tied_to_file.hex",
        "d.tied_to_file.base64",
        "d.tied_to_file.or_base64",
    ];

    for name in names {
        assert!(map.has(name), "missing command {name}");
    }

    assert_eq!(parm_for(&map, "d.max_file_size.set"), "<value>");
    assert_eq!(parm_for(&map, "d.tracker_numwant.set"), "<value>");
    assert_eq!(
        parm_for(&map, "d.tracker.insert"),
        "<group>, <url>"
    );
    assert_eq!(
        parm_for(&map, "d.create_link"),
        "<type>, <prefix>, <postfix>"
    );
    assert_eq!(
        parm_for(&map, "d.delete_link"),
        "<type>, <prefix>, <postfix>"
    );
    assert_eq!(parm_for(&map, "d.save_resume"), "");
    assert_eq!(parm_for(&map, "d.update_priorities"), "");
    assert_eq!(parm_for(&map, "d.mode.set"), "<value>");
    assert_eq!(parm_for(&map, "d.ignore_commands.set"), "<value>");
    assert_eq!(parm_for(&map, "d.hashing.set"), "<value>");
    assert_eq!(parm_for(&map, "d.local_id"), "");
    assert_eq!(parm_for(&map, "d.local_id_html"), "");
    assert_eq!(parm_for(&map, "d.group.name"), "");
    assert_eq!(parm_for(&map, "d.tracker_url"), "<index>");
    assert_eq!(parm_for(&map, "d.tracker_type"), "<index>");
    assert_eq!(parm_for(&map, "d.tracker_group"), "<index>");
    assert_eq!(parm_for(&map, "d.tracker_is_enabled"), "<index>");
    assert_eq!(parm_for(&map, "d.tracker_is_usable"), "<index>");
    assert_eq!(parm_for(&map, "d.tracker_is_extra"), "<index>");
    assert_eq!(parm_for(&map, "d.custom.if_z"), "<key>, <default>");
    assert_eq!(
        parm_for(&map, "d.multicall.filtered"),
        "<view>, <filter>, <commands...>"
    );
}

#[test]
fn binary_path_variants_preserve_bytes_and_flags() {
    let (map, target, _) = setup();

    let plain = call(&map, "d.name", &target, vec![]).unwrap();
    assert_eq!(plain.as_str().unwrap(), "extended");

    let hex = call(&map, "d.name.hex", &target, vec![]).unwrap();
    assert_eq!(hex.as_str().unwrap(), "657874656e646564");
    assert_ne!(hex.flags & FLAG_HEX, 0);

    let base64 = call(&map, "d.name.base64", &target, vec![]).unwrap();
    assert_eq!(base64.as_str().unwrap(), "ZXh0ZW5kZWQ=");
    assert_ne!(base64.flags & FLAG_BASE64, 0);

    let base64_binary = call(&map, "d.name.base64_as_binary", &target, vec![]).unwrap();
    assert_eq!(base64_binary.as_str().unwrap(), "ZXh0ZW5kZWQ=");
    assert_ne!(base64_binary.flags & FLAG_BASE64, 0);
    assert_ne!(base64_binary.flags & FLAG_AS_BINARY, 0);

    let binary = call(&map, "d.name.as_binary", &target, vec![]).unwrap();
    assert_eq!(binary.as_string().unwrap(), &b"extended"[..]);
    assert_ne!(binary.flags & FLAG_AS_BINARY, 0);

    let base_path = call(&map, "d.base_path", &target, vec![]).unwrap();
    assert_eq!(base_path.as_str().unwrap(), "/tmp/extended/extended");
    let base_path_hex = call(&map, "d.base_path.hex", &target, vec![]).unwrap();
    assert_eq!(
        base_path_hex.as_str().unwrap(),
        "2f746d702f657874656e6465642f657874656e646564"
    );

    let base_filename = call(&map, "d.base_filename", &target, vec![]).unwrap();
    assert_eq!(base_filename.as_str().unwrap(), "extended");
    assert_eq!(
        call(&map, "d.base_filename.or_base64", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "extended"
    );
    assert_eq!(
        call(&map, "d.base_filename.or_as_binary", &target, vec![])
            .unwrap()
            .as_string()
            .unwrap(),
        &b"extended"[..]
    );

    // Directory and tied/loaded path variants use the same byte-preserving
    // encoders.
    assert_eq!(
        call(&map, "d.directory.hex", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "2f746d702f657874656e646564"
    );
    assert_eq!(
        call(&map, "d.directory_base.base64", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "L3RtcC9leHRlbmRlZA=="
    );
    assert_eq!(
        call(&map, "d.tied_to_file.or_base64", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );

    call(
        &map,
        "d.loaded_file.set",
        &target,
        vec![Object::from("/tmp/loaded.torrent")],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.loaded_file.hex", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "2f746d702f6c6f616465642e746f7272656e74"
    );
    assert_ne!(
        call(&map, "d.loaded_file.as_binary", &target, vec![])
            .unwrap()
            .flags
            & FLAG_AS_BINARY,
        0
    );
}

#[test]
fn realpath_variants_distinguish_or_empty_and_or_throw() {
    let (map, target, _) = setup();

    // /tmp/extended/extended does not exist by default.
    assert_eq!(
        call(&map, "d.base_path.realpath.or_empty", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );
    assert!(call(&map, "d.base_path.realpath.or_throw", &target, vec![]).is_err());
    assert_eq!(
        call(&map, "d.loaded_file.realpath.or_empty", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );
    assert!(call(&map, "d.loaded_file.realpath.or_throw", &target, vec![]).is_err());
    assert_eq!(
        call(&map, "d.tied_to_file.realpath.or_empty", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );
    assert!(call(&map, "d.tied_to_file.realpath.or_throw", &target, vec![]).is_err());

    let temp = std::env::temp_dir().join(format!("rtorrent-rpc03-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let temp_str = temp.to_str().unwrap().to_string();

    call(&map, "d.directory.set", &target, vec![Object::from(temp_str.clone())]).unwrap();
    let resolved_dir = call(&map, "d.directory.realpath.or_throw", &target, vec![]).unwrap();
    assert_eq!(
        resolved_dir.as_str().unwrap(),
        std::fs::canonicalize(&temp).unwrap().to_str().unwrap()
    );

    let tied = temp.join("tied.txt");
    std::fs::write(&tied, b"tied").unwrap();
    call(
        &map,
        "d.tied_to_file.set",
        &target,
        vec![Object::from(tied.to_str().unwrap())],
    )
    .unwrap();
    let tied_resolved = call(&map, "d.tied_to_file.realpath.or_throw", &target, vec![])
        .unwrap();
    assert_eq!(
        tied_resolved.as_str().unwrap(),
        std::fs::canonicalize(&tied).unwrap().to_str().unwrap()
    );

    call(&map, "d.delete_tied", &target, vec![]).unwrap();
    assert!(!tied.exists());
    assert_eq!(
        call(&map, "d.tied_to_file", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn new_getters_and_setters_round_trip() {
    let (map, target, _) = setup();

    let bitfield = call(&map, "d.bitfield", &target, vec![]).unwrap();
    assert_eq!(bitfield.as_str().unwrap().len(), 2);
    let seen = call(&map, "d.chunks_seen", &target, vec![]).unwrap();
    assert_eq!(seen.as_str().unwrap().len(), 2);
    assert_eq!(
        call(&map, "d.chunks_hashed", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&map, "d.wanted_chunks", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&map, "d.bytes_done", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&map, "d.completed_bytes", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&map, "d.left_bytes", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        16384
    );
    assert_eq!(
        call(&map, "d.ratio", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert!(call(&map, "d.free_diskspace", &target, vec![])
        .unwrap()
        .as_value()
        .unwrap()
        >= 0);

    assert!(call(&map, "d.max_file_size.set", &target, vec![Object::from(2048i64)]).is_ok());
    assert_eq!(
        call(&map, "d.max_file_size", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        2048
    );
    assert!(call(&map, "d.max_file_size.set", &target, vec![Object::from(-1i64)]).is_err());

    assert_eq!(
        call(&map, "d.peer_exchange", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    call(&map, "d.peer_exchange.set", &target, vec![Object::from(0i64)]).unwrap();
    assert_eq!(
        call(&map, "d.peer_exchange", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    assert_eq!(
        call(&map, "d.max_size_pex", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        8
    );
    call(&map, "d.max_size_pex.set", &target, vec![Object::from(64i64)]).unwrap();
    assert_eq!(
        call(&map, "d.max_size_pex", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        64
    );

    assert_eq!(
        call(&map, "d.skip.rate", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&map, "d.skip.total", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    call(&map, "d.hashing_failed.set", &target, vec![Object::from(1i64)]).unwrap();
    assert_eq!(
        call(&map, "d.hashing_failed", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&map, "d.hashing", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    call(&map, "d.uploads_max.set", &target, vec![Object::from(25i64)]).unwrap();
    assert_eq!(
        call(&map, "d.uploads_max", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        25
    );
    call(&map, "d.downloads_max.set", &target, vec![Object::from(30i64)]).unwrap();
    assert_eq!(
        call(&map, "d.downloads_max", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        30
    );
    call(&map, "d.uploads_min.set", &target, vec![Object::from(3i64)]).unwrap();
    assert_eq!(
        call(&map, "d.uploads_min", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        3
    );
    call(&map, "d.downloads_min.set", &target, vec![Object::from(4i64)]).unwrap();
    assert_eq!(
        call(&map, "d.downloads_min", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        4
    );

    call(&map, "d.priority.set", &target, vec![Object::from("3")]).unwrap();
    assert_eq!(
        call(&map, "d.priority_str", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "high"
    );
    assert!(call(&map, "d.priority.set", &target, vec![Object::from("4")]).is_err());

    // The standalone registration has no session-manager handle, so these
    // must fail loudly instead of pretending a resume/session was saved.
    let save_resume = call(&map, "d.save_resume", &target, vec![])
        .err()
        .expect("d.save_resume should fail without a session manager");
    assert!(
        save_resume.to_string().contains("session"),
        "unexpected error: {save_resume}"
    );
    let save_full = call(&map, "d.save_full_session", &target, vec![])
        .err()
        .expect("d.save_full_session should fail without a session manager");
    assert!(
        save_full.to_string().contains("session"),
        "unexpected error: {save_full}"
    );
    assert!(call(&map, "d.update_priorities", &target, vec![]).is_ok());
    assert!(call(&map, "d.disconnect.seeders", &target, vec![]).is_ok());

    // Target typing is preserved: these are not callable without a download.
    assert!(call(&map, "d.max_file_size", &Target::None, vec![]).is_err());
}

#[test]
fn link_commands_create_and_remove_symlinks() {
    let (map, target, _) = setup();

    let temp = std::env::temp_dir().join(format!("rtorrent-rpc03-links-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let link = temp.join("link-extended");
    let _ = std::fs::remove_file(&link);

    let prefix = format!("{}/link-", temp.to_str().unwrap());
    let args = vec![
        Object::from("base_filename"),
        Object::from(prefix),
        Object::from(""),
    ];

    call(&map, "d.create_link", &target, args.clone()).unwrap();
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());

    call(&map, "d.delete_link", &target, args).unwrap();
    assert!(std::fs::symlink_metadata(&link).is_err());

    // Invalid argument counts are rejected.
    assert!(call(
        &map,
        "d.create_link",
        &target,
        vec![Object::from("base_filename")]
    )
    .is_err());

    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn tracker_commands_round_trip() {
    let (map, target, _) = setup();

    assert_eq!(
        call(&map, "d.tracker_numwant", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        -1
    );
    call(
        &map,
        "d.tracker_numwant.set",
        &target,
        vec![Object::from(25i64)],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.tracker_numwant", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        25
    );

    let before = call(&map, "d.tracker_size", &target, vec![])
        .unwrap()
        .as_value()
        .unwrap();
    call(
        &map,
        "d.tracker.insert",
        &target,
        vec![
            Object::from(0i64),
            Object::from("http://extra.invalid/announce"),
        ],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.tracker_size", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        before + 1
    );
    assert!(call(&map, "d.tracker.insert", &target, vec![Object::from(0i64)]).is_err());
    assert!(call(
        &map,
        "d.tracker.insert",
        &target,
        vec![Object::from(33i64), Object::from("http://bad/announce")]
    )
    .is_err());

    assert!(call(&map, "d.tracker.has_active", &target, vec![]).is_ok());
    assert!(call(&map, "d.tracker.has_active_not_scrape", &target, vec![]).is_ok());
    assert!(call(&map, "d.tracker.has_usable", &target, vec![]).is_ok());
    assert!(call(&map, "d.tracker.send_scrape", &target, vec![Object::from(1i64)]).is_ok());
    assert!(call(&map, "d.tracker_announce", &target, vec![]).is_ok());
    assert!(call(&map, "d.tracker_announce.force", &target, vec![]).is_ok());
}

#[test]
fn multicall_covers_new_getters() {
    let (map, _target, _) = setup();

    let args = Object::create_list_with(vec![
        Object::from("main"),
        Object::from("d.name"),
        Object::from("d.chunks_seen"),
        Object::from("d.free_diskspace"),
        Object::from("d.mode"),
        Object::from("d.creation_date"),
        Object::from("d.ignore_commands"),
        Object::from("d.up.choke_heuristics"),
        Object::from("d.tracker_focus"),
        Object::from("d.local_id"),
        Object::from("d.group.name"),
        Object::from("d.hashing"),
        Object::from("d.tracker_url=0"),
    ]);

    let rows = map
        .call_command("d.multicall2", &args, &Target::None, true)
        .unwrap();
    let rows = rows.as_list().unwrap();
    assert_eq!(rows.len(), 1);
    let row = rows[0].as_list().unwrap();
    assert_eq!(row.len(), 12);
    assert_eq!(row[0].as_str().unwrap(), "extended");
    assert_eq!(row[1].as_str().unwrap().len(), 2);
    assert!(row[2].as_value().unwrap() >= 0);
    assert_eq!(row[3].as_value().unwrap(), 0);
    assert_eq!(row[4].as_value().unwrap(), 0);
    assert_eq!(row[5].as_value().unwrap(), 0);
    assert_eq!(row[6].as_str().unwrap(), "upload_leech");
    assert_eq!(row[7].as_value().unwrap(), 1);
    assert_eq!(row[8].as_str().unwrap(), "");
    assert_eq!(row[9].as_str().unwrap(), "global");
    assert_eq!(row[10].as_value().unwrap(), 0);
    assert_eq!(row[11].as_str().unwrap(), "http://tracker/announce");

    let rows = map
        .call_command("d.multicall", &args, &Target::None, true)
        .unwrap();
    let rows = rows.as_list().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].as_list().unwrap()[0].as_str().unwrap(), "extended");

    // Unknown views throw; an empty name resolves to the default view.
    let unknown = Object::create_list_with(vec![
        Object::from("missing-view"),
        Object::from("d.name"),
    ]);
    assert!(map
        .call_command("d.multicall2", &unknown, &Target::None, true)
        .is_err());
    let default_view = Object::create_list_with(vec![
        Object::from(""),
        Object::from("d.name"),
    ]);
    let rows = map
        .call_command("d.multicall2", &default_view, &Target::None, true)
        .unwrap();
    assert!(rows.as_list().unwrap().is_empty());
}

#[test]
fn custom_fields_support_legacy_variants() {
    let (map, target, _) = setup();

    call(
        &map,
        "d.custom.set",
        &target,
        vec![Object::from("tag"), Object::from("alpha")],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.custom", &target, vec![Object::from("tag")])
            .unwrap()
            .as_str()
            .unwrap(),
        "alpha"
    );
    assert_eq!(
        call(&map, "d.custom_throw", &target, vec![Object::from("tag")])
            .unwrap()
            .as_str()
            .unwrap(),
        "alpha"
    );
    assert!(call(
        &map,
        "d.custom_throw",
        &target,
        vec![Object::from("missing")]
    )
    .is_err());
    assert_eq!(
        call(
            &map,
            "d.custom.if_z",
            &target,
            vec![Object::from("tag"), Object::from("fallback")],
        )
        .unwrap()
        .as_str()
        .unwrap(),
        "alpha"
    );
    assert_eq!(
        call(
            &map,
            "d.custom.if_z",
            &target,
            vec![Object::from("missing"), Object::from("fallback")],
        )
        .unwrap()
        .as_str()
        .unwrap(),
        "fallback"
    );

    call(
        &map,
        "d.custom.set",
        &target,
        vec![Object::from("empty"), Object::from("")],
    )
    .unwrap();
    assert_eq!(
        call(
            &map,
            "d.custom.if_z",
            &target,
            vec![Object::from("empty"), Object::from("fallback")],
        )
        .unwrap()
        .as_str()
        .unwrap(),
        "fallback"
    );

    let keys = call(&map, "d.custom.keys", &target, vec![]).unwrap();
    let keys: Vec<&str> = keys
        .as_list()
        .unwrap()
        .iter()
        .filter_map(|item| item.as_str().ok())
        .collect();
    assert!(keys.contains(&"tag"));
    assert!(keys.contains(&"empty"));

    let items = call(&map, "d.custom.items", &target, vec![]).unwrap();
    assert_eq!(
        items
            .as_map()
            .unwrap()
            .get(&b"tag".to_vec())
            .unwrap()
            .as_str()
            .unwrap(),
        "alpha"
    );

    for index in 1..=5 {
        let name = format!("d.custom{index}");
        let set_name = format!("d.custom{index}.set");
        call(&map, &set_name, &target, vec![Object::from("value")]).unwrap();
        assert_eq!(
            call(&map, &name, &target, vec![])
                .unwrap()
                .as_str()
                .unwrap(),
            "value"
        );
    }

    // d.custom and the numbered variants require a Download target.
    assert!(call(&map, "d.custom", &Target::None, vec![Object::from("tag")]).is_err());
    assert!(call(&map, "d.custom1", &Target::None, vec![]).is_err());
    assert!(call(&map, "d.custom.keys", &Target::None, vec![]).is_err());
    // Malformed setters are rejected rather than silently ignored.
    assert!(call(&map, "d.custom.set", &target, vec![Object::from("only-key")]).is_err());
    assert!(call(
        &map,
        "d.custom.if_z",
        &target,
        vec![Object::from(""), Object::from("fallback")],
    )
    .is_err());
}

#[test]
fn mode_ignore_commands_and_heuristics_round_trip() {
    let (map, target, _) = setup();

    assert_eq!(
        call(&map, "d.mode", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    // d.state is the legacy numeric started/stopped value.
    assert_eq!(
        call(&map, "d.state", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    call(&map, "d.mode.set", &target, vec![Object::from(2i64)]).unwrap();
    assert_eq!(
        call(&map, "d.mode", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        2
    );
    assert!(call(&map, "d.mode.set", &target, vec![Object::from(4i64)]).is_err());
    assert!(call(&map, "d.mode.set", &target, vec![Object::from(-1i64)]).is_err());

    assert_eq!(
        call(&map, "d.ignore_commands", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    call(
        &map,
        "d.ignore_commands.set",
        &target,
        vec![Object::from(1i64)],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.ignore_commands", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    assert_eq!(
        call(&map, "d.up.choke_heuristics", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "upload_leech"
    );
    call(
        &map,
        "d.up.choke_heuristics.set",
        &target,
        vec![Object::from("upload_seed")],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.up.choke_heuristics", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "upload_seed"
    );
    assert!(call(
        &map,
        "d.up.choke_heuristics.set",
        &target,
        vec![Object::from("download_leech")]
    )
    .is_err());

    assert_eq!(
        call(&map, "d.down.choke_heuristics", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "download_leech"
    );
    assert!(call(
        &map,
        "d.down.choke_heuristics.set",
        &target,
        vec![Object::from("upload_seed")]
    )
    .is_err());

    call(
        &map,
        "d.up.choke_heuristics.leech.set",
        &target,
        vec![Object::from("upload_leech_experimental")],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.up.choke_heuristics.leech", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "upload_leech_experimental"
    );
    call(
        &map,
        "d.up.choke_heuristics.seed.set",
        &target,
        vec![Object::from("upload_seed")],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.up.choke_heuristics.seed", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "upload_seed"
    );
    call(
        &map,
        "d.down.choke_heuristics.leech.set",
        &target,
        vec![Object::from("download_leech")],
    )
    .unwrap();
    call(
        &map,
        "d.down.choke_heuristics.seed.set",
        &target,
        vec![Object::from("download_leech")],
    )
    .unwrap();

    assert!(call(&map, "d.mode", &Target::None, vec![]).is_err());
    assert!(call(&map, "d.ignore_commands", &Target::None, vec![]).is_err());

    assert_eq!(
        call(&map, "d.connection_leech", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "leech"
    );
    call(
        &map,
        "d.connection_leech.set",
        &target,
        vec![Object::from("seed")],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.connection_leech", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "seed"
    );
    assert!(call(
        &map,
        "d.connection_leech.set",
        &target,
        vec![Object::from("bogus")]
    )
    .is_err());

    // d.complete.set overrides the engine completion flag for command
    // consumers (legacy UI clears it before d.check_hash).
    assert_eq!(
        call(&map, "d.complete", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    call(&map, "d.complete.set", &target, vec![Object::from(1i64)]).unwrap();
    assert_eq!(
        call(&map, "d.complete", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&map, "d.incomplete", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
}

#[test]
fn creation_date_timestamps_and_directory_alias() {
    let (map, target, _) = setup();

    assert_eq!(
        call(&map, "d.creation_date", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    call(
        &map,
        "d.timestamp.started.set",
        &target,
        vec![Object::from(100i64)],
    )
    .unwrap();
    assert_eq!(
        call(
            &map,
            "d.timestamp.started.elapsed",
            &target,
            vec![Object::from(99i64)],
        )
        .unwrap()
        .as_value()
        .unwrap(),
        1
    );
    assert_eq!(
        call(
            &map,
            "d.timestamp.started.elapsed",
            &target,
            vec![Object::from(100i64)],
        )
        .unwrap()
        .as_value()
        .unwrap(),
        0
    );

    call(
        &map,
        "d.timestamp.finished.set",
        &target,
        vec![Object::from(200i64)],
    )
    .unwrap();
    assert_eq!(
        call(
            &map,
            "d.timestamp.finished.elapsed",
            &target,
            vec![Object::from(150i64)],
        )
        .unwrap()
        .as_value()
        .unwrap(),
        1
    );

    call(
        &map,
        "d.directory.base.set",
        &target,
        vec![Object::from("/tmp/base-alias")],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.directory", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "/tmp/base-alias"
    );
    assert_eq!(
        call(&map, "d.directory_base", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "/tmp/base-alias"
    );
}

#[test]
fn local_id_group_name_and_hashing_variants() {
    let (map, target, download) = setup();

    // No persisted local id: return an empty string rather than fabricating
    // an id that would not match the peer handshake.
    assert_eq!(
        call(&map, "d.local_id", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );
    assert_eq!(
        call(&map, "d.local_id_html", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );

    let local_id = b"-RT0001-abcDEF-09_\x00\xff".to_vec();
    assert_eq!(local_id.len(), 20);
    let mut rtorrent = Object::create_map();
    rtorrent
        .insert_key("local_id", Object::create_string(local_id))
        .unwrap();
    let mut root = Object::create_map();
    root.insert_key("rtorrent", rtorrent).unwrap();
    download.write().unwrap().set_bencode(root);

    assert_eq!(
        call(&map, "d.local_id", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "2d5254303030312d6162634445462d30395f00ff"
    );
    assert_eq!(
        call(&map, "d.local_id_html", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "-RT0001-abcDEF-09%5F%00%FF"
    );

    // The fallback naming scheme matches ChokeGroupManager's auto-names.
    assert_eq!(
        call(&map, "d.group.name", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "global"
    );
    call(&map, "d.group.set", &target, vec![Object::from(3i64)]).unwrap();
    assert_eq!(
        call(&map, "d.group.name", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "group3"
    );

    assert_eq!(
        call(&map, "d.hashing", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    call(&map, "d.hashing.set", &target, vec![Object::from(3i64)]).unwrap();
    assert_eq!(
        call(&map, "d.hashing", &target, vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        3
    );
    assert!(call(&map, "d.hashing.set", &target, vec![Object::from(4i64)]).is_err());
    assert!(call(&map, "d.hashing.set", &target, vec![Object::from(-1i64)]).is_err());
}

#[test]
fn group_name_reads_live_choke_group_registry() {
    use rt_core::commands::groups::{register_group_commands, ChokeGroupManager};

    let download = Arc::new(RwLock::new(Download::from_metadata(metadata())));
    let hash = *download.read().unwrap().info_hash();
    let mut list = DownloadList::new();
    list.insert(Arc::clone(&download));
    let list = Arc::new(RwLock::new(list));

    let mut vm = ViewManager::new();
    vm.insert("main");
    let vm = Arc::new(RwLock::new(vm));

    let mut map = CommandMap::new();
    let manager = Arc::new(RwLock::new(ChokeGroupManager::new()));
    register_group_commands(&mut map, manager).unwrap();
    register_download_commands(&mut map, list, vm).unwrap();

    // Insert the group after download registration: d.group.name reads the
    // live choke_group registry rather than a frozen name snapshot.
    let index = map
        .call_command(
            "choke_group.insert",
            &Object::from("media"),
            &Target::None,
            true,
        )
        .unwrap()
        .as_value()
        .unwrap();

    let target = Target::Download(hash);
    call(&map, "d.group.set", &target, vec![Object::from(index)]).unwrap();
    assert_eq!(
        call(&map, "d.group.name", &target, vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "media"
    );
}

#[test]
fn indexed_tracker_getters_use_the_live_tracker_list() {
    let (map, target, _) = setup();

    assert_eq!(
        call(&map, "d.tracker_url", &target, vec![Object::from(0i64)])
            .unwrap()
            .as_str()
            .unwrap(),
        "http://tracker/announce"
    );
    assert_eq!(
        call(&map, "d.tracker_type", &target, vec![Object::from(0i64)])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&map, "d.tracker_group", &target, vec![Object::from(0i64)])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert_eq!(
        call(&map, "d.tracker_is_enabled", &target, vec![Object::from(0i64)])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&map, "d.tracker_is_usable", &target, vec![Object::from(0i64)])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );
    assert_eq!(
        call(&map, "d.tracker_is_extra", &target, vec![Object::from(0i64)])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );

    call(
        &map,
        "d.tracker.insert",
        &target,
        vec![
            Object::from(0i64),
            Object::from("udp://extra.invalid:6881"),
        ],
    )
    .unwrap();
    assert_eq!(
        call(&map, "d.tracker_url", &target, vec![Object::from(1i64)])
            .unwrap()
            .as_str()
            .unwrap(),
        "udp://extra.invalid:6881"
    );
    assert_eq!(
        call(&map, "d.tracker_type", &target, vec![Object::from(1i64)])
            .unwrap()
            .as_value()
            .unwrap(),
        2
    );
    assert_eq!(
        call(&map, "d.tracker_is_extra", &target, vec![Object::from(1i64)])
            .unwrap()
            .as_value()
            .unwrap(),
        1
    );

    // Indices are validated strictly; missing, negative and out-of-range
    // arguments are errors rather than empty reads.
    assert!(call(&map, "d.tracker_url", &target, vec![]).is_err());
    assert!(call(&map, "d.tracker_url", &target, vec![Object::from(-1i64)]).is_err());
    assert!(call(&map, "d.tracker_url", &target, vec![Object::from(99i64)]).is_err());
    assert!(call(&map, "d.tracker_url", &Target::None, vec![Object::from(0i64)]).is_err());
}

#[test]
fn multicall_filtered_selects_matching_downloads() {
    let (map, _target, _) = setup();

    let args = Object::create_list_with(vec![
        Object::from("main"),
        Object::from("d.incomplete"),
        Object::from("d.name"),
        Object::from("d.mode"),
    ]);
    let rows = map
        .call_command("d.multicall.filtered", &args, &Target::None, true)
        .unwrap();
    let rows = rows.as_list().unwrap();
    assert_eq!(rows.len(), 1);
    let row = rows[0].as_list().unwrap();
    assert_eq!(row[0].as_str().unwrap(), "extended");
    assert_eq!(row[1].as_value().unwrap(), 0);

    let args = Object::create_list_with(vec![
        Object::from("main"),
        Object::from("d.complete"),
        Object::from("d.name"),
    ]);
    let rows = map
        .call_command("d.multicall.filtered", &args, &Target::None, true)
        .unwrap();
    assert!(rows.as_list().unwrap().is_empty());

    let too_few = Object::create_list_with(vec![Object::from("main")]);
    assert!(map
        .call_command("d.multicall.filtered", &too_few, &Target::None, true)
        .is_err());
    let unknown = Object::create_list_with(vec![
        Object::from("missing-view"),
        Object::from("d.incomplete"),
        Object::from("d.name"),
    ]);
    assert!(map
        .call_command("d.multicall.filtered", &unknown, &Target::None, true)
        .is_err());
}

#[test]
fn test_load_raw_start_and_data_uri() {
    let mut map = CommandMap::new();
    let download_list = Arc::new(RwLock::new(DownloadList::new()));
    let view_manager = Arc::new(RwLock::new(ViewManager::new()));

    register_download_commands(&mut map, Arc::clone(&download_list), Arc::clone(&view_manager)).unwrap();

    let bencoded_torrent = {
        let mut info = Object::create_map();
        info.insert_key("name", Object::from("test_load_torrent")).unwrap();
        info.insert_key("length", Object::from(1024i64)).unwrap();
        info.insert_key("piece length", Object::from(262144i64)).unwrap();
        // 20 bytes of non-UTF8 binary data (like SHA1 pieces)
        info.insert_key("pieces", Object::from(vec![0xFFu8; 20])).unwrap();

        let mut object = Object::create_map();
        object.insert_key("info", info).unwrap();
        lt_common::object::bencode::encode(&object).unwrap()
    };

    // 1. Test load.raw_start with binary raw bytes and trailing commands
    let args = Object::create_list_with(vec![
        Object::from(""), // target placeholder
        Object::create_string(bencoded_torrent.clone()),
        Object::from("d.directory.set=\"/custom/dir\""),
        Object::from("d.custom1.set=\"my_tag\""),
    ]);

    let res = map.call_command("load.raw_start", &args, &Target::None, true).unwrap();
    assert_eq!(res.as_value().unwrap(), 0);

    // Verify torrent was added, directory was set, and custom1 was set
    {
        let dl_list = download_list.read().unwrap();
        assert_eq!(dl_list.all().len(), 1);
        let dl = dl_list.all()[0].clone();
        assert_eq!(dl.read().unwrap().directory(), "/custom/dir");
        assert!(dl.read().unwrap().is_open());

        let target = Target::Download(*dl.read().unwrap().info_hash());
        let custom1 = map.call_command("d.custom1", &Object::create_empty(), &target, true).unwrap();
        assert_eq!(custom1.as_str().unwrap(), "my_tag");
    }

    // 2. Test load.start_throw with data URI
    use base64::prelude::*;

    // Using a different torrent name so info_hash differs
    let mut info2 = Object::create_map();
    info2.insert_key("name", Object::from("test_load_torrent_2")).unwrap();
    info2.insert_key("length", Object::from(2048i64)).unwrap();
    info2.insert_key("piece length", Object::from(262144i64)).unwrap();
    info2.insert_key("pieces", Object::from(vec![0xFEu8; 20])).unwrap();
    let mut object2 = Object::create_map();
    object2.insert_key("info", info2).unwrap();
    let b64_2 = BASE64_STANDARD.encode(&lt_common::object::bencode::encode(&object2).unwrap());
    let data_uri_2 = format!("data:applications/x-bittorrent;base64,{b64_2}");

    let args2 = Object::create_list_with(vec![
        Object::from(""),
        Object::from(data_uri_2.as_str()),
        Object::from("d.directory.set=\"/custom/dir2\""),
    ]);

    let res2 = map.call_command("load.start_throw", &args2, &Target::None, true).unwrap();
    assert_eq!(res2.as_value().unwrap(), 0);
    assert_eq!(download_list.read().unwrap().all().len(), 2);
}


