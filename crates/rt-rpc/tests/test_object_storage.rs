use lt_common::Object;
use rt_rpc::object_flags::*;
use rt_rpc::{ObjectStorage, RpcError, KEY_SIZE};

#[test]
fn test_basics() {
    let mut storage = ObjectStorage::new();
    assert!(storage.is_empty());

    let node = storage
        .insert_str("test_1", Object::from("a"), FLAG_STRING_TYPE)
        .unwrap();
    assert_eq!(node.object, Object::from("a"));

    // Inserting same key again should fail
    assert!(matches!(
        storage.insert_str("test_1", Object::from("a"), FLAG_STRING_TYPE),
        Err(RpcError::InputError(_))
    ));

    // Test erase
    assert!(storage.erase_str("test_1"));
    assert!(storage.is_empty());

    // Test with no type flag
    assert!(matches!(
        storage.insert_str("test_2", Object::from("b"), 0),
        Err(RpcError::InputError(_))
    ));
    assert!(matches!(
        storage.insert_str("test_3", Object::from("c"), !MASK_TYPE),
        Err(RpcError::InputError(_))
    ));

    storage.clear();
}

#[test]
fn test_conversions() {
    let mut storage = ObjectStorage::new();

    assert!(storage
        .insert_str("test_1", Object::from("a"), FLAG_STRING_TYPE)
        .is_ok());
    assert!(storage
        .insert_str("test_2", Object::from("a"), FLAG_STRING_TYPE)
        .is_ok());

    let raw_3 = b"test_3";
    assert!(storage
        .insert(raw_3, Object::from("a"), FLAG_STRING_TYPE)
        .is_ok());
    assert_eq!(storage.get_str("test_3").unwrap(), &Object::from("a"));

    storage.clear();
}

#[test]
fn test_validate_keys() {
    let mut storage = ObjectStorage::new();

    let raw_string_4 = b"test_4\0foo";
    assert!(matches!(
        storage.insert(raw_string_4, Object::from("a"), FLAG_STRING_TYPE),
        Err(RpcError::InputError(_))
    ));

    assert!(matches!(
        storage.insert_str("", Object::from("a"), FLAG_STRING_TYPE),
        Err(RpcError::InputError(_))
    ));

    let key_max = "k".repeat(KEY_SIZE - 1);
    assert!(storage
        .insert_str(&key_max, Object::from("a"), FLAG_STRING_TYPE)
        .is_ok());

    let key_too_long = "k".repeat(KEY_SIZE);
    assert!(matches!(
        storage.insert_str(&key_too_long, Object::from("a"), FLAG_STRING_TYPE),
        Err(RpcError::InputError(_))
    ));

    // The over-long key must not have been stored as empty key
    assert!(storage.find_raw_string(b"").is_none());
    assert!(storage.find_raw_string(key_too_long.as_bytes()).is_none());

    storage.clear();
    storage
        .insert_str("test_5", Object::from("a"), FLAG_STRING_TYPE)
        .unwrap();

    // Lookup with nul-char must not match
    assert!(storage.find_raw_string(b"test_5\0x").is_none());
    assert!(storage.find_raw_string(b"test_5").is_some());

    storage.clear();
}

#[test]
fn test_access() {
    let mut storage = ObjectStorage::new();

    storage
        .insert_str("string_1", Object::from("gen_a"), FLAG_STRING_TYPE)
        .unwrap();
    storage
        .insert_str("value_1", Object::from(1i64), FLAG_VALUE_TYPE)
        .unwrap();

    assert_eq!(storage.get_str("string_1").unwrap(), &Object::from("gen_a"));
    assert_eq!(
        storage.set_str_string("string_1", "test").unwrap(),
        Object::from("test")
    );

    assert_eq!(storage.get_str("value_1").unwrap(), &Object::from(1i64));
    assert_eq!(
        storage.set_str_value("value_1", 2).unwrap(),
        Object::from(2i64)
    );
}

#[test]
fn test_multi_key_and_rlookup() {
    let mut storage = ObjectStorage::new();

    // Multi-type with rlookup requires FLAG_STATIC
    assert!(matches!(
        storage.insert_str(
            "multi_1",
            Object::create_empty(),
            FLAG_MULTI_TYPE | FLAG_RLOOKUP
        ),
        Err(RpcError::InputError(_))
    ));

    storage
        .insert_str(
            "multi_1",
            Object::create_empty(),
            FLAG_MULTI_TYPE | FLAG_STATIC | FLAG_RLOOKUP,
        )
        .unwrap();

    assert!(!storage.has_multi_key(b"multi_1", "cmd_a").unwrap());
    storage
        .set_multi_key_obj(b"multi_1", "cmd_a", Object::from("some_action"))
        .unwrap();

    assert!(storage.has_multi_key(b"multi_1", "cmd_a").unwrap());

    storage.erase_multi_key(b"multi_1", "cmd_a").unwrap();
    assert!(!storage.has_multi_key(b"multi_1", "cmd_a").unwrap());
}
