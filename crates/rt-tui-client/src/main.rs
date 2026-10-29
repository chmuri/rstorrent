//! `rstorrent-tui` — standalone gRPC TUI client for `rstorrent`.

use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;

use rt_tui_client::app::App;
use rt_tui_client::bridge::{GrpcBridge, DEFAULT_RPC_TIMEOUT};
use rt_tui_client::cli::{Cli, DEFAULT_REFRESH_MS};
use rt_tui_client::config::TuiConfig;
use rt_tui_client::run::run;
use rt_tui_client::state::AppState;
use rt_tui_client::terminal::{install_panic_hook, TerminalSession};

#[tokio::main]
async fn main() -> ExitCode {
    install_panic_hook();
    let cli = Cli::parse();

    let config = match TuiConfig::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("warning: {error}");
            TuiConfig::default()
        }
    };

    if let Some(configured) = config.endpoint.as_ref() {
        if configured != &cli.endpoint {
            eprintln!("note: config endpoint `{configured}` ignored, --endpoint wins");
        }
    }

    // The CLI default means "not given", so the config file can supply it.
    let refresh_ms = if cli.refresh_ms == DEFAULT_REFRESH_MS {
        config.refresh_ms
    } else {
        cli.refresh_ms
    };

    // Pre-flight: validate the endpoint and report reachability before the
    // alternate screen hides the message.
    if let Err(error) = run(cli.clone()).await {
        eprintln!("{error}");
        return ExitCode::from(error.exit_code());
    }

    let bridge = match GrpcBridge::connect(&cli.endpoint, DEFAULT_RPC_TIMEOUT).await {
        Ok(bridge) => bridge,
        Err(error) => {
            eprintln!("cannot reach {}: {error}", cli.endpoint);
            return ExitCode::FAILURE;
        }
    };

    let mut session = match TerminalSession::enter() {
        Ok(session) => session,
        Err(error) => {
            eprintln!("cannot start the terminal: {error}");
            return ExitCode::FAILURE;
        }
    };

    let mut app = App::new(AppState::new(), bridge, Duration::from_millis(refresh_ms))
        .with_endpoint(&cli.endpoint)
        .with_streams(&cli.endpoint)
        .with_layout(&config.keys_layout)
        .with_theme(config.theme());
    app.log("client started");

    let outcome = app.run(session.terminal_mut()).await;
    drop(session); // restore the terminal before printing anything

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("terminal error: {error}");
            ExitCode::FAILURE
        }
    }
}
