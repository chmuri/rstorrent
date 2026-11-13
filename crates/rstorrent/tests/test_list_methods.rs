use clap::Parser;
use lt_common::Object;
use rstorrent::cli::Cli;
use rstorrent::control::Control;
use rt_rpc::Target;

/// Each test locks its own session directory; sharing the XDG default would
/// make the two `initialize` calls in this binary race for the same lock.
fn unique_session_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rstorrent_list_methods_{tag}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_methods_includes_all_command_families() {
    let session = unique_session_dir("families");
    let cli = Cli::parse_from([
        "rstorrent",
        "-n",
        "--daemon",
        "-p",
        "0-0",
        "-s",
        session.to_str().unwrap(),
    ]);
    let mut control = Control::new();
    control.initialize(&cli).expect("control initialization");

    let names = {
        let map = control.command_map.read().unwrap();
        map.call_command(
            "system.listMethods",
            &Object::create_list(),
            &Target::None,
            true,
        )
        .unwrap()
        .as_list()
        .unwrap()
        .iter()
        .filter_map(|name| name.as_str().ok().map(|name| name.to_string()))
        .collect::<Vec<_>>()
    };

    for expected in [
        "d.name",
        "f.priority",
        "p.client_version",
        "t.url",
        "view.add",
        "ratio.min",
        "convert.xb",
        "schedule",
        "log.print",
        "session.save",
        "network.xmlrpc.dialect.set",
        "network.scgi.open_local",
        "lua.execute",
        "lua.execute.str",
        "system.listMethods",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "missing method {expected}"
        );
    }

    drop(control);
    let _ = std::fs::remove_dir_all(&session);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lua_execute_str_is_wired_into_daemon() {
    let session = unique_session_dir("lua");
    let cli = Cli::parse_from([
        "rstorrent",
        "-n",
        "--daemon",
        "-p",
        "0-0",
        "-s",
        session.to_str().unwrap(),
    ]);
    let mut control = Control::new();
    control.initialize(&cli).expect("control initialization");

    // A pure Lua chunk (no `rtorrent.call`) exercises the daemon wiring without
    // re-entering the command-map read lock held by the dispatcher.
    let result = {
        let map = control.command_map.read().unwrap();
        map.call_command(
            "lua.execute.str",
            &Object::from("return 20 + 22"),
            &Target::None,
            true,
        )
        .expect("lua.execute.str")
    };
    assert_eq!(result.as_value().unwrap(), 42);

    drop(control);
    let _ = std::fs::remove_dir_all(&session);
}
