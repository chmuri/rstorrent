use lt_common::hash_string::HashString;
use lt_common::Object;
use rt_core::commands::groups::{register_group_commands, ChokeGroupManager};
use rt_rpc::{CommandMap, Target};
use std::sync::{Arc, RwLock};

fn setup_groups() -> (CommandMap, Arc<RwLock<ChokeGroupManager>>) {
    let mut map = CommandMap::new();
    let mgr = Arc::new(RwLock::new(ChokeGroupManager::new()));
    register_group_commands(&mut map, Arc::clone(&mgr)).unwrap();
    (map, mgr)
}

fn call_set(map: &CommandMap, value: &str) -> Result<(), rt_rpc::RpcError> {
    let args = Object::create_list_with(vec![Object::from(0i64), Object::from(value.to_string())]);
    map.call_command("choke_group.up.max.set", &args, &Target::None, true)?;
    Ok(())
}

fn call_get(map: &CommandMap, key: &str) -> i64 {
    map.call_command(key, &Object::from(0i64), &Target::None, true)
        .unwrap()
        .as_value()
        .unwrap()
}

#[test]
fn test_max_unchoked_in_range() {
    let (map, _) = setup_groups();

    call_set(&map, "50").unwrap();
    assert_eq!(50, call_get(&map, "choke_group.up.max"));
    assert_eq!(0, call_get(&map, "choke_group.up.max.unlimited"));

    call_set(&map, "-1").unwrap();
    assert_eq!(1, call_get(&map, "choke_group.up.max.unlimited"));
}

#[test]
fn test_max_unchoked_out_of_range() {
    let (map, _) = setup_groups();

    call_set(&map, "50").unwrap();

    assert!(call_set(&map, "4294967296").is_err());
    assert_eq!(50, call_get(&map, "choke_group.up.max"));

    assert!(call_set(&map, "-2").is_err());
    assert_eq!(50, call_get(&map, "choke_group.up.max"));
}

fn call(map: &CommandMap, key: &str, args: Vec<Object>) -> Result<Object, rt_rpc::RpcError> {
    let obj = if args.is_empty() {
        Object::create_empty()
    } else {
        Object::create_list_with(args)
    };
    map.call_command(key, &obj, &Target::None, true)
}

fn value(map: &CommandMap, key: &str, args: Vec<Object>) -> i64 {
    call(map, key, args).unwrap().as_value().unwrap()
}

fn text(map: &CommandMap, key: &str, args: Vec<Object>) -> String {
    call(map, key, args).unwrap().as_str().unwrap().to_string()
}

#[test]
fn test_group_min_max_and_membership_commands() {
    let (map, manager) = setup_groups();

    let index = call(&map, "choke_group.insert", vec![Object::from("fast")])
        .unwrap()
        .as_value()
        .unwrap();
    assert!(index >= 1);

    call(
        &map,
        "choke_group.up.max.set",
        vec![Object::from(index), Object::from(2i64)],
    )
    .unwrap();
    call(
        &map,
        "choke_group.up.min.set",
        vec![Object::from(index), Object::from(9i64)],
    )
    .unwrap();

    // The minimum is clamped to the group maximum.
    assert_eq!(
        value(&map, "choke_group.up.min", vec![Object::from(index)]),
        2
    );

    // Group membership is reported from the resource manager.
    let hash = HashString::new([0x5a; 20]);
    manager
        .write()
        .unwrap()
        .resource_manager_mut()
        .insert_entry(hash, 0, index as u16);
    assert_eq!(
        value(&map, "choke_group.general.size", vec![Object::from(index)]),
        1
    );

    // Removing the group reassigns its downloads to the global group.
    call(&map, "choke_group.remove", vec![Object::from(index)]).unwrap();
    assert_eq!(
        value(&map, "choke_group.general.size", vec![Object::from(0i64)]),
        1
    );
    assert!(call(&map, "choke_group.remove", vec![Object::from(0i64)]).is_err());
    assert!(call(&map, "choke_group.remove", vec![Object::from(99i64)]).is_err());

    // Balance commands are backed by the resource manager and must not panic
    // with empty queues.
    call(&map, "choke_group.all.up.update_balance", vec![]).unwrap();
    call(&map, "choke_group.all.down.update_balance", vec![]).unwrap();
}

#[test]
fn test_group_seeding_ratio_commands_target_groups() {
    let (map, _) = setup_groups();
    let index = call(&map, "choke_group.insert", vec![Object::from("seed")])
        .unwrap()
        .as_value()
        .unwrap();

    call(
        &map,
        "group.seeding.ratio.enable",
        vec![Object::from(index)],
    )
    .unwrap();
    assert_eq!(
        value(
            &map,
            "group.seeding.ratio.enabled",
            vec![Object::from(index)]
        ),
        1
    );
    assert_eq!(value(&map, "group.seeding.ratio.enabled", vec![]), 0);

    call(
        &map,
        "group.seeding.ratio.min.set",
        vec![Object::from(index), Object::from(150i64)],
    )
    .unwrap();
    call(
        &map,
        "group.seeding.ratio.max.set",
        vec![Object::from(index), Object::from(250i64)],
    )
    .unwrap();
    call(
        &map,
        "group.seeding.ratio.upload.set",
        vec![Object::from(index), Object::from(1024i64)],
    )
    .unwrap();

    assert_eq!(
        value(&map, "group.seeding.ratio.min", vec![Object::from(index)]),
        150
    );
    assert_eq!(
        value(&map, "group.seeding.ratio.max", vec![Object::from(index)]),
        250
    );
    assert_eq!(
        value(
            &map,
            "group.seeding.ratio.upload",
            vec![Object::from(index)]
        ),
        1024
    );

    // Negative ratio values are rejected without changing the config.
    assert!(call(
        &map,
        "group.seeding.ratio.min.set",
        vec![Object::from(index), Object::from(-1i64)]
    )
    .is_err());
    assert_eq!(
        value(&map, "group.seeding.ratio.min", vec![Object::from(index)]),
        150
    );

    call(
        &map,
        "group.seeding.ratio.command.set",
        vec![Object::from(index), Object::from("d.stop=")],
    )
    .unwrap();
    assert_eq!(
        text(
            &map,
            "group.seeding.ratio.command",
            vec![Object::from(index)]
        ),
        "d.stop="
    );

    // The legacy `ratio.*` aliases operate on the global group.
    call(&map, "ratio.min.set", vec![Object::from(175i64)]).unwrap();
    assert_eq!(value(&map, "ratio.min", vec![]), 175);
    call(&map, "ratio.enable", vec![]).unwrap();
    assert_eq!(value(&map, "ratio.enabled", vec![]), 1);
}

#[test]
fn test_group_commands_override_generic_ratio_registration() {
    let mut map = CommandMap::new();
    rt_core::commands::ratio::register_ratio_commands(&mut map).unwrap();
    let manager = Arc::new(RwLock::new(ChokeGroupManager::new()));
    register_group_commands(&mut map, Arc::clone(&manager)).unwrap();

    let index = call(&map, "choke_group.insert", vec![Object::from("late")])
        .unwrap()
        .as_value()
        .unwrap();
    call(
        &map,
        "group.seeding.ratio.enable",
        vec![Object::from(index)],
    )
    .unwrap();
    assert_eq!(
        value(
            &map,
            "group.seeding.ratio.enabled",
            vec![Object::from(index)]
        ),
        1
    );
}
