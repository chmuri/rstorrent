use rt_rpc::{parse_value_nothrow, parse_whole_value_nothrow};

#[test]
fn test_whole_value_in_range() {
    assert_eq!(
        parse_whole_value_nothrow("9223372036854775807", 0, 1),
        Ok(i64::MAX)
    );
    assert_eq!(
        parse_whole_value_nothrow("-9223372036854775808", 0, 1),
        Ok(i64::MIN)
    );
}

#[test]
fn test_whole_value_out_of_range() {
    assert!(parse_whole_value_nothrow("9223372036854775808", 0, 1).is_err());
    assert!(parse_whole_value_nothrow("-9223372036854775809", 0, 1).is_err());
    assert!(parse_whole_value_nothrow("99999999999999999999999999", 0, 1).is_err());
}

#[test]
fn test_whole_value_bases() {
    assert_eq!(parse_whole_value_nothrow("0x1f", 0, 1), Ok(31));
    assert_eq!(parse_whole_value_nothrow("0X1F", 0, 1), Ok(31));
    assert!(parse_whole_value_nothrow("-0x1f", 0, 1).is_err());

    assert_eq!(parse_whole_value_nothrow("0022", 0, 1), Ok(0o22));
    assert!(parse_whole_value_nothrow("0028", 0, 1).is_err());
    assert!(parse_whole_value_nothrow("+022", 0, 1).is_err());
    assert!(parse_whole_value_nothrow("-022", 0, 1).is_err());

    assert_eq!(parse_whole_value_nothrow("22", 0, 1), Ok(22));

    assert_eq!(parse_whole_value_nothrow("1f", 16, 1), Ok(31));
    assert_eq!(parse_whole_value_nothrow("0x1f", 16, 1), Ok(31));

    assert_eq!(parse_whole_value_nothrow("22", 8, 1), Ok(0o22));
    assert_eq!(parse_whole_value_nothrow("022", 8, 1), Ok(0o22));

    let no_digits = "0x";
    assert!(parse_whole_value_nothrow(no_digits, 0, 1).is_err());
    let (_, offset) = parse_value_nothrow(no_digits, 0, 1).unwrap();
    assert_eq!(offset, 1);

    let bad_digits = "0xzz";
    assert!(parse_whole_value_nothrow(bad_digits, 0, 1).is_err());
    let (_, offset2) = parse_value_nothrow(bad_digits, 0, 1).unwrap();
    assert_eq!(offset2, 1);
}

#[test]
fn test_whole_value_prefixes() {
    assert!(parse_whole_value_nothrow("+5", 0, 1).is_err());

    assert_eq!(parse_whole_value_nothrow("  5  ", 0, 1), Ok(5));
    assert_eq!(parse_whole_value_nothrow("5k", 0, 1), Ok(5120));
    assert_eq!(parse_whole_value_nothrow("yes", 0, 1), Ok(1));
    assert_eq!(parse_whole_value_nothrow("false", 0, 1), Ok(0));

    assert!(parse_whole_value_nothrow("junk", 0, 1).is_err());
    assert!(parse_whole_value_nothrow("", 0, 1).is_err());
}

#[test]
fn test_dsl_commands() {
    use lt_common::Object;
    use rt_rpc::{parse_command_multiple, CommandMap, Target};

    let mut map = CommandMap::new();

    map.insert_func(
        "math.add",
        |_target, obj| {
            if let Ok(list) = obj.as_list() {
                let a = list
                    .first()
                    .and_then(|x| rt_rpc::convert_to_value(x, 0, 1).ok())
                    .unwrap_or(0);
                let b = list
                    .get(1)
                    .and_then(|x| rt_rpc::convert_to_value(x, 0, 1).ok())
                    .unwrap_or(0);
                Ok(Object::from(a + b))
            } else {
                Ok(Object::from(0i64))
            }
        },
        0,
        "",
        "",
    )
    .unwrap();

    map.insert_func("test.echo", |_target, obj| Ok(obj.clone()), 0, "", "")
        .unwrap();

    let target = Target::None;

    // Test simple command
    let res = parse_command_multiple("test.echo=hello", &target, &map, true).unwrap();
    assert_eq!(res, Object::from("hello"));

    // Test nested function (math.add, 10, 20)
    let res2 = parse_command_multiple("test.echo=(math.add, 10, 20)", &target, &map, true).unwrap();
    assert_eq!(res2, Object::from(30i64));

    // Test list {1, 2, 3}
    let res3 = parse_command_multiple("test.echo={1, 2, 3}", &target, &map, true).unwrap();
    assert!(res3.is_list());
    assert_eq!(res3.as_list().unwrap().len(), 3);

    // Test multiple commands separated by ';'
    let res4 =
        parse_command_multiple("test.echo=first; test.echo=second", &target, &map, true).unwrap();
    assert_eq!(res4, Object::from("second"));
}
