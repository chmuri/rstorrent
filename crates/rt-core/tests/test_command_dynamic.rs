use lt_common::Object;
use rt_core::commands::dynamic::register_dynamic_commands;
use rt_rpc::{CommandMap, ObjectStorage, Target};
use std::sync::{Arc, RwLock};

fn setup_dynamic() -> (CommandMap, Arc<RwLock<ObjectStorage>>) {
    let mut map = CommandMap::new();
    let storage = Arc::new(RwLock::new(ObjectStorage::new()));
    register_dynamic_commands(&mut map, Arc::clone(&storage)).unwrap();
    (map, storage)
}

#[test]
fn test_basics() {
    let (map, _) = setup_dynamic();
    map.call_command(
        "method.insert.value",
        &Object::create_list_with(vec![Object::from("test_basics.1"), Object::from(1i64)]),
        &Target::None,
        true,
    )
    .unwrap();

    let val = map
        .call_command(
            "method.get",
            &Object::from("test_basics.1"),
            &Target::None,
            true,
        )
        .unwrap();
    assert_eq!(1, val.as_value().unwrap());
}

#[test]
fn test_get_set() {
    let (map, _) = setup_dynamic();
    map.call_command(
        "method.insert.simple",
        &Object::create_list_with(vec![Object::from("test_get_set.1"), Object::from("cat=1")]),
        &Target::None,
        true,
    )
    .unwrap();

    assert_eq!(
        "cat=1",
        map.call_command(
            "method.get",
            &Object::from("test_get_set.1"),
            &Target::None,
            true
        )
        .unwrap()
        .as_str()
        .unwrap()
    );

    map.call_command(
        "method.set",
        &Object::create_list_with(vec![Object::from("test_get_set.1"), Object::from("cat=2")]),
        &Target::None,
        true,
    )
    .unwrap();

    assert_eq!(
        "cat=2",
        map.call_command(
            "method.get",
            &Object::from("test_get_set.1"),
            &Target::None,
            true
        )
        .unwrap()
        .as_str()
        .unwrap()
    );
}

#[test]
fn test_old_style() {
    let (map, _) = setup_dynamic();

    map.call_command(
        "method.insert",
        &Object::create_list_with(vec![
            Object::from("test_old_style.1"),
            Object::from("value"),
            Object::from(1i64),
        ]),
        &Target::None,
        true,
    )
    .unwrap();
    assert_eq!(
        1,
        map.call_command(
            "method.get",
            &Object::from("test_old_style.1"),
            &Target::None,
            true
        )
        .unwrap()
        .as_value()
        .unwrap()
    );

    map.call_command(
        "method.insert",
        &Object::create_list_with(vec![
            Object::from("test_old_style.2"),
            Object::from("bool"),
            Object::from(5i64),
        ]),
        &Target::None,
        true,
    )
    .unwrap();
    assert_eq!(
        1,
        map.call_command(
            "method.get",
            &Object::from("test_old_style.2"),
            &Target::None,
            true
        )
        .unwrap()
        .as_value()
        .unwrap()
    );

    map.call_command(
        "method.insert",
        &Object::create_list_with(vec![
            Object::from("test_old_style.3"),
            Object::from("string"),
            Object::from("test.2"),
        ]),
        &Target::None,
        true,
    )
    .unwrap();
    assert_eq!(
        "test.2",
        map.call_command(
            "method.get",
            &Object::from("test_old_style.3"),
            &Target::None,
            true
        )
        .unwrap()
        .as_str()
        .unwrap()
    );
}

#[test]
fn test_insert_list() {
    let (map, _) = setup_dynamic();

    map.call_command(
        "method.insert.list",
        &Object::create_list_with(vec![Object::from("test_insert_list.1")]),
        &Target::None,
        true,
    )
    .unwrap();

    let res1 = map
        .call_command(
            "method.get",
            &Object::from("test_insert_list.1"),
            &Target::None,
            true,
        )
        .unwrap();
    assert!(res1.is_list());
    assert!(res1.as_list().unwrap().is_empty());

    map.call_command(
        "method.insert.list",
        &Object::create_list_with(vec![
            Object::from("test_insert_list.2"),
            Object::create_list_with(vec![Object::from("a"), Object::from("b")]),
        ]),
        &Target::None,
        true,
    )
    .unwrap();

    let res2 = map
        .call_command(
            "method.get",
            &Object::from("test_insert_list.2"),
            &Target::None,
            true,
        )
        .unwrap();
    assert!(res2.is_list());
    assert_eq!(2, res2.as_list().unwrap().len());
}

#[test]
fn test_value_base() {
    let (map, _) = setup_dynamic();

    let v1 = map
        .call_command("value", &Object::from("10"), &Target::None, true)
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(10, v1);

    let v2 = map
        .call_command(
            "value",
            &Object::create_list_with(vec![Object::from("ff"), Object::from(16i64)]),
            &Target::None,
            true,
        )
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(255, v2);
}

#[test]
fn test_math_and_logic() {
    let (map, _) = setup_dynamic();

    // Math
    let sum = map
        .call_command(
            "math.add",
            &Object::create_list_with(vec![
                Object::from(10i64),
                Object::from(20i64),
                Object::from(12i64),
            ]),
            &Target::None,
            true,
        )
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(42, sum);

    let diff = map
        .call_command(
            "math.sub",
            &Object::create_list_with(vec![Object::from(50i64), Object::from(8i64)]),
            &Target::None,
            true,
        )
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(42, diff);

    let prod = map
        .call_command(
            "math.mul",
            &Object::create_list_with(vec![Object::from(6i64), Object::from(7i64)]),
            &Target::None,
            true,
        )
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(42, prod);

    let div = map
        .call_command(
            "math.div",
            &Object::create_list_with(vec![Object::from(84i64), Object::from(2i64)]),
            &Target::None,
            true,
        )
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(42, div);

    // Logic
    let and_res = map
        .call_command(
            "and",
            &Object::create_list_with(vec![Object::from(1i64), Object::from(1i64)]),
            &Target::None,
            true,
        )
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(1, and_res);

    let or_res = map
        .call_command(
            "or",
            &Object::create_list_with(vec![Object::from(0i64), Object::from(1i64)]),
            &Target::None,
            true,
        )
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(1, or_res);

    let not_res = map
        .call_command("not", &Object::from(0i64), &Target::None, true)
        .unwrap()
        .as_value()
        .unwrap();
    assert_eq!(1, not_res);
}

#[test]
fn test_catch() {
    let (map, _) = setup_dynamic();

    // Catch suppressing invalid command
    let res = map
        .call_command(
            "catch",
            &Object::from("non_existent_command=foo"),
            &Target::None,
            true,
        )
        .unwrap();
    assert!(res.is_empty());
}
