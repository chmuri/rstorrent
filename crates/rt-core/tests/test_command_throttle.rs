use std::sync::{Arc, RwLock};
use lt_common::Object;
use rt_core::commands::throttle::{register_throttle_commands, ThrottleManager};
use rt_rpc::{CommandMap, Target};

fn setup_throttle() -> (CommandMap, Arc<RwLock<ThrottleManager>>) {
    let mut map = CommandMap::new();
    let mgr = Arc::new(RwLock::new(ThrottleManager::new()));
    register_throttle_commands(&mut map, Arc::clone(&mgr)).unwrap();
    (map, mgr)
}

fn call_set(map: &CommandMap, key: &str, value: &str) -> Result<(), rt_rpc::RpcError> {
    map.call_command(key, &Object::from(value.to_string()), &Target::None, true)?;
    Ok(())
}

fn call_named(map: &CommandMap, key: &str, name: &str, value: &str) -> Result<(), rt_rpc::RpcError> {
    let args = Object::create_list_with(vec![
        Object::from(name.to_string()),
        Object::from(value.to_string()),
    ]);
    map.call_command(key, &args, &Target::None, true)?;
    Ok(())
}

#[test]
fn test_global_rate_in_range() {
    let (map, mgr) = setup_throttle();

    call_set(&map, "throttle.global_down.max_rate.set_kb", "1024").unwrap();
    assert_eq!(1048576, mgr.read().unwrap().global_down);

    call_set(&map, "throttle.global_down.max_rate.set_kb", "4194303").unwrap();
    assert_eq!(4294966272, mgr.read().unwrap().global_down);

    call_set(&map, "throttle.global_down.max_rate.set", "4294966272").unwrap();
    assert_eq!(4294966272, mgr.read().unwrap().global_down);
}

#[test]
fn test_global_rate_kb_out_of_range() {
    let (map, mgr) = setup_throttle();

    call_set(&map, "throttle.global_down.max_rate.set_kb", "1024").unwrap();
    assert!(call_set(&map, "throttle.global_down.max_rate.set_kb", "4194304").is_err());
    assert_eq!(1048576, mgr.read().unwrap().global_down);
}

#[test]
fn test_global_rate_bytes_out_of_range() {
    let (map, mgr) = setup_throttle();

    call_set(&map, "throttle.global_down.max_rate.set", "1048576").unwrap();
    assert!(call_set(&map, "throttle.global_down.max_rate.set", "4294967296").is_err());
    assert_eq!(1048576, mgr.read().unwrap().global_down);
}

#[test]
fn test_global_rate_negative() {
    let (map, mgr) = setup_throttle();

    call_set(&map, "throttle.global_down.max_rate.set", "1048576").unwrap();
    assert!(call_set(&map, "throttle.global_down.max_rate.set", "-1").is_err());
    assert_eq!(1048576, mgr.read().unwrap().global_down);
}

#[test]
fn test_named_rate_in_range() {
    let (map, mgr) = setup_throttle();

    call_named(&map, "throttle.down", "test_named_in_range", "1024").unwrap();
    let throttles = mgr.read().unwrap().named_throttles.clone();
    assert!(throttles.contains_key("test_named_in_range"));
    assert_eq!(1048576, throttles["test_named_in_range"].1);
}
