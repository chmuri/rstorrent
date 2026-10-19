use std::sync::{Arc, RwLock};
use lt_common::Object;
use rt_core::commands::tracker::{checked_port_value, register_tracker_commands, DhtConfig};
use rt_rpc::{CommandMap, Target};

fn setup_tracker() -> (CommandMap, Arc<RwLock<DhtConfig>>) {
    let mut map = CommandMap::new();
    let config = Arc::new(RwLock::new(DhtConfig::new()));
    let dl_list = Arc::new(RwLock::new(rt_core::DownloadList::new()));
    register_tracker_commands(&mut map, Arc::clone(&config), dl_list).unwrap();
    (map, config)
}

fn call_set(map: &CommandMap, value: &str) -> Result<(), rt_rpc::RpcError> {
    map.call_command(
        "dht.override_port.set",
        &Object::from(value.to_string()),
        &Target::None,
        true,
    )?;
    Ok(())
}

fn override_port(map: &CommandMap) -> u16 {
    map.call_command("dht.override_port", &Object::create_empty(), &Target::None, true)
        .unwrap()
        .as_value()
        .unwrap() as u16
}

#[test]
fn test_dht_override_port_in_range() {
    let (map, _) = setup_tracker();

    call_set(&map, "6881").unwrap();
    assert_eq!(6881, override_port(&map));

    call_set(&map, "65535").unwrap();
    assert_eq!(65535, override_port(&map));
}

#[test]
fn test_dht_override_port_out_of_range() {
    let (map, _) = setup_tracker();

    call_set(&map, "6881").unwrap();

    assert!(call_set(&map, "70000").is_err());
    assert_eq!(6881, override_port(&map));

    assert!(call_set(&map, "-1").is_err());
    assert_eq!(6881, override_port(&map));
}

#[test]
fn test_checked_port_value() {
    assert_eq!(0, checked_port_value(0, "test").unwrap());
    assert_eq!(6881, checked_port_value(6881, "test").unwrap());
    assert_eq!(65535, checked_port_value(65535, "test").unwrap());

    assert!(checked_port_value(-1, "test").is_err());
    assert!(checked_port_value(65536, "test").is_err());
    assert!(checked_port_value(4294967296, "test").is_err());
}
