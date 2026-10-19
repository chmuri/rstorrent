use std::sync::{Arc, RwLock};

use lt_common::Object;
use lt_net::config::NetworkConfig;
use rt_core::commands::groups::{register_group_commands, ChokeGroupManager};
use rt_core::commands::network::register_network_commands;
use rt_core::commands::pieces::register_pieces_commands;
use rt_core::commands::protocol::register_protocol_commands;
use rt_core::commands::system::register_system_commands;
use rt_rpc::{CommandMap, Target};

fn call(map: &CommandMap, name: &str, args: Vec<Object>) -> Result<Object, rt_rpc::RpcError> {
    map.call_command(name, &Object::create_list_with(args), &Target::None, true)
}

#[test]
fn network_commands_update_shared_config() {
    let config = NetworkConfig::new();
    let mut map = CommandMap::new();
    register_network_commands(&mut map, config.clone()).unwrap();

    assert!(call(&map, "network.block.ipv4.set", vec![Object::from(1i64)]).is_ok());
    assert_eq!(call(&map, "network.block.ipv4", vec![]).unwrap().as_value().unwrap(), 1);

    assert!(call(&map, "network.local_port.set", vec![Object::from(7001i64)]).is_ok());
    assert_eq!(call(&map, "network.local_port", vec![]).unwrap().as_value().unwrap(), 7001);

    assert!(call(
        &map,
        "network.bind_address.set",
        vec![Object::from("127.0.0.1")]
    )
    .is_ok());
    assert_eq!(
        call(&map, "network.bind_address", vec![]).unwrap().as_str().unwrap(),
        "127.0.0.1"
    );
}

#[test]
fn pieces_and_protocol_commands_round_trip() {
    let mut map = CommandMap::new();
    register_pieces_commands(&mut map).unwrap();
    register_protocol_commands(&mut map, NetworkConfig::new()).unwrap();

    assert!(call(&map, "pieces.memory.max.set", vec![Object::from(4096i64)]).is_ok());
    assert_eq!(call(&map, "pieces.memory.max", vec![]).unwrap().as_value().unwrap(), 4096);

    assert!(call(&map, "pieces.hash.on_completion.set", vec![Object::from(0i64)]).is_ok());
    assert_eq!(call(&map, "pieces.hash.on_completion", vec![]).unwrap().as_value().unwrap(), 0);

    assert!(call(
        &map,
        "protocol.encryption.set",
        vec![Object::from("require"), Object::from("require")]
    )
    .is_ok());
    assert_eq!(
        call(&map, "protocol.encryption", vec![]).unwrap().as_str().unwrap(),
        "require,require"
    );

    assert!(call(&map, "protocol.pex.set", vec![Object::from(0i64)]).is_ok());
    assert_eq!(call(&map, "protocol.pex", vec![]).unwrap().as_value().unwrap(), 0);
}

#[test]
fn choke_group_commands_cover_listing_and_limits() {
    let manager = Arc::new(RwLock::new(ChokeGroupManager::new()));
    let mut map = CommandMap::new();
    register_group_commands(&mut map, Arc::clone(&manager)).unwrap();

    assert!(call(&map, "choke_group.size", vec![]).unwrap().as_value().unwrap() >= 1);
    assert!(call(&map, "choke_group.insert", vec![Object::from("fast")]).is_ok());
    let index = call(&map, "choke_group.index_of", vec![Object::from("fast")])
        .unwrap()
        .as_value()
        .unwrap();
    assert!(index >= 1);

    assert!(call(
        &map,
        "choke_group.up.max.set",
        vec![Object::from(index), Object::from(12i64)]
    )
    .is_ok());
    assert_eq!(
        call(&map, "choke_group.up.max", vec![Object::from(index)])
            .unwrap()
            .as_value()
            .unwrap(),
        12
    );

    assert!(call(
        &map,
        "choke_group.up.heuristics.set",
        vec![Object::from(index), Object::from("upload_seed")]
    )
    .is_ok());
    assert_eq!(
        call(&map, "choke_group.up.heuristics", vec![Object::from(index)])
            .unwrap()
            .as_str()
            .unwrap(),
        "upload_seed"
    );
}

#[test]
fn system_file_settings_round_trip() {
    let mut map = CommandMap::new();
    register_system_commands(&mut map).unwrap();

    assert!(call(&map, "system.file.allocate.set", vec![Object::from(2i64)]).is_ok());
    assert_eq!(call(&map, "system.file.allocate", vec![]).unwrap().as_value().unwrap(), 2);

    assert!(call(
        &map,
        "system.file.split_suffix.set",
        vec![Object::from(".split")]
    )
    .is_ok());
    assert_eq!(
        call(&map, "system.file.split_suffix", vec![]).unwrap().as_str().unwrap(),
        ".split"
    );
}
