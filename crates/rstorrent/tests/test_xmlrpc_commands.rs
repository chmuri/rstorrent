use std::sync::{Arc, RwLock};

use lt_common::Object;
use rstorrent::register_scgi_commands;
use rt_rpc::{CommandMap, Target};

fn call(map: &CommandMap, name: &str, args: Vec<Object>) -> Result<Object, rt_rpc::RpcError> {
    map.call_command(name, &Object::create_list_with(args), &Target::None, true)
}

#[test]
fn xmlrpc_commands_manage_size_limit_and_dialect() {
    let mut map = CommandMap::new();
    let map_arc = Arc::new(RwLock::new(map.clone()));
    register_scgi_commands(&mut map, map_arc).unwrap();

    let default_limit = call(&map, "network.xmlrpc.size_limit", vec![])
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(default_limit, rt_rpc::MAX_CONTENT_SIZE as i64);

    assert!(call(
        &map,
        "network.xmlrpc.size_limit.set",
        vec![Object::from(4096i64)]
    )
    .is_ok());
    assert_eq!(
        call(&map, "network.xmlrpc.size_limit", vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        4096
    );

    // Too small or too large limits are rejected.
    assert!(call(
        &map,
        "network.xmlrpc.size_limit.set",
        vec![Object::from(1i64)]
    )
    .is_err());
    assert!(call(
        &map,
        "network.xmlrpc.size_limit.set",
        vec![Object::from(rt_rpc::MAX_CONTENT_SIZE as i64 + 1)]
    )
    .is_err());

    assert_eq!(
        call(&map, "network.xmlrpc.dialect", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "i8"
    );
    assert!(call(
        &map,
        "network.xmlrpc.dialect.set",
        vec![Object::from("apache")]
    )
    .is_ok());
    assert_eq!(
        call(&map, "network.xmlrpc.dialect", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "apache"
    );
    assert!(call(
        &map,
        "network.xmlrpc.dialect.set",
        vec![Object::from("unknown")]
    )
    .is_err());
}
