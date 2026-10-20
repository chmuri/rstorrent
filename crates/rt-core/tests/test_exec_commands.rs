use std::time::{Duration, Instant};

use lt_common::Object;
use rt_core::commands::exec::register_exec_commands;
use rt_rpc::{CommandMap, Target};

fn map() -> CommandMap {
    let mut map = CommandMap::new();
    register_exec_commands(&mut map).unwrap();
    map
}

fn call(map: &CommandMap, name: &str, args: Vec<&str>) -> Result<Object, rt_rpc::RpcError> {
    let list = Object::create_list_with(args.into_iter().map(Object::from).collect());
    map.call_command(name, &list, &Target::None, true)
}

fn call_objects(
    map: &CommandMap,
    name: &str,
    args: Vec<Object>,
) -> Result<Object, rt_rpc::RpcError> {
    let list = Object::create_list_with(args);
    map.call_command(name, &list, &Target::None, true)
}

fn bin_prog(name: &str) -> String {
    for dir in ["/bin", "/usr/bin"] {
        let p = format!("{dir}/{name}");
        if std::path::Path::new(&p).exists() {
            return p;
        }
    }
    name.to_string()
}

#[test]
fn all_execute_variants_are_registered() {
    let map = map();
    let names = [
        "execute",
        "execute.throw",
        "execute.throw.bg",
        "execute.nothrow",
        "execute.nothrow.bg",
        "execute.raw",
        "execute.raw.bg",
        "execute.raw_nothrow",
        "execute.raw_nothrow.bg",
        "execute.capture",
        "execute.capture_nothrow",
        // Legacy redirect from old/src/main.cc.
        "execute2",
    ];

    for name in names {
        assert!(map.has(name), "missing command {name}");
    }
}

#[test]
fn capture_returns_stdout() {
    let map = map();
    let result = call(&map, "execute.capture", vec!["/bin/echo", "hello"]).unwrap();
    // stdout is returned verbatim: a newline produced by the child is kept.
    assert_eq!(result.as_string().unwrap(), b"hello\n");
}

#[test]
fn capture_does_not_add_a_trailing_newline() {
    let map = map();
    let result = call(
        &map,
        "execute.capture",
        vec!["/bin/sh", "-c", "printf hello"],
    )
    .unwrap();
    assert_eq!(result.as_string().unwrap(), b"hello");
}

#[test]
fn capture_preserves_binary_output() {
    let map = map();
    let result = call(
        &map,
        "execute.capture",
        vec!["/bin/sh", "-c", "printf '\\377\\000A'"],
    )
    .unwrap();

    // Invalid UTF-8 and NUL bytes must survive unchanged.
    assert_eq!(result.as_string().unwrap(), &[0xff, 0x00, b'A']);
}

#[test]
fn capture_nothrow_returns_output_on_failure() {
    let map = map();
    let output = call(
        &map,
        "execute.capture_nothrow",
        vec!["/bin/sh", "-c", "printf out; exit 3"],
    )
    .unwrap();
    assert_eq!(output.as_string().unwrap(), b"out");

    let error = call(
        &map,
        "execute.capture",
        vec!["/bin/sh", "-c", "printf out; exit 3"],
    )
    .unwrap_err();
    assert!(error.to_string().contains("non-zero exit status: 3"));
}

#[test]
fn throw_and_nothrow_semantics() {
    let map = map();
    let t = bin_prog("true");
    let f = bin_prog("false");

    assert_eq!(
        call(&map, "execute", vec![&t])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert!(call(&map, "execute", vec![&f]).is_err());
    assert!(call(&map, "execute.throw", vec![&f]).is_err());

    let result = call(&map, "execute.nothrow", vec![&f]).unwrap();
    assert_eq!(result.as_value().unwrap(), 1);

    let raw = call(&map, "execute.raw_nothrow", vec![&f]).unwrap();
    assert_eq!(raw.as_value().unwrap(), 1);
}

#[test]
fn throw_reports_signal_death_separately_from_exit_codes() {
    let map = map();

    let error = call(&map, "execute", vec!["/bin/sh", "-c", "kill -TERM $$"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("terminated by signal"), "{error}");

    let status = call(
        &map,
        "execute.nothrow",
        vec!["/bin/sh", "-c", "kill -TERM $$"],
    )
    .unwrap();
    assert_eq!(status.as_value().unwrap(), 128 + 15);
}

#[test]
fn spawn_failures_are_distinct_and_thrown_in_background() {
    let map = map();
    let missing = "/definitely/not/a/real/rtorrent-exec-command";

    let error = call(&map, "execute", vec![missing])
        .unwrap_err()
        .to_string();
    assert!(error.contains("Failed to spawn command"), "{error}");

    let error = call(&map, "execute.throw.bg", vec![missing])
        .unwrap_err()
        .to_string();
    assert!(error.contains("Failed to spawn command"), "{error}");

    // Non-throwing background still reports an inability to spawn.
    assert!(call(&map, "execute.raw_nothrow.bg", vec![missing]).is_err());
}

#[test]
fn rejects_missing_or_empty_command() {
    let map = map();

    let empty = Object::create_list();
    assert!(map
        .call_command("execute", &empty, &Target::None, true)
        .is_err());

    let empty_command = call_objects(
        &map,
        "execute",
        vec![Object::from(""), Object::from("argument")],
    )
    .unwrap_err()
    .to_string();
    assert!(
        empty_command.contains("Command path cannot be empty"),
        "{empty_command}"
    );
}

#[test]
fn enforces_argument_limit() {
    let map = map();
    let t = bin_prog("true");

    // Command path plus the maximum of 128 arguments is accepted.
    let mut max_args = vec![Object::from(t.clone())];
    max_args.extend((0..128).map(|index| Object::from(format!("arg-{index}"))));
    assert!(call_objects(&map, "execute", max_args).is_ok());

    // One more argument is rejected before spawning.
    let mut too_many = vec![Object::from(t)];
    too_many.extend((0..129).map(|index| Object::from(format!("arg-{index}"))));
    let error = call_objects(&map, "execute", too_many)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Too many arguments"), "{error}");
}

#[test]
fn background_never_blocks_the_rpc_caller() {
    let map = map();
    let start = Instant::now();
    let sleep_cmd = bin_prog("sleep");
    let result = call(&map, "execute.throw.bg", vec![&sleep_cmd, "2"]).unwrap();
    let elapsed = start.elapsed();

    assert_eq!(result.as_value().unwrap(), 0);
    assert!(
        elapsed < Duration::from_secs(1),
        "background command blocked for {elapsed:?}"
    );

    let t = bin_prog("true");
    let result = call(&map, "execute.nothrow.bg", vec![&t]).unwrap();
    assert_eq!(result.as_value().unwrap(), 0);
}

#[test]
fn arguments_are_not_interpreted_by_a_shell() {
    let map = map();
    let output = call(
        &map,
        "execute.capture",
        vec!["/bin/echo", "*.literal", "$HOME", "a b", "x|y", ">file"],
    )
    .unwrap();

    assert_eq!(
        output.as_string().unwrap(),
        b"*.literal $HOME a b x|y >file\n"
    );
}

#[test]
fn tilde_expansion_is_applied_only_to_legacy_variants() {
    let home = match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => home,
        _ => return,
    };

    let map = map();
    let expanded = call(
        &map,
        "execute.capture",
        vec!["/bin/echo", "~/rtorrent-exec-tilde"],
    )
    .unwrap();
    assert_eq!(
        expanded.as_string().unwrap(),
        format!("{home}/rtorrent-exec-tilde\n").as_bytes()
    );

    // `execute.raw_nothrow` must leave the argument untouched. The helper
    // shell exits 0 only when it sees the literal `~/...` spelling.
    let raw = call(
        &map,
        "execute.raw_nothrow",
        vec![
            "/bin/sh",
            "-c",
            "test \"$1\" = \"~/rtorrent-exec-tilde\"",
            "sh",
            "~/rtorrent-exec-tilde",
        ],
    )
    .unwrap();
    assert_eq!(raw.as_value().unwrap(), 0);
}

#[test]
fn execute2_is_a_legacy_alias_for_execute() {
    let map = map();
    let t = bin_prog("true");
    let f = bin_prog("false");

    assert_eq!(
        call(&map, "execute2", vec![&t])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
    assert!(call(&map, "execute2", vec![&f]).is_err());
}
