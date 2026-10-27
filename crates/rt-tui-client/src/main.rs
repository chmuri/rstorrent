//! `rstorrent-tui` — standalone gRPC TUI client for `rstorrent`.

use std::process::ExitCode;

use clap::Parser;

use rt_tui_client::cli::Cli;
use rt_tui_client::run::run;

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(error.exit_code())
        }
    }
}
