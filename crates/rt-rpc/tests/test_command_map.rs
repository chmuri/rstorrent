use lt_common::Object;
use rt_rpc::command_flags::*;
use rt_rpc::{CommandMap, RpcError, Target};

#[test]
fn test_basics() {
    let mut map = CommandMap::new();

    map.insert_func(
        "test_a",
        |_target, obj| Ok(obj.clone()),
        FLAG_DONT_DELETE | FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    map.insert_func(
        "test_b",
        |_target, _obj| Ok(Object::from(2i64)),
        FLAG_DONT_DELETE | FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    map.insert_func(
        "any_string",
        |_target, _obj| Ok(Object::from(3i64)),
        FLAG_DONT_DELETE | FLAG_PUBLIC_RPC,
        "",
        "",
    )
    .unwrap();

    let target = Target::None;
    assert_eq!(
        map.call_command("test_a", &Object::from(1i64), &target, true).unwrap(),
        Object::from(1i64)
    );
    assert_eq!(
        map.call_command("test_b", &Object::from(1i64), &target, true).unwrap(),
        Object::from(2i64)
    );
    assert_eq!(
        map.call_command("any_string", &Object::from(""), &target, true).unwrap(),
        Object::from(3i64)
    );
}

#[test]
fn test_redirect_and_erase() {
    let mut map = CommandMap::new();

    map.insert_func(
        "orig_cmd",
        |_target, obj| Ok(obj.clone()),
        FLAG_IS_REDIRECT | FLAG_PUBLIC_RPC,
        "parm",
        "doc",
    )
    .unwrap();

    // Create redirect
    map.create_redirect("alias_cmd", "orig_cmd", 0).unwrap();

    // Alias works
    let target = Target::None;
    assert_eq!(
        map.call_command("alias_cmd", &Object::from(42i64), &target, true).unwrap(),
        Object::from(42i64)
    );

    // Erasing orig_cmd should fail because it has redirects
    assert!(matches!(
        map.erase("orig_cmd"),
        Err(RpcError::InputError(_))
    ));

    // Erasing alias_cmd should succeed
    assert!(map.erase("alias_cmd").is_ok());
}

#[test]
fn test_untrusted() {
    let mut map = CommandMap::new();

    map.insert_func(
        "safe_cmd",
        |_target, _obj| Ok(Object::from(10i64)),
        FLAG_UNTRUSTED_SAFE,
        "",
        "",
    )
    .unwrap();

    map.insert_func(
        "unsafe_cmd",
        |_target, _obj| Ok(Object::from(20i64)),
        0,
        "",
        "",
    )
    .unwrap();

    let target = Target::None;
    // Trusted connection can call both
    assert_eq!(
        map.call_command("safe_cmd", &Object::create_empty(), &target, true).unwrap(),
        Object::from(10i64)
    );
    assert_eq!(
        map.call_command("unsafe_cmd", &Object::create_empty(), &target, true).unwrap(),
        Object::from(20i64)
    );

    // Untrusted connection can only call safe_cmd
    assert_eq!(
        map.call_command("safe_cmd", &Object::create_empty(), &target, false).unwrap(),
        Object::from(10i64)
    );
    assert!(matches!(
        map.call_command("unsafe_cmd", &Object::create_empty(), &target, false),
        Err(RpcError::UntrustedError(_))
    ));
}
