use clap::Parser;
use lt_common::Object;
use rstorrent::cli::Cli;
use rstorrent::control::Control;
use rt_rpc::Target;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_methods_includes_all_command_families() {
    let cli = Cli::parse_from(["rtorrent", "-n", "--daemon", "-p", "0-0"]);
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
        "system.listMethods",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "missing method {expected}"
        );
    }
}
