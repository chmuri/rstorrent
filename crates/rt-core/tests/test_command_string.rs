use lt_common::Object;
use rt_core::commands::string::register_string_commands;
use rt_rpc::parse_commands::parse_command;
use rt_rpc::{CommandMap, Target};

fn setup_commands() -> CommandMap {
    let mut map = CommandMap::new();
    register_string_commands(&mut map).unwrap();
    map
}

fn call_string(map: &CommandMap, key: &str, objects: Vec<Object>) -> String {
    let args = Object::create_list_with(objects);
    let res = map.call_command(key, &args, &Target::None, true).unwrap();
    res.as_str().unwrap().to_string()
}

fn call_value(map: &CommandMap, key: &str, objects: Vec<Object>) -> i64 {
    let args = Object::create_list_with(objects);
    let res = map.call_command(key, &args, &Target::None, true).unwrap();
    res.as_value().unwrap()
}

fn call_list(map: &CommandMap, key: &str, objects: Vec<Object>) -> Vec<Object> {
    let args = Object::create_list_with(objects);
    let res = map.call_command(key, &args, &Target::None, true).unwrap();
    res.as_list().unwrap().to_vec()
}

fn parse(map: &CommandMap, command: &str) -> Object {
    parse_command(command, &Target::None, map, true).unwrap().0
}

#[test]
fn test_length() {
    let map = setup_commands();
    assert_eq!(0, call_value(&map, "string.length", vec![Object::from("")]));
    assert_eq!(
        3,
        call_value(&map, "string.length", vec![Object::from("abc")])
    );

    // The length is counted in utf-8 characters, not bytes.
    assert_eq!(
        5,
        call_value(&map, "string.length", vec![Object::from("héllo")])
    );
    assert_eq!(
        3,
        call_value(&map, "string.length", vec![Object::from("日本語")])
    );

    // Values are converted to their string representation.
    assert_eq!(
        4,
        call_value(&map, "string.length", vec![Object::from(1234i64)])
    );
}

#[test]
fn test_equals() {
    let map = setup_commands();
    assert_eq!(
        1,
        call_value(
            &map,
            "string.equals",
            vec![Object::from("abc"), Object::from("abc")]
        )
    );
    assert_eq!(
        0,
        call_value(
            &map,
            "string.equals",
            vec![Object::from("abc"), Object::from("abd")]
        )
    );
    assert_eq!(
        0,
        call_value(
            &map,
            "string.equals",
            vec![Object::from("abc"), Object::from("ab")]
        )
    );

    assert_eq!(
        1,
        call_value(
            &map,
            "string.equals",
            vec![Object::from("abc"), Object::from("x"), Object::from("abc")]
        )
    );
    assert_eq!(
        0,
        call_value(
            &map,
            "string.equals",
            vec![Object::from("abc"), Object::from("x"), Object::from("y")]
        )
    );

    assert_eq!(
        1,
        call_value(
            &map,
            "string.equals",
            vec![Object::from(42i64), Object::from("42")]
        )
    );
}

#[test]
fn test_starts_with() {
    let map = setup_commands();
    assert_eq!(
        1,
        call_value(
            &map,
            "string.starts_with",
            vec![Object::from("abcdef"), Object::from("abc")]
        )
    );
    assert_eq!(
        1,
        call_value(
            &map,
            "string.starts_with",
            vec![Object::from("abcdef"), Object::from("")]
        )
    );
    assert_eq!(
        0,
        call_value(
            &map,
            "string.starts_with",
            vec![Object::from("abcdef"), Object::from("bcd")]
        )
    );
    assert_eq!(
        0,
        call_value(
            &map,
            "string.starts_with",
            vec![Object::from("ab"), Object::from("abc")]
        )
    );

    assert_eq!(
        1,
        call_value(
            &map,
            "string.starts_with",
            vec![
                Object::from("abcdef"),
                Object::from("x"),
                Object::from("ab")
            ]
        )
    );
}

#[test]
fn test_ends_with() {
    let map = setup_commands();
    assert_eq!(
        1,
        call_value(
            &map,
            "string.ends_with",
            vec![Object::from("abcdef"), Object::from("def")]
        )
    );
    assert_eq!(
        1,
        call_value(
            &map,
            "string.ends_with",
            vec![Object::from("abcdef"), Object::from("")]
        )
    );
    assert_eq!(
        0,
        call_value(
            &map,
            "string.ends_with",
            vec![Object::from("abcdef"), Object::from("cde")]
        )
    );
    assert_eq!(
        0,
        call_value(
            &map,
            "string.ends_with",
            vec![Object::from("ef"), Object::from("def")]
        )
    );

    assert_eq!(
        1,
        call_value(
            &map,
            "string.ends_with",
            vec![
                Object::from("a.torrent"),
                Object::from(".rar"),
                Object::from(".torrent")
            ]
        )
    );
}

#[test]
fn test_contains() {
    let map = setup_commands();
    assert_eq!(
        1,
        call_value(
            &map,
            "string.contains",
            vec![Object::from("abcdef"), Object::from("cde")]
        )
    );
    assert_eq!(
        0,
        call_value(
            &map,
            "string.contains",
            vec![Object::from("abcdef"), Object::from("ace")]
        )
    );
    assert_eq!(
        1,
        call_value(
            &map,
            "string.contains",
            vec![
                Object::from("abcdef"),
                Object::from("x"),
                Object::from("bcd")
            ]
        )
    );

    assert_eq!(
        0,
        call_value(
            &map,
            "string.contains",
            vec![Object::from("retracker.local"), Object::from("RETRACKER")]
        )
    );
    assert_eq!(
        1,
        call_value(
            &map,
            "string.contains_i",
            vec![Object::from("retracker.local"), Object::from("RETRACKER")]
        )
    );
    assert_eq!(
        1,
        call_value(
            &map,
            "string.contains_i",
            vec![Object::from("RETRACKER.LOCAL"), Object::from("retracker")]
        )
    );
    assert_eq!(
        0,
        call_value(
            &map,
            "string.contains_i",
            vec![Object::from("abcdef"), Object::from("xyz")]
        )
    );
}

#[test]
fn test_substr() {
    let map = setup_commands();
    assert_eq!(
        "abcdef",
        call_string(&map, "string.substr", vec![Object::from("abcdef")])
    );
    assert_eq!(
        "cdef",
        call_string(
            &map,
            "string.substr",
            vec![Object::from("abcdef"), Object::from(2i64)]
        )
    );
    assert_eq!(
        "cde",
        call_string(
            &map,
            "string.substr",
            vec![
                Object::from("abcdef"),
                Object::from(2i64),
                Object::from(3i64)
            ]
        )
    );
    assert_eq!(
        "cdef",
        call_string(
            &map,
            "string.substr",
            vec![
                Object::from("abcdef"),
                Object::from(2i64),
                Object::from(100i64)
            ]
        )
    );
    assert_eq!(
        "",
        call_string(
            &map,
            "string.substr",
            vec![
                Object::from("abcdef"),
                Object::from(2i64),
                Object::from(0i64)
            ]
        )
    );

    // Negative positions are relative to the end of the string.
    assert_eq!(
        "ef",
        call_string(
            &map,
            "string.substr",
            vec![Object::from("abcdef"), Object::from(-2i64)]
        )
    );
    assert_eq!(
        "e",
        call_string(
            &map,
            "string.substr",
            vec![
                Object::from("abcdef"),
                Object::from(-2i64),
                Object::from(1i64)
            ]
        )
    );

    // Out-of-bounds positions return the default value.
    assert_eq!(
        "",
        call_string(
            &map,
            "string.substr",
            vec![Object::from("abcdef"), Object::from(10i64)]
        )
    );
    assert_eq!(
        "n/a",
        call_string(
            &map,
            "string.substr",
            vec![
                Object::from("abcdef"),
                Object::from(10i64),
                Object::from(1i64),
                Object::from("n/a")
            ]
        )
    );
    assert_eq!(
        "n/a",
        call_string(
            &map,
            "string.substr",
            vec![
                Object::from("abcdef"),
                Object::from(-10i64),
                Object::from(1i64),
                Object::from("n/a")
            ]
        )
    );

    // Positions and counts are in utf-8 characters, not bytes.
    assert_eq!(
        "本",
        call_string(
            &map,
            "string.substr",
            vec![
                Object::from("日本語"),
                Object::from(1i64),
                Object::from(1i64)
            ]
        )
    );
    assert_eq!(
        "本語",
        call_string(
            &map,
            "string.substr",
            vec![Object::from("日本語"), Object::from(-2i64)]
        )
    );
}

#[test]
fn test_split() {
    let map = setup_commands();
    let parts = call_list(
        &map,
        "string.split",
        vec![Object::from("a,b,c"), Object::from(",")],
    );
    assert_eq!(3, parts.len());
    assert_eq!("a", parts.first().unwrap().as_str().unwrap());
    assert_eq!("c", parts.last().unwrap().as_str().unwrap());

    // Empty fields are preserved.
    assert_eq!(
        3,
        call_list(
            &map,
            "string.split",
            vec![Object::from("a,,b"), Object::from(",")]
        )
        .len()
    );
    assert_eq!(
        1,
        call_list(
            &map,
            "string.split",
            vec![Object::from("abc"), Object::from(",")]
        )
        .len()
    );

    // Multi-char delimiter.
    assert_eq!(
        2,
        call_list(
            &map,
            "string.split",
            vec![Object::from("a::b"), Object::from("::")]
        )
        .len()
    );

    // Empty delimiter splits UTF-8 chars.
    let characters = call_list(
        &map,
        "string.split",
        vec![Object::from("日本語"), Object::from("")],
    );
    assert_eq!(3, characters.len());
    assert_eq!("日", characters.first().unwrap().as_str().unwrap());
}

#[test]
fn test_join() {
    let map = setup_commands();
    assert_eq!(
        "a, b",
        call_string(
            &map,
            "string.join",
            vec![Object::from(", "), Object::from("a"), Object::from("b")]
        )
    );
    assert_eq!(
        "ab",
        call_string(
            &map,
            "string.join",
            vec![Object::from(""), Object::from("a"), Object::from("b")]
        )
    );
    assert_eq!(
        "",
        call_string(&map, "string.join", vec![Object::from(", ")])
    );
    assert_eq!(
        "a",
        call_string(
            &map,
            "string.join",
            vec![Object::from(", "), Object::from("a")]
        )
    );
    assert_eq!(
        "1-2",
        call_string(
            &map,
            "string.join",
            vec![Object::from("-"), Object::from(1i64), Object::from(2i64)]
        )
    );

    // Lists are flattened.
    assert_eq!(
        "a-b-c",
        call_string(
            &map,
            "string.join",
            vec![
                Object::from("-"),
                Object::create_list_with(vec![Object::from("a"), Object::from("b")]),
                Object::from("c"),
            ]
        )
    );
}

#[test]
fn test_pad() {
    let map = setup_commands();
    assert_eq!(
        "  7",
        call_string(
            &map,
            "string.lpad",
            vec![Object::from("7"), Object::from(3i64)]
        )
    );
    assert_eq!(
        "7  ",
        call_string(
            &map,
            "string.rpad",
            vec![Object::from("7"), Object::from(3i64)]
        )
    );
    assert_eq!(
        "007",
        call_string(
            &map,
            "string.lpad",
            vec![Object::from("7"), Object::from(3i64), Object::from("0")]
        )
    );
    assert_eq!(
        "700",
        call_string(
            &map,
            "string.rpad",
            vec![Object::from("7"), Object::from(3i64), Object::from("0")]
        )
    );

    // Already long enough
    assert_eq!(
        "abcd",
        call_string(
            &map,
            "string.lpad",
            vec![Object::from("abcd"), Object::from(2i64)]
        )
    );
    assert_eq!(
        "abcd",
        call_string(
            &map,
            "string.rpad",
            vec![Object::from("abcd"), Object::from(4i64)]
        )
    );

    // Multi-char padding repeated
    assert_eq!(
        "axyx",
        call_string(
            &map,
            "string.rpad",
            vec![Object::from("a"), Object::from(4i64), Object::from("xy")]
        )
    );
    assert_eq!(
        "a",
        call_string(
            &map,
            "string.rpad",
            vec![Object::from("a"), Object::from(4i64), Object::from("")]
        )
    );

    // Padding in utf-8 chars
    assert_eq!(
        "00日",
        call_string(
            &map,
            "string.lpad",
            vec![Object::from("日"), Object::from(3i64), Object::from("0")]
        )
    );
}

#[test]
fn test_strip() {
    let map = setup_commands();
    assert_eq!(
        "a b",
        call_string(&map, "string.strip", vec![Object::from("  a b  ")])
    );
    assert_eq!(
        "a",
        call_string(&map, "string.strip", vec![Object::from("\t\n a \r\n")])
    );
    assert_eq!(
        "a  ",
        call_string(&map, "string.lstrip", vec![Object::from("  a  ")])
    );
    assert_eq!(
        "  a",
        call_string(&map, "string.rstrip", vec![Object::from("  a  ")])
    );

    assert_eq!(
        "a",
        call_string(
            &map,
            "string.strip",
            vec![Object::from("xxaxx"), Object::from("x")]
        )
    );
    assert_eq!(
        "a",
        call_string(
            &map,
            "string.strip",
            vec![
                Object::from("/x/a/x/"),
                Object::from("/"),
                Object::from("x")
            ]
        )
    );
    assert_eq!(
        "",
        call_string(
            &map,
            "string.strip",
            vec![Object::from("aaa"), Object::from("a")]
        )
    );
    assert_eq!(
        "  a  ",
        call_string(
            &map,
            "string.strip",
            vec![Object::from("  a  "), Object::from("")]
        )
    );

    // Strippable is a set of UTF-8 chars
    assert_eq!(
        "a",
        call_string(
            &map,
            "string.strip",
            vec![Object::from("日a日"), Object::from("日")]
        )
    );
}

#[test]
fn test_map_and_replace() {
    let map = setup_commands();

    let pair_ab = Object::create_list_with(vec![Object::from("a"), Object::from("b")]);
    assert_eq!(
        "b",
        call_string(&map, "string.map", vec![Object::from("a"), pair_ab.clone()])
    );
    assert_eq!(
        "c",
        call_string(&map, "string.map", vec![Object::from("c"), pair_ab])
    );

    let pair_hyphen = Object::create_list_with(vec![Object::from("-"), Object::from("+")]);
    assert_eq!(
        "a+b+c",
        call_string(
            &map,
            "string.replace",
            vec![Object::from("a-b-c"), pair_hyphen]
        )
    );

    let pair_empty = Object::create_list_with(vec![Object::from("-"), Object::from("")]);
    assert_eq!(
        "abc",
        call_string(
            &map,
            "string.replace",
            vec![Object::from("a-b-c"), pair_empty]
        )
    );
}

#[test]
fn test_config_syntax() {
    let map = setup_commands();
    assert_eq!(3, parse(&map, "string.length=abc").as_value().unwrap());
    assert_eq!(
        1,
        parse(&map, "string.contains=retracker.local,retracker")
            .as_value()
            .unwrap()
    );
    assert_eq!(
        1,
        parse(
            &map,
            "string.starts_with=udp://tracker.example.com,http://,udp://"
        )
        .as_value()
        .unwrap()
    );
    assert_eq!(
        "cde",
        parse(&map, "string.substr=abcdef,2,3").as_str().unwrap()
    );
    assert_eq!(
        "padded",
        parse(&map, "string.strip=\"  padded  \"").as_str().unwrap()
    );
    assert_eq!(
        "a+b+c",
        parse(&map, "string.replace=a-b-c,{-,+}").as_str().unwrap()
    );
    assert_eq!(
        "y",
        parse(&map, "string.map=x,{a,b},{x,y}").as_str().unwrap()
    );
    assert_eq!(
        "a-b-c",
        parse(&map, "string.join=-,(string.split,a.b.c,.)")
            .as_str()
            .unwrap()
    );
}
