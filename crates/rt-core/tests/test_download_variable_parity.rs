//! Parity for per-download connection-type and numbered custom variables.
//!
//! Oracle: rtorrent `src/command_download.cc`.
//!
//! * `d.connection_leech` and `d.connection_seed` are independent
//!   `rtorrent.*` variables. Setting one must never change the other.
//! * `d.custom1`..`d.custom5` read and write `rtorrent:customN`, which is a
//!   different slot from the `rtorrent:custom` map used by `d.custom`,
//!   `d.custom.set`, `d.custom.keys` and `d.custom.items`.

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
    let info_hash = HashString::from_hex("5152535455565758595a5b5c5d5e5f6061626364").unwrap();
    let mut path = lt_common::path::Path::new();
    path.push("payload.bin");

    TorrentMetadata {
        info_hash,
        name: "variable-parity".to_string(),
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

fn setup() -> (CommandMap, Target) {
    let download = Arc::new(RwLock::new(Download::from_metadata(metadata())));
    download
        .write()
        .unwrap()
        .set_directory("/tmp/variable-parity");

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

    (map, Target::Download(hash))
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

fn text(map: &CommandMap, target: &Target, name: &str) -> String {
    call(map, name, target, vec![])
        .unwrap_or_else(|err| panic!("{name} failed: {err}"))
        .as_str()
        .unwrap()
        .to_string()
}

fn set(map: &CommandMap, name: &str, target: &Target, value: &str) -> Result<(), rt_rpc::RpcError> {
    call(map, name, target, vec![Object::from(value)]).map(|_| ())
}

fn list_of_strings(object: &Object) -> Vec<String> {
    object
        .as_list()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn connection_leech_and_seed_are_independent_variables() {
    let (map, target) = setup();

    assert_eq!(text(&map, &target, "d.connection_leech"), "leech");
    assert_eq!(text(&map, &target, "d.connection_seed"), "leech");

    // Seeding preference must not leak into the leech preference.
    set(&map, "d.connection_seed.set", &target, "seed").unwrap();
    assert_eq!(text(&map, &target, "d.connection_seed"), "seed");
    assert_eq!(
        text(&map, &target, "d.connection_leech"),
        "leech",
        "d.connection_seed.set must not rewrite d.connection_leech"
    );

    // ...and the reverse direction must hold too.
    set(&map, "d.connection_leech.set", &target, "initial_seed").unwrap();
    assert_eq!(text(&map, &target, "d.connection_leech"), "initial_seed");
    assert_eq!(
        text(&map, &target, "d.connection_seed"),
        "seed",
        "d.connection_leech.set must not rewrite d.connection_seed"
    );
}

#[test]
fn connection_leech_accepts_every_known_connection_type() {
    let (map, target) = setup();

    for value in ["leech", "seed", "initial_seed", "metadata"] {
        set(&map, "d.connection_leech.set", &target, value)
            .unwrap_or_else(|err| panic!("d.connection_leech.set={value} failed: {err}"));
        assert_eq!(text(&map, &target, "d.connection_leech"), value);
    }
}

#[test]
fn connection_seed_accepts_every_known_connection_type() {
    let (map, target) = setup();

    // `d.connection_seed.set` validates against the same connection-type
    // option table as `d.connection_leech.set` in rtorrent.
    for value in ["leech", "seed", "initial_seed", "metadata"] {
        set(&map, "d.connection_seed.set", &target, value)
            .unwrap_or_else(|err| panic!("d.connection_seed.set={value} failed: {err}"));
        assert_eq!(text(&map, &target, "d.connection_seed"), value);
        assert_eq!(
            text(&map, &target, "d.connection_leech"),
            "leech",
            "d.connection_seed.set={value} must not rewrite d.connection_leech"
        );
    }
}

#[test]
fn connection_leech_normalises_case_and_surrounding_whitespace() {
    let (map, target) = setup();

    set(&map, "d.connection_leech.set", &target, "  SEED  ").unwrap();
    assert_eq!(text(&map, &target, "d.connection_leech"), "seed");
}

#[test]
fn connection_leech_rejects_unknown_values_without_mutating_state() {
    let (map, target) = setup();

    set(&map, "d.connection_leech.set", &target, "seed").unwrap();
    assert!(set(&map, "d.connection_leech.set", &target, "bogus").is_err());
    assert_eq!(
        text(&map, &target, "d.connection_leech"),
        "seed",
        "a rejected value must leave the previous preference intact"
    );
}

#[test]
fn numbered_custom_variables_do_not_alias_the_custom_map() {
    let (map, target) = setup();

    set(&map, "d.custom1.set", &target, "alpha").unwrap();
    assert_eq!(text(&map, &target, "d.custom1"), "alpha");

    // The generic map is a separate slot: `custom1` is not visible there.
    assert_eq!(
        call(&map, "d.custom", &target, vec![Object::from("custom1")])
            .unwrap()
            .as_str()
            .unwrap(),
        ""
    );
    assert!(list_of_strings(&call(&map, "d.custom.keys", &target, vec![]).unwrap()).is_empty());

    // Writing through the generic map must not shadow the numbered variable.
    call(
        &map,
        "d.custom.set",
        &target,
        vec![Object::from("custom1"), Object::from("beta")],
    )
    .unwrap();
    assert_eq!(text(&map, &target, "d.custom1"), "alpha");
    assert_eq!(
        call(&map, "d.custom", &target, vec![Object::from("custom1")])
            .unwrap()
            .as_str()
            .unwrap(),
        "beta"
    );
    assert_eq!(
        list_of_strings(&call(&map, "d.custom.keys", &target, vec![]).unwrap()),
        vec!["custom1".to_string()]
    );
}

#[test]
fn numbered_custom_variables_are_independent() {
    let (map, target) = setup();

    for index in 1..=5 {
        set(
            &map,
            &format!("d.custom{index}.set"),
            &target,
            &format!("value{index}"),
        )
        .unwrap();
    }
    for index in 1..=5 {
        assert_eq!(
            text(&map, &target, &format!("d.custom{index}")),
            format!("value{index}")
        );
    }

    set(&map, "d.custom3.set", &target, "rewritten").unwrap();
    assert_eq!(text(&map, &target, "d.custom3"), "rewritten");
    assert_eq!(text(&map, &target, "d.custom2"), "value2");
    assert_eq!(text(&map, &target, "d.custom4"), "value4");
}

#[test]
fn numbered_custom_variables_default_to_empty_and_can_be_cleared() {
    let (map, target) = setup();

    assert_eq!(text(&map, &target, "d.custom5"), "");
    set(&map, "d.custom5.set", &target, "temporary").unwrap();
    assert_eq!(text(&map, &target, "d.custom5"), "temporary");
    set(&map, "d.custom5.set", &target, "").unwrap();
    assert_eq!(text(&map, &target, "d.custom5"), "");
}

#[test]
fn numbered_custom_variables_round_trip_unicode() {
    let (map, target) = setup();

    let value = "zażółć gęślą jaźń 🚀";
    set(&map, "d.custom2.set", &target, value).unwrap();
    assert_eq!(text(&map, &target, "d.custom2"), value);
}

#[test]
fn numbered_custom_variables_require_a_download_target() {
    let (map, _target) = setup();

    for index in 1..=5 {
        assert!(
            call(&map, &format!("d.custom{index}"), &Target::None, vec![]).is_err(),
            "d.custom{index} must reject a non-download target"
        );
        assert!(
            set(
                &map,
                &format!("d.custom{index}.set"),
                &Target::None,
                "value"
            )
            .is_err(),
            "d.custom{index}.set must reject a non-download target"
        );
    }
}

#[test]
fn connection_variables_require_a_download_target() {
    let (map, _target) = setup();

    for name in ["d.connection_leech", "d.connection_seed"] {
        assert!(
            call(&map, name, &Target::None, vec![]).is_err(),
            "{name} must reject a non-download target"
        );
    }
    assert!(set(&map, "d.connection_leech.set", &Target::None, "seed").is_err());
}
