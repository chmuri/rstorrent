use std::sync::{Arc, RwLock};

use lt_common::Object;
use rt_core::commands::dht::register_dht_commands;
use rt_core::commands::tracker::{register_tracker_commands, DhtConfig};
use rt_rpc::{CommandMap, Target};

fn setup() -> (CommandMap, Arc<RwLock<DhtConfig>>) {
    let mut map = CommandMap::new();
    let config = Arc::new(RwLock::new(DhtConfig::new()));
    let list = Arc::new(RwLock::new(rt_core::DownloadList::new()));
    // Install the legacy tracker commands first, then replace the DHT subset.
    register_tracker_commands(
        &mut map,
        Arc::clone(&config),
        list,
    )
    .unwrap();
    register_dht_commands(&mut map, Arc::clone(&config)).unwrap();
    (map, config)
}

fn call(map: &CommandMap, name: &str, value: &str) -> Result<Object, rt_rpc::RpcError> {
    map.call_command(
        name,
        &Object::from(value.to_string()),
        &Target::None,
        true,
    )
}

fn value(map: &CommandMap, name: &str) -> i64 {
    map.call_command(name, &Object::create_empty(), &Target::None, true)
        .unwrap()
        .as_value()
        .unwrap()
}

#[test]
fn dht_mode_and_port_surface_is_runtime_aware() {
    let (map, config) = setup();

    let mode = map
        .call_command("dht.mode", &Object::create_empty(), &Target::None, true)
        .unwrap();
    assert_eq!(mode.as_str().unwrap(), "auto");
    assert!(call(&map, "dht.mode.set", "on").is_ok());
    assert_eq!(config.read().unwrap().mode, "on");
    assert!(call(&map, "dht.mode.set", "bogus").is_err());

    assert!(call(&map, "dht.port.set", "7000").is_ok());
    assert_eq!(value(&map, "dht.port"), 7000);
    assert_eq!(config.read().unwrap().port, 7000);

    assert!(call(&map, "dht.override_port.set", "7001").is_ok());
    assert_eq!(value(&map, "dht.override_port"), 7001);
    assert_eq!(config.read().unwrap().override_port, 7001);

    assert!(call(&map, "dht.port.set", "65536").is_err());
    assert!(call(&map, "dht.override_port.set", "-1").is_err());
}

#[test]
fn dht_statistics_and_controls_work_without_a_running_runtime() {
    let (map, _) = setup();

    let stats = map
        .call_command(
            "dht.statistics",
            &Object::create_empty(),
            &Target::None,
            true,
        )
        .unwrap();
    assert_eq!(
        stats.get_key("active").unwrap().as_value().unwrap(),
        0
    );
    assert!(stats.get_key("nodes").is_ok());
    assert!(stats.get_key("buckets").is_ok());
    assert_eq!(value(&map, "dht.is_active"), 0);

    assert!(call(&map, "dht.add_node", "127.0.0.1:6881").is_ok());
    assert!(call(&map, "dht.add_node", "127.0.0.1:99999").is_err());
    assert!(call(&map, "dht.bootstrap", "").is_ok());
    assert!(call(&map, "dht.refresh", "").is_ok());
    assert!(call(&map, "dht.reset_statistics", "").is_ok());

    assert!(call(&map, "dht.refresh_interval.set", "120").is_ok());
    assert!(call(&map, "dht.refresh_interval.set", "0").is_err());
    assert!(call(&map, "dht.bootstrap_retry_interval.set", "30").is_ok());
    assert!(call(&map, "dht.bootstrap_retry_interval.set", "-1").is_err());
}

#[test]
fn dht_commands_can_be_installed_after_tracker_without_duplicates() {
    let (map, _) = setup();
    // Both command families remain callable after the DHT replacement.
    assert!(map
        .call_command("dht.mode", &Object::create_empty(), &Target::None, true)
        .is_ok());
    assert!(map
        .call_command("dht.statistics", &Object::create_empty(), &Target::None, true)
        .is_ok());
}
