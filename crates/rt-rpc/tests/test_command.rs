use lt_common::object::ObjectVal;
use lt_common::Object;
use rt_rpc::{parse_command_object, CommandStack, PreparsedCommands, RpcError};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[test]
fn test_stack() {
    assert!(CommandStack::is_empty());

    {
        let _guard = CommandStack::push_stack(&[]);
        assert!(CommandStack::is_empty());
    }
    assert!(CommandStack::is_empty());

    // Test stack with one
    {
        let _guard = CommandStack::push_stack(&[Object::from(1i64)]);
        assert!(!CommandStack::is_empty());
        assert_eq!(CommandStack::argument(0), Object::from(1i64));
    }
    assert!(CommandStack::is_empty());

    // Test stack with two
    {
        let _guard = CommandStack::push_stack(&[Object::from(2i64), Object::from(3i64)]);
        assert!(!CommandStack::is_empty());
        assert_eq!(CommandStack::argument(0), Object::from(2i64));
        assert_eq!(CommandStack::argument(1), Object::from(3i64));
    }
    assert!(CommandStack::is_empty());
}

#[test]
fn test_stack_double() {
    let _guard1 = CommandStack::push_stack(&[Object::from(1i64)]);
    assert!(!CommandStack::is_empty());
    assert_eq!(CommandStack::argument(0), Object::from(1i64));

    {
        let _guard2 = CommandStack::push_stack(&[Object::from(2i64), Object::from(3i64)]);
        assert!(!CommandStack::is_empty());
        assert_eq!(CommandStack::argument(0), Object::from(2i64));
        assert_eq!(CommandStack::argument(1), Object::from(3i64));
    }

    assert!(!CommandStack::is_empty());
    assert_eq!(CommandStack::argument(0), Object::from(1i64));

    drop(_guard1);
    assert!(CommandStack::is_empty());
}

#[test]
fn test_preparsed_commands() {
    let prepare_count = Arc::new(AtomicU32::new(0));
    let prepare_count_clone = prepare_count.clone();

    let mut commands = PreparsedCommands::new(move |prepared| {
        prepare_count_clone.fetch_add(1, Ordering::SeqCst);
        prepared.prepare_if_needed();
        prepared.push_back(parse_command_object("string.length=abc").unwrap());
    });

    assert_eq!(prepare_count.load(Ordering::SeqCst), 0);
    assert!(commands.is_empty());

    commands.prepare_if_needed();
    assert_eq!(prepare_count.load(Ordering::SeqCst), 1);

    let mut count = 0;
    for itr in commands.iter() {
        assert!(itr.is_dict_key());
        count += 1;
    }
    assert_eq!(count, 1);
    assert_eq!(prepare_count.load(Ordering::SeqCst), 1);

    // Iterating again should not call prepare again
    for itr in commands.iter() {
        assert!(itr.is_dict_key());
    }
    assert_eq!(prepare_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_parse_command_object() {
    let command = parse_command_object("\tstring.length=abc  ").unwrap();
    if let ObjectVal::DictKey(key, obj) = command.val {
        assert_eq!(key, b"string.length");
        assert_eq!(*obj, Object::from("abc"));
    } else {
        panic!("Expected DictKey object");
    }

    // Must reject multipart command separated by ';'
    assert!(matches!(
        parse_command_object("string.length=abc;string.length=def"),
        Err(RpcError::InputError(_))
    ));

    // Must reject newline boundaries, including CRLF
    assert!(matches!(
        parse_command_object("string.length=abc\nstring.length=def"),
        Err(RpcError::InputError(_))
    ));
    assert!(matches!(
        parse_command_object("string.length=abc\r\nstring.length=def"),
        Err(RpcError::InputError(_))
    ));
}
