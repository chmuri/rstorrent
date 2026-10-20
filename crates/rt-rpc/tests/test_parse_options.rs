use rt_rpc::{
    parse_option_flag, parse_option_flags, parse_option_print_flags, parse_option_print_vector,
    RpcError,
};

const FLAG_1: i32 = 1 << 0;
const FLAG_2: i32 = 1 << 1;
const FLAG_3: i32 = 1 << 2;
const FLAG_4: i32 = 1 << 3;
const FLAG_5: i32 = 1 << 4;
const FLAG_6: i32 = 1 << 5;

const FLAG_LIST: &[(&str, i32)] = &[
    ("foo", FLAG_1),
    ("bar", FLAG_2),
    ("baz", FLAG_3),
    ("a12", FLAG_4),
    ("not_bar", !FLAG_2),
    ("45b", FLAG_5),
    ("a_b_c__e_3_g", FLAG_6),
    ("not_a12", !FLAG_4),
];

fn flag_to_int(flag: &str) -> Result<i32, RpcError> {
    for (name, val) in FLAG_LIST {
        if *name == flag {
            return Ok(*val);
        }
    }
    Err(RpcError::InputError("unknown flag".to_string()))
}

fn int_to_flag(flag: u32) -> Result<String, RpcError> {
    for (name, val) in FLAG_LIST {
        if *val > 0 && (*val as u32) == flag {
            return Ok(name.to_string());
        }
    }
    Err(RpcError::InputError("unknown flag".to_string()))
}

#[test]
fn test_flag_basic() {
    assert_eq!(parse_option_flag("foo", flag_to_int).unwrap(), FLAG_1);
    assert_eq!(parse_option_flag(" foo ", flag_to_int).unwrap(), FLAG_1);
    assert_eq!(
        parse_option_flag("    foo     ", flag_to_int).unwrap(),
        FLAG_1
    );
    assert_eq!(parse_option_flag("a12", flag_to_int).unwrap(), FLAG_4);
    assert_eq!(parse_option_flag("45b", flag_to_int).unwrap(), FLAG_5);
    assert_eq!(
        parse_option_flag("a_b_c__e_3_g", flag_to_int).unwrap(),
        FLAG_6
    );
}

#[test]
fn test_flag_error() {
    assert!(parse_option_flag("", flag_to_int).is_err());
    assert!(parse_option_flag("foo|bar", flag_to_int).is_err());
    assert!(parse_option_flag("foo|bar|baz", flag_to_int).is_err());
    assert!(parse_option_flag("foo |bar", flag_to_int).is_err());
    assert!(parse_option_flag("foo | bar| baz", flag_to_int).is_err());
}

#[test]
fn test_flags_basic() {
    assert_eq!(parse_option_flags("", flag_to_int, 0).unwrap(), 0);
    assert_eq!(parse_option_flags("foo", flag_to_int, 0).unwrap(), FLAG_1);
    assert_eq!(
        parse_option_flags("foo|bar", flag_to_int, 0).unwrap(),
        FLAG_1 | FLAG_2
    );
    assert_eq!(
        parse_option_flags("foo|bar|baz", flag_to_int, 0).unwrap(),
        FLAG_1 | FLAG_2 | FLAG_3
    );

    assert_eq!(parse_option_flags(" foo ", flag_to_int, 0).unwrap(), FLAG_1);
    assert_eq!(
        parse_option_flags("    foo     ", flag_to_int, 0).unwrap(),
        FLAG_1
    );
    assert_eq!(
        parse_option_flags("foo |bar", flag_to_int, 0).unwrap(),
        FLAG_1 | FLAG_2
    );
    assert_eq!(
        parse_option_flags("foo | bar| baz", flag_to_int, 0).unwrap(),
        FLAG_1 | FLAG_2 | FLAG_3
    );

    assert_eq!(parse_option_flags("a12", flag_to_int, 0).unwrap(), FLAG_4);
    assert_eq!(parse_option_flags("45b", flag_to_int, 0).unwrap(), FLAG_5);
    assert_eq!(
        parse_option_flags("a_b_c__e_3_g", flag_to_int, 0).unwrap(),
        FLAG_6
    );
}

#[test]
fn test_flags_error() {
    assert!(parse_option_flags("fooa", flag_to_int, 0).is_err());
    assert!(parse_option_flags("afoo", flag_to_int, 0).is_err());
    assert!(parse_option_flags("|", flag_to_int, 0).is_err());
    assert!(parse_option_flags("foo|", flag_to_int, 0).is_err());
    assert!(parse_option_flags("|foo", flag_to_int, 0).is_err());
    assert!(parse_option_flags(",foo", flag_to_int, 0).is_err());
}

#[test]
fn test_flags_complex() {
    assert_eq!(parse_option_flags("", flag_to_int, 0).unwrap(), 0);
    assert_eq!(parse_option_flags("", flag_to_int, FLAG_1).unwrap(), FLAG_1);
    assert_eq!(
        parse_option_flags("bar", flag_to_int, FLAG_1).unwrap(),
        FLAG_1 | FLAG_2
    );
    assert_eq!(
        parse_option_flags("bar|baz", flag_to_int, FLAG_1).unwrap(),
        FLAG_1 | FLAG_2 | FLAG_3
    );

    assert_eq!(
        parse_option_flags("not_bar", flag_to_int, FLAG_2).unwrap(),
        0
    );
    assert_eq!(
        parse_option_flags("not_bar|not_a12", flag_to_int, FLAG_2).unwrap(),
        0
    );

    assert_eq!(
        parse_option_flags("bar|not_a12", flag_to_int, FLAG_3 | FLAG_4).unwrap(),
        FLAG_2 | FLAG_3
    );
}

#[test]
fn test_flags_print_vector() {
    assert_eq!(parse_option_print_vector(0, FLAG_LIST), "not_bar|not_a12");
    assert_eq!(parse_option_print_vector(FLAG_2, FLAG_LIST), "bar|not_a12");
    assert_eq!(
        parse_option_print_vector(FLAG_2 | FLAG_4, FLAG_LIST),
        "bar|a12"
    );
    assert_eq!(
        parse_option_print_vector(FLAG_1, FLAG_LIST),
        "foo|not_bar|not_a12"
    );
}

#[test]
fn test_flags_print_flags() {
    assert_eq!(parse_option_print_flags(0, int_to_flag).unwrap(), "");
    assert_eq!(
        parse_option_print_flags(FLAG_1 as u32, int_to_flag).unwrap(),
        "foo"
    );
    assert_eq!(
        parse_option_print_flags((FLAG_1 | FLAG_2) as u32, int_to_flag).unwrap(),
        "foo|bar"
    );
    assert_eq!(
        parse_option_print_flags((FLAG_1 | FLAG_2 | FLAG_3) as u32, int_to_flag).unwrap(),
        "foo|bar|baz"
    );

    assert!(parse_option_print_flags(100, int_to_flag).is_err());
}
