//! TDD contract for TASK-01 — `rstorrent-tui` CLI parsing.

use clap::Parser;
use rt_tui_client::cli::Cli;

#[test]
fn cli_parses_tcp_endpoint() {
    let cli =
        Cli::try_parse_from(["rstorrent-tui", "--endpoint", "localhost:5000"]).expect("valid CLI");
    assert_eq!(cli.endpoint, "localhost:5000");
    assert_eq!(cli.refresh_ms, 500); // default
}

#[test]
fn cli_parses_unix_socket_endpoint() {
    let cli = Cli::try_parse_from(["rstorrent-tui", "--endpoint", "unix:///tmp/rstorrent.sock"])
        .expect("valid CLI");
    assert_eq!(cli.endpoint, "unix:///tmp/rstorrent.sock");
}

#[test]
fn cli_rejects_missing_endpoint() {
    let result = Cli::try_parse_from(["rstorrent-tui"]);
    assert!(result.is_err());
}

#[test]
fn cli_defaults_theme() {
    let cli =
        Cli::try_parse_from(["rstorrent-tui", "--endpoint", "localhost:5000"]).expect("valid CLI");
    assert_eq!(cli.theme, "default");
}

#[test]
fn cli_accepts_theme_and_refresh_overrides() {
    let cli = Cli::try_parse_from([
        "rstorrent-tui",
        "--endpoint",
        "localhost:5000",
        "--theme",
        "dracula",
        "--refresh-ms",
        "1000",
    ])
    .expect("valid CLI");
    assert_eq!(cli.theme, "dracula");
    assert_eq!(cli.refresh_ms, 1000);
}
